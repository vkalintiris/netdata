use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use file_registry::{ByteSize, FileDir, FileId, FileRegistry, TimestampNs};

use crate::FileSummary;

const SFST_EXT: &str = "sfst";

#[derive(Debug, Clone)]
pub struct File {
    pub id: FileId,
    pub created_at_ns: TimestampNs,
    pub size: ByteSize,
    /// Cheap summary fields lifted off the SFST file's `SUMR` chunk. Stored
    /// inline so the query planner and catalog builder can read them without
    /// opening the file.
    pub summary: FileSummary,
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
    ///
    /// Reads each file's `SUMR` chunk to recover the summary fields; files
    /// whose summary cannot be read are skipped with a warning rather than
    /// aborting recovery. Returns the number of files successfully recovered.
    pub fn recover(&mut self) -> usize {
        let scan_results = self.inner.dir().scan().unwrap_or_default();
        let dir = self.inner.dir().path().to_path_buf();
        let mut recovered = 0usize;

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

            let path = dir.join(id.to_filename(SFST_EXT));
            let summary = match read_summary(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(
                        "skipping sfst file during recovery path={} error={}",
                        path.display(),
                        e,
                    );
                    continue;
                }
            };

            self.inner.insert(
                id.seq,
                File {
                    id,
                    created_at_ns,
                    size,
                    summary,
                    pending_deletion: false,
                },
            );
            recovered += 1;
        }

        recovered
    }

    pub fn track(
        &mut self,
        id: FileId,
        created_at_ns: TimestampNs,
        size: ByteSize,
        summary: FileSummary,
    ) {
        self.inner.insert(
            id.seq,
            File {
                id,
                created_at_ns,
                size,
                summary,
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

/// Read the `SUMR` chunk of an SFST file and decode the summary.
///
/// Used by [`Registry::recover`] to rebuild summaries on startup. Reads the
/// whole file into memory, which is fine for the small SFSTs typical for
/// log indexes; switch to mmap if recovery becomes I/O-bound.
fn read_summary(path: &Path) -> Result<FileSummary, String> {
    let data = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
    let reader = crate::Reader::open(&data).map_err(|e| format!("open: {e}"))?;
    reader.summary().map_err(|e| format!("summary: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{StreamEntry, Writer, pack, pack_metadata};
    use fst_index::FstIndex;

    fn write_sfst_with_summary(dir: &Path, id: FileId, summary: &FileSummary) {
        let primary: FstIndex<u64> = FstIndex::build([("k", 1u64)]).unwrap();
        let mut writer = Writer::new();
        writer.set_summary(pack_metadata(summary, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();
        let path = dir.join(id.to_filename(SFST_EXT));
        std::fs::write(&path, &buf).unwrap();
    }

    #[test]
    fn recover_rebuilds_summary_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let id1 = FileId::new(uuid::Uuid::nil(), uuid::Uuid::from_u128(1), 1, 7);
        let id2 = FileId::new(uuid::Uuid::nil(), uuid::Uuid::from_u128(1), 2, 7);

        let s1 = FileSummary {
            min_timestamp_s: 100,
            max_timestamp_s: 200,
            total_logs: 50,
            streams: vec![StreamEntry::new("ns", "a", 50)],
        };
        let s2 = FileSummary {
            min_timestamp_s: 300,
            max_timestamp_s: 400,
            total_logs: 25,
            streams: vec![StreamEntry::new("ns", "b", 25)],
        };
        write_sfst_with_summary(dir.path(), id1, &s1);
        write_sfst_with_summary(dir.path(), id2, &s2);

        let mut reg = Registry::new(dir.path());
        let n = reg.recover();
        assert_eq!(n, 2);
        assert_eq!(reg.get(1).unwrap().summary, s1);
        assert_eq!(reg.get(2).unwrap().summary, s2);
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn recover_skips_unreadable_files() {
        let dir = tempfile::tempdir().unwrap();
        let id_good = FileId::new(uuid::Uuid::nil(), uuid::Uuid::from_u128(1), 1, 7);
        let id_bad = FileId::new(uuid::Uuid::nil(), uuid::Uuid::from_u128(1), 2, 7);
        let s = FileSummary {
            min_timestamp_s: 1,
            max_timestamp_s: 2,
            total_logs: 1,
            streams: vec![],
        };
        write_sfst_with_summary(dir.path(), id_good, &s);
        // Garbage file with the right extension/name shape but invalid contents.
        std::fs::write(dir.path().join(id_bad.to_filename(SFST_EXT)), b"junk").unwrap();

        let mut reg = Registry::new(dir.path());
        let n = reg.recover();
        assert_eq!(n, 1);
        assert!(reg.get(1).is_some());
        assert!(reg.get(2).is_none());
    }

    #[test]
    fn track_sets_summary() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        let id = FileId::new(uuid::Uuid::nil(), uuid::Uuid::from_u128(1), 5, 7);
        let summary = FileSummary {
            min_timestamp_s: 1,
            max_timestamp_s: 9,
            total_logs: 7,
            streams: vec![StreamEntry::new("a", "b", 7)],
        };
        reg.track(id, TimestampNs(0), ByteSize(1), summary.clone());
        assert_eq!(reg.get(5).unwrap().summary, summary);
    }
}
