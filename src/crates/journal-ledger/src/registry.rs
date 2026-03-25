use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use wal::{ByteSize, FileId, TimestampNs};

// ---------------------------------------------------------------------------
// WAL files (.bin)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalFileStatus {
    /// The ingestor is actively writing to this file.
    Active,
    /// The ingestor has finished writing; the file is immutable.
    Archived,
}

#[derive(Debug, Clone)]
pub struct WalFile {
    pub id: FileId,
    pub created_at_ns: TimestampNs,
    pub status: WalFileStatus,
    pub size: ByteSize,
}

pub struct WalRegistry {
    dir: PathBuf,
    files: BTreeMap<u64, WalFile>,
}

impl WalRegistry {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            files: BTreeMap::new(),
        }
    }

    /// Derive the on-disk path for a WAL file from its FileId.
    pub fn path(&self, id: FileId) -> PathBuf {
        self.dir.join(id.to_filename("bin"))
    }

    /// Scan the directory for `.bin` files and reconstruct state.
    pub fn recover(dir: &Path) -> Self {
        let mut registry = Self::new(dir.to_path_buf());

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return registry,
        };

        for dir_entry in entries.flatten() {
            let path = dir_entry.path();

            let Some(id) = FileId::parse(&path) else {
                continue;
            };

            let created_at_ns = match read_wal_header(&path) {
                Ok(h) => TimestampNs(h.created_at),
                Err(_) => continue,
            };

            let size = ByteSize(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0));

            registry.files.insert(
                id.seq,
                WalFile {
                    id,
                    created_at_ns,
                    status: WalFileStatus::Archived,
                    size,
                },
            );
        }

        registry
    }

    pub fn track_active(&mut self, id: FileId, created_at_ns: TimestampNs) {
        self.files.insert(
            id.seq,
            WalFile {
                id,
                created_at_ns,
                status: WalFileStatus::Active,
                size: ByteSize::ZERO,
            },
        );
    }

    pub fn mark_archived(&mut self, id: FileId, size: ByteSize) {
        if let Some(entry) = self.files.get_mut(&id.seq) {
            entry.status = WalFileStatus::Archived;
            entry.size = size;
        }
    }

    pub fn remove(&mut self, seq: u64) -> Option<WalFile> {
        self.files.remove(&seq)
    }

    pub fn get(&self, seq: u64) -> Option<&WalFile> {
        self.files.get(&seq)
    }

    /// Returns FileIds of all archived WAL files.
    pub fn archived_ids(&self) -> Vec<FileId> {
        self.files
            .values()
            .filter(|f| f.status == WalFileStatus::Archived)
            .map(|f| f.id)
            .collect()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn read_wal_header(path: &Path) -> Result<wal::format::FileHeader, wal::Error> {
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; wal::format::HEADER_SIZE];
    file.read_exact(&mut buf)?;
    wal::format::FileHeader::from_bytes(&buf)
}

// ---------------------------------------------------------------------------
// Index files (.sfst)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct IndexFile {
    pub sequence: u64,
    pub created_at_ns: TimestampNs,
    pub size: ByteSize,
    pending_deletion: bool,
}

pub struct IndexRegistry {
    dir: PathBuf,
    files: BTreeMap<u64, IndexFile>,
}

