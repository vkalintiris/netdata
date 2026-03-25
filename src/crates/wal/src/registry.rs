use std::collections::BTreeMap;
use std::fs;

use crate::format::{HEADER_SIZE, WalEvent};
use crate::types::{ByteSize, FileId, TimestampNs};
use crate::waldir::WalDir;
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
#[derive(Debug, Clone)]
pub struct WalFileEntry {
    pub id: FileId,
    pub status: WalFileStatus,
    pub created_at_ns: TimestampNs,
    pub size: ByteSize,
}

/// An ordered collection of WAL files.
///
/// Files are keyed by sequence number, which provides chronological ordering.
/// Path derivation is delegated to the owned [`WalDir`].
pub struct WalRegistry {
    dir: WalDir,
    files: BTreeMap<u64, WalFileEntry>,
}

impl WalRegistry {
    pub fn new(dir: WalDir) -> Self {
        Self {
            dir,
            files: BTreeMap::new(),
        }
    }

    /// Recovers registry state by scanning the directory and reading file headers.
    pub fn recover(dir: WalDir) -> Result<Self> {
        let mut registry = Self::new(dir);

        let entries = match fs::read_dir(registry.dir.path()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(registry),
            Err(e) => return Err(e.into()),
        };

        for dir_entry in entries.flatten() {
            let path = dir_entry.path();

            let Some(id) = FileId::parse(&path) else {
                tracing::warn!("skipping file with unparseable name: {}", path.display());
                continue;
            };

            let header = match read_header(&path) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!("failed to read WAL header {}: {e}", path.display());
                    continue;
                }
            };

            let size = ByteSize(fs::metadata(&path).map(|m| m.len()).unwrap_or(0));

            registry.files.insert(
                id.seq,
                WalFileEntry {
                    id,
                    status: WalFileStatus::Archived,
                    created_at_ns: TimestampNs(header.created_at),
                    size,
                },
            );
        }

        Ok(registry)
    }

    pub fn dir(&self) -> &WalDir {
        &self.dir
    }

    /// Registers a new active file.
    fn track_active(&mut self, id: FileId, created_at_ns: TimestampNs) -> Result<()> {
        if self.files.contains_key(&id.seq) {
            return Err(Error::DuplicateSequence(id.seq));
        }
        self.files.insert(
            id.seq,
            WalFileEntry {
                id,
                status: WalFileStatus::Active,
                created_at_ns,
                size: ByteSize::ZERO,
            },
        );
        Ok(())
    }

    /// Marks a file as archived.
    fn mark_archived(&mut self, id: FileId, size: ByteSize) -> Result<()> {
        let entry = self
            .files
            .get_mut(&id.seq)
            .ok_or(Error::UnknownSequence(id.seq))?;
        entry.status = WalFileStatus::Archived;
        entry.size = size;
        Ok(())
    }

    /// Applies a `WalEvent` from the ingester.
    pub fn apply_event(&mut self, event: &WalEvent) -> Result<()> {
        match event {
            WalEvent::FileCreated { id, created_at_ns } => self.track_active(*id, *created_at_ns),
            WalEvent::FileSynced { .. } => Ok(()),
            WalEvent::FileCompleted { id, size, .. } => self.mark_archived(*id, *size),
        }
    }

    /// Removes a file from the registry.
    pub fn remove(&mut self, id: FileId) -> Option<WalFileEntry> {
        self.files.remove(&id.seq)
    }

    /// Returns a file by its id.
    pub fn get(&self, id: FileId) -> Option<&WalFileEntry> {
        self.files.get(&id.seq)
    }

    /// Returns all archived files, ordered by sequence number.
    pub fn archived_files(&self) -> impl Iterator<Item = &WalFileEntry> {
        self.files
            .values()
            .filter(|f| f.status == WalFileStatus::Archived)
    }

    /// Returns all files in sequence order.
    pub fn iter(&self) -> impl Iterator<Item = &WalFileEntry> {
        self.files.values()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Evaluates a retention policy and returns the FileIds of files to delete.
    pub fn evaluate_retention(
        &self,
        max_files: usize,
        max_total_size: ByteSize,
        max_age_ns: TimestampNs,
        now_ns: TimestampNs,
    ) -> Vec<FileId> {
        let eligible: Vec<&WalFileEntry> = self
            .files
            .values()
            .filter(|f| f.status == WalFileStatus::Archived)
            .collect();

        let total_files = eligible.len();
        let total_size: u64 = eligible.iter().map(|f| f.size.as_u64()).sum();

        let mut to_delete = Vec::new();
        let mut remaining_files = total_files;
        let mut remaining_size = total_size;

        for entry in &eligible {
            let mut should_delete = false;

            if remaining_files > max_files {
                should_delete = true;
            }
            if remaining_size > max_total_size.as_u64() {
                should_delete = true;
            }
            if now_ns.saturating_sub(entry.created_at_ns) > max_age_ns.as_u64() {
                should_delete = true;
            }

            if should_delete {
                to_delete.push(entry.id);
                remaining_files -= 1;
                remaining_size -= entry.size.as_u64();
            }
        }

        to_delete
    }
}

