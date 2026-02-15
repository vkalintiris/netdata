use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::format::{HEADER_SIZE, WalEvent, parse_sequence};
use crate::{Error, Result};

/// Lifecycle status of a WAL file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalFileStatus {
    /// The ingester is actively writing to this file.
    Active,
    /// The ingester has finished writing; the file is immutable.
    Archived,
}

/// A WAL file tracked by the registry.
///
/// The registry tracks existence and lifecycle status. Auxiliary metadata
/// (entry count, size, time range) is carried by `WalEvent::FileCompleted`
/// and consumed directly by the ledger — not stored here.
#[derive(Debug, Clone)]
pub struct WalFileEntry {
    pub path: PathBuf,
    pub sequence: u64,
    pub status: WalFileStatus,
    pub created_at_ns: u64,
    /// Whether a split-FST index has been successfully built for this file.
    pub indexed: bool,
    /// Size of the WAL file in bytes.
    pub size: u64,
}

/// An ordered collection of WAL files.
///
/// Files are keyed by sequence number, which provides chronological ordering.
/// The registry tracks existence and lifecycle status — it is the single
/// source of truth for what WAL files exist on the system.
pub struct WalRegistry {
    dir: PathBuf,
    files: BTreeMap<u64, WalFileEntry>,
}

impl WalRegistry {
    /// Creates an empty registry for the given directory.
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
            files: BTreeMap::new(),
        }
    }

    /// Recovers registry state by scanning the directory and reading file headers.
    ///
    /// Files that cannot be parsed (bad header, not a WAL file) are silently skipped.
    /// All discovered files are marked as `Archived` since the ingester is not
    /// running (or has restarted) — if the ingester has an active file, it will
    /// send a `FileCreated` event that overrides the status.
    pub fn recover(dir: &Path) -> Result<Self> {
        let mut registry = Self::new(dir);

        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(registry),
            Err(e) => return Err(e.into()),
        };

        for dir_entry in entries.flatten() {
            let path = dir_entry.path();

            // Clean up stale temp files from interrupted index writes.
            if path.extension().is_some_and(|ext| ext == "tmp") {
                if let Err(e) = fs::remove_file(&path) {
                    if e.kind() != std::io::ErrorKind::NotFound {
                        tracing::warn!(path = %path.display(), %e, "failed to remove stale tmp file");
                    }
                } else {
                    tracing::info!(path = %path.display(), "removed stale tmp file");
                }
                continue;
            }

            let Some(seq) = parse_sequence(&path) else {
                continue;
            };

            let header = match read_header(&path) {
                Ok(h) => h,
                Err(_) => continue,
            };

            let sfst_path = path.with_extension("sfst");
            let indexed = sfst_path.exists();
            let size = if indexed {
                fs::metadata(&sfst_path).map(|m| m.len()).unwrap_or(0)
            } else {
                fs::metadata(&path).map(|m| m.len()).unwrap_or(0)
            };
            registry.files.insert(
                seq,
                WalFileEntry {
                    path,
                    sequence: seq,
                    status: WalFileStatus::Archived,
                    created_at_ns: header.created_at,
                    indexed,
                    size,
                },
            );
        }

        Ok(registry)
    }

    /// Registers a new active file. Called when the ingester creates a WAL file.
    pub fn track_active(&mut self, path: PathBuf, sequence: u64, created_at_ns: u64) -> Result<()> {
        if self.files.contains_key(&sequence) {
            return Err(Error::DuplicateSequence(sequence));
        }
        self.files.insert(
            sequence,
            WalFileEntry {
                path,
                sequence,
                status: WalFileStatus::Active,
                created_at_ns,
                indexed: false,
                size: 0,
            },
        );
        Ok(())
    }

    /// Marks a file as archived. Called when the ingester completes a WAL file.
    pub fn mark_archived(&mut self, sequence: u64) -> Result<()> {
        let entry = self
            .files
            .get_mut(&sequence)
            .ok_or(Error::UnknownSequence(sequence))?;
        entry.status = WalFileStatus::Archived;
        Ok(())
    }

    /// Marks a file as indexed. Called when the indexer successfully builds
    /// a split-FST index for this file.
    ///
    /// Updates the tracked size to the `.sfst` file size, since the WAL
    /// `.bin` is deleted after indexing and retention accounts for index
    /// files only.
    pub fn mark_indexed(&mut self, sequence: u64) -> Result<()> {
        let entry = self
            .files
            .get_mut(&sequence)
            .ok_or(Error::UnknownSequence(sequence))?;
        entry.indexed = true;
        let sfst_path = entry.path.with_extension("sfst");
        if let Ok(meta) = fs::metadata(&sfst_path) {
            entry.size = meta.len();
        }
        Ok(())
    }

    /// Applies a `WalEvent` from the ingester.
    pub fn apply_event(&mut self, event: &WalEvent) -> Result<()> {
        match event {
            WalEvent::FileCreated {
                path,
                created_at_ns,
            } => {
                let Some(seq) = parse_sequence(path) else {
                    return Ok(());
                };
                self.track_active(path.clone(), seq, *created_at_ns)
            }
            WalEvent::DataSynced { .. } => Ok(()),
            WalEvent::FileCompleted { path, size, .. } => {
                let Some(seq) = parse_sequence(path) else {
                    return Ok(());
                };
                self.mark_archived(seq)?;
                if let Some(entry) = self.files.get_mut(&seq) {
                    entry.size = *size;
                }
                Ok(())
            }
        }
    }

    /// Removes a file from the registry. Called by the ledger after deleting a file.
    pub fn remove(&mut self, sequence: u64) -> Option<WalFileEntry> {
        self.files.remove(&sequence)
    }

    /// Returns all archived files, ordered by sequence number (chronological).
    pub fn archived_files(&self) -> impl Iterator<Item = &WalFileEntry> {
        self.files
            .values()
            .filter(|f| f.status == WalFileStatus::Archived)
    }

    /// Returns the total number of tracked files.
    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Returns the directory this registry manages.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Returns all files in sequence order.
    pub fn iter(&self) -> impl Iterator<Item = &WalFileEntry> {
        self.files.values()
    }

    /// Returns a file by its sequence number.
    pub fn get(&self, sequence: u64) -> Option<&WalFileEntry> {
        self.files.get(&sequence)
    }

    /// Evaluates a retention policy and returns the paths of files that should
    /// be deleted.
    ///
    /// Only archived+indexed files are eligible for deletion. Files are
    /// considered oldest-first (by sequence number). A file is marked for
    /// deletion if any of these conditions is true:
    ///
    /// - The number of indexed files exceeds `max_files`
    /// - The total size of indexed files exceeds `max_total_size`
    /// - The file's age exceeds `max_age`
    ///
    /// The `now_ns` parameter is the current wall-clock time in nanoseconds
    /// (same epoch as `created_at_ns`).
    pub fn evaluate_retention(
        &self,
        max_files: usize,
        max_total_size: u64,
        max_age_ns: u64,
        now_ns: u64,
    ) -> Vec<PathBuf> {
        let eligible: Vec<&WalFileEntry> = self
            .files
            .values()
            .filter(|f| f.status == WalFileStatus::Archived && f.indexed)
            .collect();

        let total_files = eligible.len();
        let total_size: u64 = eligible.iter().map(|f| f.size).sum();

        let mut to_delete = Vec::new();
        let mut remaining_files = total_files;
        let mut remaining_size = total_size;

        // Iterate oldest first (BTreeMap is sorted by sequence).
        for entry in &eligible {
            let mut should_delete = false;

            if remaining_files > max_files {
                should_delete = true;
            }
            if remaining_size > max_total_size {
                should_delete = true;
            }
            if now_ns.saturating_sub(entry.created_at_ns) > max_age_ns {
                should_delete = true;
            }

            if should_delete {
                to_delete.push(entry.path.clone());
                remaining_files -= 1;
                remaining_size -= entry.size;
            }
        }

        to_delete
    }
}

