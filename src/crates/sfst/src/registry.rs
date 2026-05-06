use std::ops::Range;
use std::path::{Path, PathBuf};

use file_registry::{ByteSize, FileDir, FileId, FileRegistry, Query};

use crate::FileSummary;

const SFST_EXT: &str = "sfst";

#[derive(Debug, Clone)]
pub struct File {
    pub id: FileId,
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
                    size,
                    summary,
                    pending_deletion: false,
                },
            );
            recovered += 1;
        }

        recovered
    }

    pub fn track(&mut self, id: FileId, size: ByteSize, summary: FileSummary) {
        self.inner.insert(
            id.seq,
            File {
                id,
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

    /// Files in the registry whose summary intersects `q`.
    ///
    /// Pure filter — does not open any SFST file. Excludes entries marked
    /// `pending_deletion` so callers don't see files that are queued for
    /// removal by the cleaner.
    ///
    /// Time-range overlap is computed against the file's full
    /// `[min_timestamp_s, max_timestamp_s]` range (inclusive on both ends);
    /// the query's `time_range` is `[start, end)` (half-open). A file is
    /// included if any second is shared by both ranges.
    ///
    /// Stream filter, when present, is exact equality on
    /// `(namespace, name)` — there is no partial / prefix matching, by
    /// design (each SFST holds exactly one stream; see [`StreamEntry`]).
    pub fn candidates<'a>(&'a self, q: &Query) -> impl Iterator<Item = &'a File> + 'a {
        // Extract q's contents upfront so the filter closures don't borrow
        // q. This decouples the iterator's lifetime from q's, letting
        // callers pass a temporary `Query` without binding it to a local.
        let q_range = q.time_range.clone();
        let q_stream = q.stream.clone();
        self.inner
            .values()
            .filter(|f| !f.pending_deletion)
            .filter(move |f| range_overlaps(&f.summary, &q_range))
            .filter(move |f| q_stream.as_ref().is_none_or(|s| &f.summary.stream == s))
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
    ///
    /// Age is measured against `summary.max_timestamp_s` — the most recent
    /// log entry in the file. An empty SFST (`total_logs == 0`,
    /// `max_timestamp_s == 0`) ages out immediately, which matches the
    /// "no useful data" disposition.
    pub fn evaluate_retention(
        &self,
        retention: &bridge::config::RetentionConfig,
        now_ns: u64,
    ) -> Vec<u64> {
        let max_files = retention.max_files;
        let max_total_size = retention.max_total_size.as_u64();
        let max_age_s = retention.max_age.as_secs();
        let now_s = (now_ns / 1_000_000_000) as u64;

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
            if now_s.saturating_sub(entry.summary.max_timestamp_s as u64) > max_age_s {
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

/// True iff the file's `[min, max]` second range shares any second with
/// the query's half-open `[start, end)` range.
///
/// Edge cases:
/// - Empty SFSTs (`total_logs == 0`, `min == max == 0`) overlap with any
///   query that includes second 0; in practice they're filtered earlier
///   by retention.
/// - A query with `start == end` is empty and matches no file.
fn range_overlaps(summary: &FileSummary, q: &Range<u32>) -> bool {
    if q.start >= q.end {
        return false;
    }
    summary.max_timestamp_s >= q.start && summary.min_timestamp_s < q.end
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
            stream: StreamEntry::new("ns", "a"),
        };
        let s2 = FileSummary {
            min_timestamp_s: 300,
            max_timestamp_s: 400,
            total_logs: 25,
            stream: StreamEntry::new("ns", "b"),
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
            stream: StreamEntry::new("", ""),
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
            stream: StreamEntry::new("a", "b"),
        };
        reg.track(id, ByteSize(1), summary.clone());
        assert_eq!(reg.get(5).unwrap().summary, summary);
    }

    // ── Candidate selection tests ───────────────────────────────────

    fn fid(seq: u64) -> FileId {
        FileId::new(uuid::Uuid::nil(), uuid::Uuid::from_u128(1), seq, 0)
    }

    fn populate(
        reg: &mut Registry,
        entries: &[(u64, u32, u32, &str, &str)], // (seq, min_s, max_s, ns, name)
    ) {
        for &(seq, min_s, max_s, ns, name) in entries {
            reg.track(
                fid(seq),
                ByteSize(1),
                FileSummary {
                    min_timestamp_s: min_s,
                    max_timestamp_s: max_s,
                    total_logs: 1,
                    stream: StreamEntry::new(ns, name),
                },
            );
        }
    }

    fn seqs<'a>(iter: impl Iterator<Item = &'a File>) -> Vec<u64> {
        let mut v: Vec<u64> = iter.map(|f| f.id.seq).collect();
        v.sort();
        v
    }

    #[test]
    fn candidates_filter_by_time_range_overlap() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(
            &mut reg,
            &[
                (1, 100, 200, "ns", "a"),
                (2, 300, 400, "ns", "a"),
                (3, 150, 350, "ns", "a"),
            ],
        );

        // Window [50, 250) covers files 1 and 3.
        let q = Query {
            time_range: 50..250,
            stream: None,
        };
        assert_eq!(seqs(reg.candidates(&q)), vec![1, 3]);

        // Window [500, 600) covers nothing.
        let q = Query {
            time_range: 500..600,
            stream: None,
        };
        assert_eq!(seqs(reg.candidates(&q)), Vec::<u64>::new());
    }

    #[test]
    fn candidates_inclusive_lower_exclusive_upper() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(
            &mut reg,
            &[
                (1, 100, 200, "ns", "a"),
                (2, 200, 300, "ns", "a"),
                (3, 300, 400, "ns", "a"),
            ],
        );

        // Query [200, 300) — touches file 1's max (200, inclusive),
        // touches file 2's min (200, inclusive), does NOT touch file 3
        // because q.end=300 is exclusive and file 3's min is 300.
        let q = Query {
            time_range: 200..300,
            stream: None,
        };
        assert_eq!(seqs(reg.candidates(&q)), vec![1, 2]);
    }

    #[test]
    fn candidates_single_point_query() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(
            &mut reg,
            &[
                (1, 100, 200, "ns", "a"),
                (2, 150, 250, "ns", "a"),
                (3, 300, 400, "ns", "a"),
            ],
        );

        // [150, 151) hits file 1 (max=200 ≥ 150, min=100 < 151) and file 2
        // (max=250 ≥ 150, min=150 < 151), but not file 3.
        let q = Query {
            time_range: 150..151,
            stream: None,
        };
        assert_eq!(seqs(reg.candidates(&q)), vec![1, 2]);
    }

    #[test]
    fn candidates_empty_query_matches_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(&mut reg, &[(1, 100, 200, "ns", "a")]);

        // start == end is an empty window.
        let q = Query {
            time_range: 200..200,
            stream: None,
        };
        assert!(reg.candidates(&q).next().is_none());
    }

    #[test]
    fn candidates_filter_by_stream() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(
            &mut reg,
            &[
                (1, 100, 200, "prod", "api"),
                (2, 100, 200, "prod", "worker"),
                (3, 100, 200, "staging", "api"),
            ],
        );

        let q = Query {
            time_range: 0..u32::MAX,
            stream: Some(StreamEntry::new("prod", "api")),
        };
        assert_eq!(seqs(reg.candidates(&q)), vec![1]);
    }

    #[test]
    fn candidates_no_stream_filter_returns_all_in_range() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(
            &mut reg,
            &[
                (1, 100, 200, "prod", "api"),
                (2, 100, 200, "prod", "worker"),
                (3, 100, 200, "staging", "api"),
            ],
        );

        let q = Query {
            time_range: 0..u32::MAX,
            stream: None,
        };
        assert_eq!(seqs(reg.candidates(&q)), vec![1, 2, 3]);
    }

    #[test]
    fn candidates_skip_pending_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let mut reg = Registry::new(dir.path());
        populate(
            &mut reg,
            &[
                (1, 100, 200, "ns", "a"),
                (2, 100, 200, "ns", "a"),
                (3, 100, 200, "ns", "a"),
            ],
        );
        reg.mark_pending_deletion(2);

        let q = Query {
            time_range: 0..u32::MAX,
            stream: None,
        };
        assert_eq!(seqs(reg.candidates(&q)), vec![1, 3]);
    }

    #[test]
    fn candidates_on_empty_registry() {
        let dir = tempfile::tempdir().unwrap();
        let reg = Registry::new(dir.path());
        let q = Query {
            time_range: 0..u32::MAX,
            stream: None,
        };
        assert!(reg.candidates(&q).next().is_none());
    }
}
