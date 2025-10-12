use crate::error::Result;
use crate::log::RetentionPolicy;
use crate::registry::{File as RegistryFile, Origin, Source, Status, paths};
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

// Helper function to create a File with archived status
pub(crate) fn create_chain_file(
    machine_id: Uuid,
    seqnum_id: Uuid,
    head_seqnum: u64,
    head_realtime: u64,
) -> RegistryFile {
    let origin = Origin {
        machine_id: Some(machine_id),
        namespace: None,
        source: Source::System,
    };

    let status = Status::Archived {
        seqnum_id,
        head_seqnum,
        head_realtime,
    };

    // Format the path using the same logic as journal_registry
    let path = format!(
        "system@{}-{:016x}-{:016x}.journal",
        seqnum_id.simple(),
        head_seqnum,
        head_realtime
    );

    RegistryFile {
        path,
        origin,
        status,
    }
}

/// Manages a directory of journal files with automatic cleanup.
///
/// Scans the directory for existing files, tracks their sizes, and enforces retention
/// policies. Typically not used directly - see [`JournalLog`](crate::JournalLog) instead.
#[derive(Debug)]
pub struct Chain {
    pub(crate) path: PathBuf,
    pub(crate) machine_id: Uuid,

    pub(crate) inner: paths::Chain,
    pub(crate) file_sizes: std::collections::HashMap<String, u64>,
    pub(crate) total_size: u64,
}

impl Chain {
    /// Creates a new directory manager, scanning for existing journal files.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(path: PathBuf, machine_id: Uuid) -> Result<Self> {
        #[cfg(debug_assertions)]
        {
            debug_assert!(path.exists() && path.is_dir());

            let parent = path.parent().unwrap();
            let filename = parent.file_name().unwrap().as_bytes();
            debug_assert_eq!(Ok(machine_id), Uuid::try_parse_ascii(filename));
        }

        let mut chain = Self {
            path,
            machine_id,
            inner: paths::Chain::default(),
            file_sizes: std::collections::HashMap::new(),
            total_size: 0,
        };

        for entry in std::fs::read_dir(&chain.path)? {
            let file_path = entry?.path();

            let Some(file) = RegistryFile::from_path(&file_path) else {
                continue;
            };

            let Ok(size) = std::fs::metadata(&file.path).map(|m| m.len()) else {
                continue;
            };

            chain.total_size += size;
            chain.file_sizes.insert(file.path.clone(), size);
            chain.inner.insert_file(file);
        }

        Ok(chain)
    }

    /// Registers a new journal file with the directory.
    pub fn create_file(
        &mut self,
        seqnum_id: Uuid,
        head_seqnum: u64,
        head_realtime: u64,
    ) -> Result<RegistryFile> {
        let file = create_chain_file(self.machine_id, seqnum_id, head_seqnum, head_realtime);
        self.inner.insert_file(file.clone());
        Ok(file)
    }

    /// Retains the files that satisfy retention policy limits.
    pub fn retain(&mut self, retention_policy: &RetentionPolicy) -> Result<()> {
        // Remove by file count limit
        if let Some(max_files) = retention_policy.number_of_journal_files {
            while self.inner.len() > max_files {
                self.delete_oldest_file()?;
            }
        }

        // Remove by total size limit
        if let Some(max_total_size) = retention_policy.size_of_journal_files {
            while self.total_size > max_total_size && !self.inner.is_empty() {
                self.delete_oldest_file()?;
            }
        }

        // Remove by entry age limit
        if let Some(max_entry_age) = retention_policy.duration_of_journal_files {
            self.delete_files_older_than(max_entry_age)?;
        }

        Ok(())
    }

    /// Remove the oldest file
    fn delete_oldest_file(&mut self) -> Result<()> {
        let Some(file) = self.inner.pop_back() else {
            return Ok(());
        };

        let file_size = self.file_sizes.get(&file.path).copied().unwrap_or(0);

        // Remove from filesystem
        if let Err(e) = std::fs::remove_file(&file.path) {
            // Log error but continue cleanup - file might already be deleted
            eprintln!(
                "Warning: Failed to remove journal file {:?}: {}",
                file.path, e
            );
        }

        self.file_sizes.remove(&file.path);
        self.total_size = self.total_size.saturating_sub(file_size);
        Ok(())
    }

    /// Remove files older than the specified cutoff time
    fn delete_files_older_than(&mut self, max_entry_age: std::time::Duration) -> Result<()> {
        let cutoff_time = SystemTime::now()
            .checked_sub(max_entry_age)
            .unwrap_or(SystemTime::UNIX_EPOCH);

        let cutoff_time = cutoff_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        for file in self.inner.drain(cutoff_time) {
            let file_size = self.file_sizes.get(&file.path).copied().unwrap_or(0);

            // Remove from filesystem
            if let Err(e) = std::fs::remove_file(&file.path) {
                // Log error but continue cleanup
                eprintln!(
                    "Warning: Failed to remove journal file {:?}: {}",
                    file.path, e
                );
            }

            self.file_sizes.remove(&file.path);
            self.total_size = self.total_size.saturating_sub(file_size);
        }

        Ok(())
    }
}