/// Read and parse the WAL file header.
fn read_header(path: &Path) -> Result<crate::format::FileHeader> {
    use std::io::Read;
    let mut file = fs::File::open(path)?;
    let mut buf = [0u8; HEADER_SIZE];
    file.read_exact(&mut buf)?;
    Ok(crate::format::FileHeader::from_bytes(&buf)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::WalEvent;
    use crate::{Config, RotationConfig, WalWriter};

    /// Helper: create a WalWriter, write entries, shutdown, and return all events.
    fn write_wal_files(dir: &Path, entry_counts: &[usize]) -> Vec<WalEvent> {
        let entries_per_file: usize = *entry_counts.iter().max().unwrap_or(&10);
        let config = Config {
            rotation: RotationConfig {
                max_log_entries: entries_per_file,
                max_file_size: u64::MAX,
                max_duration: None,
            },
            crc_enabled: false,
            compression_enabled: true,
        };
        let mut writer = WalWriter::new(dir, config).unwrap();
        let mut all_events = Vec::new();
        for &count in entry_counts {
            for i in 0..count {
                writer.write_frame(&(i as u32).to_le_bytes(), 1).unwrap();
            }
            all_events.extend(writer.take_events());
        }
        all_events.extend(writer.shutdown().unwrap());
        all_events
    }

    #[test]
    fn apply_events_tracks_files() {
        let dir = tempfile::tempdir().unwrap();
        let events = write_wal_files(dir.path(), &[10, 10, 10]);

        let mut registry = WalRegistry::new(dir.path());
        for event in &events {
            registry.apply_event(event).unwrap();
        }

        assert_eq!(registry.len(), 3);
        assert!(registry.iter().all(|f| f.status == WalFileStatus::Archived));

        // Files are in sequence order.
        let seqs: Vec<u64> = registry.iter().map(|f| f.sequence).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[test]
    fn recover_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let _ = write_wal_files(dir.path(), &[10, 10]);

        let registry = WalRegistry::recover(dir.path()).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.iter().all(|f| f.status == WalFileStatus::Archived));

        let seqs: Vec<u64> = registry.iter().map(|f| f.sequence).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn remove_file() {
        let dir = tempfile::tempdir().unwrap();
        let events = write_wal_files(dir.path(), &[10, 10]);

        let mut registry = WalRegistry::new(dir.path());
        for event in &events {
            registry.apply_event(event).unwrap();
        }
        assert_eq!(registry.len(), 2);

        let removed = registry.remove(1).unwrap();
        assert_eq!(removed.sequence, 1);
        assert_eq!(registry.len(), 1);
        assert!(registry.get(1).is_none());
    }

    #[test]
    fn active_then_archived() {
        let dir = tempfile::tempdir().unwrap();

        let mut registry = WalRegistry::new(dir.path());
        let path = dir.path().join("wal-0000000001.bin");

        registry
            .track_active(path.clone(), 1, 1_000_000_000)
            .unwrap();
        assert!(registry.get(1).unwrap().status == WalFileStatus::Active);

        registry.mark_archived(1).unwrap();
        assert!(registry.get(1).unwrap().status == WalFileStatus::Archived);
    }

    #[test]
    fn duplicate_sequence_rejected() {
        let dir = tempfile::tempdir().unwrap();

        let mut registry = WalRegistry::new(dir.path());
        let path = dir.path().join("wal-0000000001.bin");

        registry
            .track_active(path.clone(), 1, 1_000_000_000)
            .unwrap();
        let err = registry.track_active(path, 1, 2_000_000_000).unwrap_err();
        assert!(matches!(err, Error::DuplicateSequence(1)));
    }
}
