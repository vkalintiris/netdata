use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use file_registry::{ByteSize, FileDir, FileId, FileRegistry, TimestampNs};

const SFST_EXT: &str = "sfst";

#[derive(Debug, Clone)]
pub struct File {
    pub id: FileId,
    pub created_at_ns: TimestampNs,
    pub size: ByteSize,
    pending_deletion: bool,
}

pub struct Registry {
    inner: FileRegistry<File>,
}

impl Registry {
    pub fn new(dir: &Path) -> Self {
        Self {
            inner: FileRegistry::new(FileDir::new(dir, SFST_EXT)),
        }
    }

    pub fn dir(&self) -> &Path {
        self.inner.dir().path()
    }

    /// Derive the on-disk path for an index file from its FileId.
    pub fn file_path(&self, id: FileId) -> PathBuf {
        self.inner.file_path(id)
    }

    /// Scan the directory for `.sfst` files and reconstruct state.
    pub fn recover(&mut self) {
        let scan_results = self.inner.dir().scan().unwrap_or_default();

        for (id, meta) in scan_results {
            let size = ByteSize(meta.len());

            // Use the file's modification time as an approximation for
            // creation time. The actual WAL `created_at_ns` is not available
            // when the .wal has already been deleted.
            let created_at_ns = TimestampNs(
                meta.modified()
                    .unwrap_or(SystemTime::UNIX_EPOCH)
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos() as u64,
            );

            self.inner.insert(
                id.seq,
                File {
                    id,
                    created_at_ns,
                    size,
                    pending_deletion: false,
                },
            );
        }
    }

    pub fn track(&mut self, id: FileId, created_at_ns: TimestampNs, size: ByteSize) {
        self.inner.insert(
            id.seq,
            File {
                id,
                created_at_ns,
                size,
                pending_deletion: false,
            },
        );
    }

    pub fn remove(&mut self, seq: u64) -> Option<File> {
        self.inner.remove(seq)
    }

    pub fn mark_pending_deletion(&mut self, seq: u64) {
        if let Some(entry) = self.inner.get_mut(seq) {
            entry.pending_deletion = true;
        }
    }

    pub fn clear_pending_deletion(&mut self, seq: u64) {
        if let Some(entry) = self.inner.get_mut(seq) {
            entry.pending_deletion = false;
        }
    }

    pub fn get(&self, seq: u64) -> Option<&File> {
        self.inner.get(seq)
    }

    pub fn values(&self) -> impl Iterator<Item = &File> {
        self.inner.values()
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Evaluate the retention policy and return sequences of files to evict.
    ///
    /// Only files that are not already pending deletion are considered.
    /// Files are evaluated oldest-first (by sequence number). A file is
    /// marked for eviction if any limit is exceeded.
    pub fn evaluate_retention(
        &self,
        retention: &bridge::config::RetentionConfig,
        now_ns: u64,
    ) -> Vec<u64> {
        let max_files = retention.max_files;
        let max_total_size = retention.max_total_size.as_u64();
        let max_age_ns = retention.max_age.as_nanos() as u64;

        let eligible: Vec<&File> = self
            .inner
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
                to_evict.push(entry.id.seq);
                remaining_files -= 1;
                remaining_size -= entry.size.as_u64();
            }
        }

        to_evict
    }
}
