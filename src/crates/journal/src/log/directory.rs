use super::config::Config;
use crate::error::{JournalError, Result};
use crate::registry::{File as RegistryFile, Origin, Source, Status};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

// Helper function to create a File with archived status
pub(crate) fn create_journal_file(
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
pub struct JournalDirectory {
    pub(crate) path: PathBuf,
    pub(crate) config: Config,
    /// Files ordered by head_realtime and head_seqnum (oldest first)
    pub(crate) files: Vec<RegistryFile>,
    /// Cached file sizes (path -> size)
    pub(crate) file_sizes: std::collections::HashMap<String, u64>,
    /// Cached total size of all files
    pub(crate) total_size: u64,
}

impl JournalDirectory {
    /// Creates a new directory manager, scanning for existing journal files.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn with_config(path: PathBuf, config: Config) -> Result<Self> {
        // Create directory if it does not already exist.
        if !path.exists() {
            std::fs::create_dir_all(&path)?;
        } else if !path.is_dir() {
            return Err(JournalError::NotADirectory);
        }

        let mut journal_directory = Self {
            path,
            config,
            files: Vec::new(),
            file_sizes: std::collections::HashMap::new(),
            total_size: 0,
        };

        // Read all .journal files from directory recursively (to handle machine_id subdirs)
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

                    // Use journal_registry::File::from_path for parsing
                    if let Some(file_info) = RegistryFile::from_path(&subpath) {
                        let file_size = get_file_size(&subpath).unwrap_or(0);
                        journal_directory.total_size += file_size;
                        journal_directory
                            .file_sizes
                            .insert(file_info.path.clone(), file_size);
                        journal_directory.files.push(file_info);
                    }
                }
            } else if file_path.extension() == Some(OsStr::new("journal")) {
                // Handle journal files in the root directory (for backward compatibility)
                if let Some(file_info) = RegistryFile::from_path(&file_path) {
                    let file_size = get_file_size(&file_path).unwrap_or(0);
                    journal_directory.total_size += file_size;
                    journal_directory
                        .file_sizes
                        .insert(file_info.path.clone(), file_size);
                    journal_directory.files.push(file_info);
                }
            }
        }

        // Sort files - File already implements Ord
        journal_directory.files.sort();

        Ok(journal_directory)
    }

    pub fn get_full_path(&self, file_info: &RegistryFile) -> PathBuf {
        let path = Path::new(&file_info.path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.path.join(&file_info.path)
        }
    }

    /// Registers a new journal file with the directory.
    ///
    /// Does not create the physical file - use [`JournalFile::create`](journal_file::JournalFile::create).
    pub fn new_file(
        &mut self,
        machine_id: Uuid,
        seqnum_id: Uuid,
        head_seqnum: u64,
        head_realtime: u64,
    ) -> Result<RegistryFile> {
        let new_file = create_journal_file(machine_id, seqnum_id, head_seqnum, head_realtime);
        self.files.push(new_file.clone());
        Ok(new_file)
    }

    /// Remove the oldest file (by head_realtime/head_seqnum) from both filesystem and tracking
    fn remove_oldest_file(&mut self) -> Result<()> {
        if let Some(oldest_file) = self.files.first() {
            let file_path = self.get_full_path(oldest_file);
            let file_size = self.file_sizes.get(&oldest_file.path).copied().unwrap_or(0);

            // Remove from filesystem
            if let Err(e) = std::fs::remove_file(&file_path) {
                // Log error but continue cleanup - file might already be deleted
                eprintln!(
                    "Warning: Failed to remove journal file {:?}: {}",
                    file_path, e
                );
            }

            // Remove from tracking and update total size
            let removed_file = self.files.remove(0);
            self.file_sizes.remove(&removed_file.path);
            self.total_size = self.total_size.saturating_sub(file_size);
        }

        Ok(())
    }

    /// Remove files older than the specified cutoff time
    fn remove_files_older_than(&mut self, cutoff_time: SystemTime) -> Result<()> {
        let cutoff_micros = cutoff_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;

        // Find files older than cutoff time (head_realtime is in microseconds since epoch)
        let mut files_to_remove = Vec::new();
        for (index, file) in self.files.iter().enumerate() {
            // Extract head_realtime from Status
            if let Status::Archived { head_realtime, .. } = file.status {
                if head_realtime <= cutoff_micros {
                    files_to_remove.push(index);
                }
            }
        }

        // Remove files in reverse order to maintain indices
        for &index in files_to_remove.iter().rev() {
            let file = &self.files[index];
            let file_path = self.get_full_path(file);
            let file_size = self.file_sizes.get(&file.path).copied().unwrap_or(0);

            // Remove from filesystem
            if let Err(e) = std::fs::remove_file(&file_path) {
                // Log error but continue cleanup
                eprintln!(
                    "Warning: Failed to remove journal file {:?}: {}",
                    file_path, e
                );
            }

            // Remove from tracking and update total size
            let removed_file = self.files.remove(index);
            self.file_sizes.remove(&removed_file.path);
            self.total_size = self.total_size.saturating_sub(file_size);
        }

        Ok(())
    }

    /// Deletes old files to satisfy retention policy limits.
    ///
    /// Removes oldest files first until all limits are met.
    pub fn enforce_retention_policy(&mut self) -> Result<()> {
        let policy = self.config.retention_policy;

        // 1. Remove by file count limit
        if let Some(max_files) = policy.number_of_journal_files {
            while self.files.len() > max_files {
                self.remove_oldest_file()?;
            }
        }

        // 2. Remove by total size limit
        if let Some(max_total_size) = policy.size_of_journal_files {
            while self.total_size > max_total_size && !self.files.is_empty() {
                self.remove_oldest_file()?;
            }
        }

        // 3. Remove by entry age limit
        if let Some(max_entry_age) = policy.duration_of_journal_files {
            let cutoff_time = SystemTime::now()
                .checked_sub(max_entry_age)
                .unwrap_or(SystemTime::UNIX_EPOCH);
            self.remove_files_older_than(cutoff_time)?;
        }

        Ok(())
    }
}
