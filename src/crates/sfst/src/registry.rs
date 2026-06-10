use std::ops::Range;
use std::path::{Path, PathBuf};

use file_registry::{ByteSize, FileDir, FileId, FileRegistry, Query};

use crate::Summary;

pub(crate) const SFST_EXT: &str = "sfst";

#[derive(Debug, Clone)]
pub struct File {
    pub id: FileId,
    pub size: ByteSize,
    /// Cheap summary fields lifted off the SFST file's `SUMR` chunk. Stored
    /// inline so the query planner and catalog builder can read them without
    /// opening the file.
    pub summary: Summary,
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

    pub fn track(&mut self, id: FileId, size: ByteSize, summary: Summary) {
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
    /// design (each SFST holds exactly one stream; see [`crate::ServiceStream`]).
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
fn range_overlaps(summary: &Summary, q: &Range<u32>) -> bool {
    if q.start >= q.end {
        return false;
    }
    summary.max_timestamp_s >= q.start && summary.min_timestamp_s < q.end
}

/// Read the `SUMR` chunk of an SFST file and decode the summary.
///
/// Used by [`Registry::recover`] to rebuild summaries on startup. Maps the
/// file instead of reading it: `Reader::open` touches only the header + TOC
/// pages and `summary()` only the SUMR chunk's, so recovery faults in a few
/// KB per file rather than the whole file — which, across thousands of
/// files, turned startup into a multi-GB sequential read. `Advice::Random`
/// suppresses readahead so the kernel doesn't speculatively pull
/// neighbouring pages either.
fn read_summary(path: &Path) -> Result<Summary, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    // SAFETY: recovery runs before the indexer and cleaner are spawned, so
    // the file is not concurrently truncated or rewritten while mapped.
    let mmap = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| format!("mmap: {e}"))?;
    // `madvise` is a POSIX API; memmap2 only exposes `advise`/`Advice` on Unix.
    #[cfg(unix)]
    let _ = mmap.advise(memmap2::Advice::Random);
    let reader = crate::Reader::open(&mmap).map_err(|e| format!("parse: {e}"))?;
    reader.summary().map_err(|e| format!("summary: {e}"))
}

#[cfg(test)]
mod tests;