/// Read and parse the WAL file header.
fn read_header(path: &std::path::Path) -> Result<crate::format::FileHeader> {
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

    fn test_wal_dir(dir: &std::path::Path) -> WalDir {
        WalDir::new(
            dir,
            uuid::Uuid::try_parse("550e8400e29b41d4a716446655440000").unwrap(),
            uuid::Uuid::try_parse("7f3b2a1e9c4d4f8ab1c2d3e4f5a6b7c8").unwrap(),
        )
    }

    fn test_file_id(dir: &std::path::Path, seq: u64) -> FileId {
        let d = test_wal_dir(dir);
        FileId::new(d.machine_id(), d.boot_id(), seq)
    }

    /// Helper: create a WalWriter, write entries, shutdown, and return all events.
    fn write_wal_files(dir: &std::path::Path, entry_counts: &[usize]) -> Vec<WalEvent> {
        let entries_per_file: usize = *entry_counts.iter().max().unwrap_or(&10);
        let config = Config {
            rotation: RotationConfig {
                max_log_entries: entries_per_file,
                max_file_size: ByteSize(u64::MAX),
                max_duration: None,
            },
            crc_enabled: false,
            compression_enabled: true,
        };
        let wal_dir = test_wal_dir(dir);
        let mut writer = WalWriter::new(wal_dir, config).unwrap();
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

        let mut registry = WalRegistry::new(test_wal_dir(dir.path()));
        for event in &events {
            registry.apply_event(event).unwrap();
        }

        assert_eq!(registry.len(), 3);
        assert!(registry.iter().all(|f| f.status == WalFileStatus::Archived));

        let seqs: Vec<u64> = registry.iter().map(|f| f.id.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[test]
    fn recover_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let _ = write_wal_files(dir.path(), &[10, 10]);

        let registry = WalRegistry::recover(test_wal_dir(dir.path())).unwrap();
        assert_eq!(registry.len(), 2);
        assert!(registry.iter().all(|f| f.status == WalFileStatus::Archived));

        let seqs: Vec<u64> = registry.iter().map(|f| f.id.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn remove_file() {
        let dir = tempfile::tempdir().unwrap();
        let events = write_wal_files(dir.path(), &[10, 10]);

        let mut registry = WalRegistry::new(test_wal_dir(dir.path()));
        for event in &events {
            registry.apply_event(event).unwrap();
        }
        assert_eq!(registry.len(), 2);

        let id = test_file_id(dir.path(), 1);
        let removed = registry.remove(id).unwrap();
        assert_eq!(removed.id.seq, 1);
        assert_eq!(registry.len(), 1);
        assert!(registry.get(id).is_none());
    }

    #[test]
    fn active_then_archived() {
        let dir = tempfile::tempdir().unwrap();
        let id = test_file_id(dir.path(), 1);

        let mut registry = WalRegistry::new(test_wal_dir(dir.path()));

        registry
            .apply_event(&WalEvent::FileCreated {
                id,
                created_at_ns: TimestampNs(1_000_000_000),
            })
            .unwrap();
        assert!(registry.get(id).unwrap().status == WalFileStatus::Active);

        registry
            .apply_event(&WalEvent::FileCompleted {
                id,
                frame_count: 1,
                min_timestamp_ns: TimestampNs(1_000_000_000),
                max_timestamp_ns: TimestampNs(1_000_000_000),
                size: ByteSize(4096),
            })
            .unwrap();
        assert!(registry.get(id).unwrap().status == WalFileStatus::Archived);
    }

    #[test]
    fn duplicate_sequence_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let id = test_file_id(dir.path(), 1);

        let mut registry = WalRegistry::new(test_wal_dir(dir.path()));

        registry
            .apply_event(&WalEvent::FileCreated {
                id,
                created_at_ns: TimestampNs(1_000_000_000),
            })
            .unwrap();
        let err = registry
            .apply_event(&WalEvent::FileCreated {
                id,
                created_at_ns: TimestampNs(2_000_000_000),
            })
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateSequence(1)));
    }
}