impl IndexRegistry {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            files: BTreeMap::new(),
        }
    }

    /// Derive the on-disk path for an index file from its sequence number.
    pub fn path(&self, sequence: u64) -> PathBuf {
        self.dir.join(format!("wal-{sequence:010}.sfst"))
    }

    /// Scan the directory for `.sfst` files and reconstruct state.
    pub fn recover(dir: &Path) -> Self {
        let mut registry = Self::new(dir.to_path_buf());

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return registry,
        };

        for dir_entry in entries.flatten() {
            let path = dir_entry.path();

            if path.extension().and_then(|e| e.to_str()) != Some("sfst") {
                continue;
            }

            let Some(id) = FileId::parse(&path) else {
                continue;
            };

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(_) => continue,
            };

            let size = ByteSize(meta.len());

            // Use the file's modification time as an approximation for
            // creation time. The actual WAL `created_at_ns` is not available
            // when the .bin has already been deleted.
            let created_at_ns = TimestampNs(
                meta.modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            );

            registry.files.insert(
                id.seq,
                IndexFile {
                    sequence: id.seq,
                    created_at_ns,
                    size,
                    pending_deletion: false,
                },
            );
        }

        registry
    }

    pub fn track(&mut self, sequence: u64, created_at_ns: TimestampNs, size: ByteSize) {
        self.files.insert(
            sequence,
            IndexFile {
                sequence,
                created_at_ns,
                size,
                pending_deletion: false,
            },
        );
    }

    pub fn remove(&mut self, sequence: u64) -> Option<IndexFile> {
        self.files.remove(&sequence)
    }

    pub fn mark_pending_deletion(&mut self, sequence: u64) {
        if let Some(entry) = self.files.get_mut(&sequence) {
            entry.pending_deletion = true;
        }
    }

    pub fn clear_pending_deletion(&mut self, sequence: u64) {
        if let Some(entry) = self.files.get_mut(&sequence) {
            entry.pending_deletion = false;
        }
    }

    pub fn get(&self, sequence: u64) -> Option<&IndexFile> {
        self.files.get(&sequence)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Evaluate the retention policy and return sequences of files to evict.
    ///
    /// Only files that are not already pending deletion are considered.
    /// Files are evaluated oldest-first (by sequence number). A file is
    /// marked for eviction if any limit is exceeded.
    pub fn evaluate_retention(
        &self,
        max_files: usize,
        max_total_size: u64,
        max_age_ns: u64,
        now_ns: u64,
    ) -> Vec<u64> {
        let eligible: Vec<&IndexFile> = self
            .files
            .values()
            .filter(|f| !f.pending_deletion)
            .collect();

        let total_files = eligible.len();
        let total_size: u64 = eligible.iter().map(|f| f.size.as_u64()).sum();

        let mut to_evict = Vec::new();
        let mut remaining_files = total_files;
        let mut remaining_size = total_size;

        for entry in &eligible {
            let mut should_evict = false;

            if remaining_files > max_files {
                should_evict = true;
            }
            if remaining_size > max_total_size {
                should_evict = true;
            }
            if now_ns.saturating_sub(entry.created_at_ns.as_u64()) > max_age_ns {
                should_evict = true;
            }

            if should_evict {
                to_evict.push(entry.sequence);
                remaining_files -= 1;
                remaining_size -= entry.size.as_u64();
            }
        }

        to_evict
    }
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

pub struct Registry {
    pub wal: WalRegistry,
    pub index: IndexRegistry,
}

impl Registry {
    /// Recover both registries from disk.
    ///
    /// Cleans up stale `.tmp` files (from interrupted index writes) before
    /// scanning.
    pub fn recover(wal_dir: &Path, index_dir: &Path) -> Self {
        cleanup_temp_files(wal_dir);
        if index_dir != wal_dir {
            cleanup_temp_files(index_dir);
        }

        let wal = WalRegistry::recover(wal_dir);
        let index = IndexRegistry::recover(index_dir);

        if !wal.is_empty() || !index.is_empty() {
            tracing::info!(
                "recovered files from disk: wal_files={} index_files={}",
                wal.len(),
                index.len(),
            );
        }

        Self { wal, index }
    }

    /// Returns FileIds of archived WAL files that have no corresponding index.
    pub fn unindexed_ids(&self) -> Vec<FileId> {
        self.wal
            .archived_ids()
            .into_iter()
            .filter(|id| self.index.get(id.seq).is_none())
            .collect()
    }
}

fn cleanup_temp_files(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for dir_entry in entries.flatten() {
        let path = dir_entry.path();
        if path.extension().is_some_and(|ext| ext == "tmp") {
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!("removed stale tmp file path={}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(
                        "failed to remove stale tmp file path={}: {e}",
                        path.display()
                    )
                }
            }
        }
    }
}
