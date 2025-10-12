use crate::error::{JournalError, Result};
use crate::log::RetentionPolicy;
use crate::registry::{File as RegistryFile, Origin, Source, Status, paths};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
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

// Helper to get file size
fn get_file_size(path: impl AsRef<Path>) -> Result<u64> {
    let metadata = std::fs::metadata(path)?;
    Ok(metadata.len())
}

/// Manages a directory of journal files with automatic cleanup.
///
/// Scans the directory for existing files, tracks their sizes, and enforces retention
/// policies. Typically not used directly - see [`JournalLog`](crate::JournalLog) instead.
#[derive(Debug)]
pub struct Chain {
    pub(crate) path: PathBuf,
    pub(crate) inner: paths::Chain,
    pub(crate) file_sizes: std::collections::HashMap<String, u64>,
    pub(crate) total_size: u64,
}

impl Chain {
    /// Creates a new directory manager, scanning for existing journal files.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(path: PathBuf) -> Result<Self> {
        // Create directory if it does not already exist.
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
        } else if !path.is_dir() {
            return Err(JournalError::NotADirectory);
        }

        let mut journal_directory = Self {
            path,
            inner: paths::Chain::default(),
            file_sizes: std::collections::HashMap::new(),
            total_size: 0,
        };

        for entry in std::fs::read_dir(&journal_directory.path)? {
            let entry = entry?;
            let file_path = entry.path();

            if file_path.is_dir() {
                // Scan subdirectories for journal files
                for subentry in std::fs::read_dir(&file_path)? {
                    let subentry = subentry?;
                    let subpath = subentry.path();

                    if subpath.extension() != Some(OsStr::new("journal")) {
                        continue;
                    }

                    if let Some(file) = RegistryFile::from_path(&subpath) {
                        let file_size = get_file_size(&subpath).unwrap_or(0);
                        journal_directory.total_size += file_size;
                        journal_directory
                            .file_sizes
                            .insert(file.path.clone(), file_size);
                        journal_directory.inner.insert_file(file);
                    }
                }
            } else if file_path.extension() == Some(OsStr::new("journal")) {
                // Handle journal files in the root directory (for backward compatibility)
                if let Some(file) = RegistryFile::from_path(&file_path) {
                    let file_size = get_file_size(&file_path).unwrap_or(0);
                    journal_directory.total_size += file_size;
                    journal_directory
                        .file_sizes
                        .insert(file.path.clone(), file_size);
                    journal_directory.inner.insert_file(file);
                }
            }
        }

        Ok(journal_directory)
    }

    /// Registers a new journal file with the directory.
    pub fn create_file(
        &mut self,
        machine_id: Uuid,
        seqnum_id: Uuid,
        head_seqnum: u64,
        head_realtime: u64,
    ) -> Result<RegistryFile> {
        let file = create_chain_file(machine_id, seqnum_id, head_seqnum, head_realtime);
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
