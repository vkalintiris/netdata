use std::fs;
use std::path::{Path, PathBuf};

use file_registry::{FileDir, FileRegistry};

use crate::format::{HEADER_SIZE, WalEvent};
use crate::{ByteSize, FileId, TimestampNs};
use crate::{Error, Result};

const WAL_EXT: &str = "wal";

/// Lifecycle status of a WAL file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// The ingester is actively writing to this file.
    Active,
    /// The ingester has finished writing; the file is immutable.
    Archived,
}

/// A WAL file tracked by the registry.
#[derive(Debug, Clone)]
pub struct File {
    pub id: FileId,
    pub status: FileStatus,
    pub created_at_ns: TimestampNs,
    pub size: ByteSize,
}

/// An ordered collection of WAL files.
///
/// Files are keyed by sequence number, which provides chronological ordering.
pub struct Registry {
    files: FileRegistry<File>,
}

impl Registry {
    pub fn new(path: &Path) -> Self {
        Self {
            files: FileRegistry::new(FileDir::new(path, WAL_EXT)),
        }
    }

    /// Recovers registry state by scanning the directory and reading file headers.
    pub fn recover(&mut self) -> Result<()> {
        let entries = self.files.dir().scan()?;

        for (file_id, meta) in entries {
            let path = self.files.file_path(file_id);

            let header = match read_header(&path) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!("failed to read WAL header {}: {e}", path.display());
                    continue;
                }
            };

            let size = ByteSize(meta.len());

            self.files.insert(
                file_id.seq,
                File {
                    id: file_id,
                    status: FileStatus::Archived,
                    created_at_ns: TimestampNs(header.created_at),
                    size,
                },
            );
        }

        Ok(())
    }

    pub fn path(&self) -> &Path {
        self.files.dir().path()
    }

    /// Derive the on-disk path for a WAL file.
    pub fn file_path(&self, id: FileId) -> PathBuf {
        self.files.file_path(id)
    }

    /// Scan the directory for the highest existing sequence number.
    pub fn scan_max_sequence(&self) -> Result<u64> {
        Ok(self.files.dir().scan_max_sequence()?)
    }

    /// Applies a `WalEvent` from the ingester.
    pub fn apply_event(&mut self, event: &WalEvent) -> Result<()> {
        match event {
            WalEvent::FileCreated {
                file_id,
                created_at_ns,
            } => {
                if self.files.contains(file_id.seq) {
                    return Err(Error::DuplicateSequence(file_id.seq));
                }
                self.files.insert(
                    file_id.seq,
                    File {
                        id: *file_id,
                        status: FileStatus::Active,
                        created_at_ns: *created_at_ns,
                        size: ByteSize::ZERO,
                    },
                );
                Ok(())
            }
            WalEvent::FileSynced { .. } => Ok(()),
            WalEvent::FileCompleted { file_id, size, .. } => {
                let entry = self
                    .files
                    .get_mut(file_id.seq)
                    .ok_or(Error::UnknownSequence(file_id.seq))?;
                entry.status = FileStatus::Archived;
                entry.size = *size;
                Ok(())
            }
        }
    }

    /// Look up a file by sequence number.
    pub fn get(&self, seq: u64) -> Option<&File> {
        self.files.get(seq)
    }

    /// Removes a file by sequence number.
    pub fn remove_by_seq(&mut self, seq: u64) -> Option<File> {
        self.files.remove(seq)
    }

    /// Returns all archived files, ordered by sequence number.
    pub fn archived_files(&self) -> impl Iterator<Item = &File> {
        self.files
            .values()
            .filter(|f| f.status == FileStatus::Archived)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
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
    use crate::{Config, Ingester, RotationConfig};

    fn test_file_id(seq: u64) -> FileId {
        let machine_id = uuid::Uuid::try_parse("550e8400e29b41d4a716446655440000").unwrap();
        let boot_id = uuid::Uuid::try_parse("7f3b2a1e9c4d4f8ab1c2d3e4f5a6b7c8").unwrap();
        FileId::new(machine_id, boot_id, seq, 0)
    }

    /// Helper: create an Ingester, write entries, shutdown, and return all events.
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
        let seq = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let mut ingester = Ingester::new(dir, config, seq).unwrap();
        let mut all_events = Vec::new();
        for &count in entry_counts {
            for i in 0..count {
                ingester
                    .write_frame(0, &(i as u32).to_le_bytes(), 1)
                    .unwrap();
            }
            all_events.extend(ingester.take_all_events());
        }
        all_events.extend(ingester.shutdown_all().unwrap());
        all_events
    }

    #[test]
    fn apply_events_tracks_files() {
        let dir = tempfile::tempdir().unwrap();
        let events = write_wal_files(dir.path(), &[10, 10, 10]);

        let mut registry = Registry::new(dir.path());
        registry.recover().unwrap();
        // recover finds all files as Archived; clear them to test apply_event from scratch
        for seq in [1u64, 2, 3] {
            registry.remove_by_seq(seq);
        }

        for event in &events {
            registry.apply_event(event).unwrap();
        }

        assert_eq!(registry.len(), 3);
        assert!(registry.archived_files().count() == 3);

        let seqs: Vec<u64> = registry.archived_files().map(|f| f.id.seq).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
    }

    #[test]
    fn recover_from_directory() {
        let dir = tempfile::tempdir().unwrap();
        let _ = write_wal_files(dir.path(), &[10, 10]);

        let mut registry = Registry::new(dir.path());
        registry.recover().unwrap();
        assert_eq!(registry.len(), 2);
        assert_eq!(registry.archived_files().count(), 2);

        let seqs: Vec<u64> = registry.archived_files().map(|f| f.id.seq).collect();
        assert_eq!(seqs, vec![1, 2]);
    }

    #[test]
    fn remove_by_seq() {
        let dir = tempfile::tempdir().unwrap();
        let _events = write_wal_files(dir.path(), &[10, 10]);

        let mut registry = Registry::new(dir.path());
        registry.recover().unwrap();
        assert_eq!(registry.len(), 2);

        let removed = registry.remove_by_seq(1).unwrap();
        assert_eq!(removed.id.seq, 1);
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn active_then_archived() {
        let dir = tempfile::tempdir().unwrap();
        let id = test_file_id(1);

        let mut registry = Registry::new(dir.path());
        registry.recover().unwrap();

        registry
            .apply_event(&WalEvent::FileCreated {
                file_id: id,
                created_at_ns: TimestampNs(1_000_000_000),
            })
            .unwrap();

        // Active files are not in archived_files
        assert_eq!(registry.len(), 1);
        assert_eq!(registry.archived_files().count(), 0);

        registry
            .apply_event(&WalEvent::FileCompleted {
                file_id: id,
                frame_count: 1,
                min_timestamp_ns: TimestampNs(1_000_000_000),
                max_timestamp_ns: TimestampNs(1_000_000_000),
                size: ByteSize(4096),
            })
            .unwrap();

        assert_eq!(registry.archived_files().count(), 1);
    }

    #[test]
    fn duplicate_sequence_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let id = test_file_id(1);

        let mut registry = Registry::new(dir.path());
        registry.recover().unwrap();

        registry
            .apply_event(&WalEvent::FileCreated {
                file_id: id,
                created_at_ns: TimestampNs(1_000_000_000),
            })
            .unwrap();
        let err = registry
            .apply_event(&WalEvent::FileCreated {
                file_id: id,
                created_at_ns: TimestampNs(2_000_000_000),
            })
            .unwrap_err();
        assert!(matches!(err, Error::DuplicateSequence(1)));
    }
}
