//! The journal v2 files of a tier in use (`njfv2idx` and each journal's v2 data in C): a file's header and extent list
//! in memory, a sparse index of its metric list (every 256th UUID) for lookups, and the retention population that
//! feeds the metric registry from each file (`journalfile_v2_populate_retention_to_mrg()`, `rrdeng_populate_mrg()`).
//! Brief `knowledge/brief-dbengine-s2.md` §1.5 and §4.5 in the status repository.
//!
//! A file whose metric list is out of bounds or fails its CRC (checked once, here, when the load did not check it) is
//! unavailable for good: it stays on disk, as C leaves it, and serves nothing (D29).

use std::fs::File;
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{
    ErrorLimit, Priority, Source, nd_log, nd_log_limit, netdata_log_error, netdata_log_info,
};

use super::load::Tier;
use super::mrg::Mrg;
use crate::dbengine::format::crc::{crc_bytes, crc32, crc32_update};
use crate::dbengine::format::journal_v2::{
    self, EXTENT_SIZE, ExtentEntry, HEADER_SIZE, Header, METRIC_SIZE, MetricEntry,
    PAGE_HEADER_SIZE, PAGE_SIZE, PageEntry, PageHeader, TRAILER_SIZE,
};
use crate::dbengine::format::{FileKind, ReadAt, file_name};

/// Metric list entries between two samples of the sparse index.
const SPARSE_EVERY: usize = 256;
/// Metric list entries read at a time: about 1 MiB.
const CHUNK_ENTRIES: usize = 1024 * 1024 / METRIC_SIZE;

/// A v2 file ready for lookups.
#[derive(Debug)]
pub struct V2Index {
    pub fileno: u32,
    pub file: File,
    pub size: u64,
    pub header: Header,
    pub extents: Vec<ExtentEntry>,
    /// Every 256th metric list UUID with its position, in the list's (UUID) order.
    sparse: Vec<([u8; 16], u32)>,
}

impl V2Index {
    /// The header's start in seconds, the base of every delta in the file.
    pub fn start_time_s(&self) -> i64 {
        (self.header.start_time_ut / 1_000_000) as i64
    }

    /// The newest point of the file (the header's end), which orders the files of a tier.
    pub fn end_time_s(&self) -> i64 {
        (self.header.end_time_ut / 1_000_000) as i64
    }

    fn metric_at(&self, index: u32) -> io::Result<MetricEntry> {
        let mut b = [0u8; METRIC_SIZE];
        let at = u64::from(self.header.metric_offset) + u64::from(index) * METRIC_SIZE as u64;
        ReadAt::read_exact_at(&self.file, &mut b, at)?;
        Ok(MetricEntry::decode(&b))
    }

    /// A metric's entry, found by a binary search of the sparse index and then of one stretch of the list.
    pub fn find(&self, uuid: &[u8; 16]) -> io::Result<Option<MetricEntry>> {
        let count = self.header.metric_count;
        if count == 0 {
            return Ok(None);
        }
        let from = match self.sparse.binary_search_by(|(u, _)| u.cmp(uuid)) {
            Ok(i) => return self.metric_at(self.sparse[i].1).map(Some),
            Err(0) => return Ok(None),
            Err(i) => self.sparse[i - 1].1,
        };
        let to = (from as usize + SPARSE_EVERY).min(count as usize) as u32;
        let n = (to - from) as usize;
        let mut b = vec![0u8; n * METRIC_SIZE];
        let at = u64::from(self.header.metric_offset) + u64::from(from) * METRIC_SIZE as u64;
        ReadAt::read_exact_at(&self.file, &mut b, at)?;
        let entries: Vec<MetricEntry> = journal_v2::metrics(&b).collect();
        Ok(entries
            .binary_search_by(|m| m.uuid.cmp(uuid))
            .ok()
            .map(|i| entries[i]))
    }

    /// A metric's pages, when its page header matches its entry and its list's CRC holds.
    pub fn pages(&self, metric: &MetricEntry) -> io::Result<Option<Vec<PageEntry>>> {
        let mut hb = [0u8; PAGE_HEADER_SIZE];
        ReadAt::read_exact_at(&self.file, &mut hb, u64::from(metric.page_offset))?;
        let ph = PageHeader::decode(&hb);
        if ph.crc != ph.compute_crc() || ph.uuid != metric.uuid || ph.entries != metric.entries {
            return Ok(None);
        }
        let len = metric.entries as usize * PAGE_SIZE;
        let mut list = vec![0u8; len + TRAILER_SIZE];
        let at = u64::from(metric.page_offset) + PAGE_HEADER_SIZE as u64;
        ReadAt::read_exact_at(&self.file, &mut list, at)?;
        let (entries, trailer) = list.split_at(len);
        if trailer != crc_bytes(crc32(entries)) {
            return Ok(None);
        }
        Ok(Some(journal_v2::pages(entries).collect()))
    }
}

/// What one file's population produced.
struct Populated {
    fileno: u32,
    index: Option<V2Index>,
    /// The header's start, a candidate for the tier's first time (`global_first_time_s`).
    first_time_s: i64,
}

/// Reads the metric list in chunks, handing each entry to `f`; the CRC of the whole list.
fn walk_metric_list(
    file: &File,
    h: &Header,
    mut f: impl FnMut(u32, &MetricEntry),
) -> io::Result<u32> {
    let mut crc = 0u32;
    let mut index = 0u32;
    let count = h.metric_count as usize;
    let mut buf = Vec::new();
    while (index as usize) < count {
        let n = (count - index as usize).min(CHUNK_ENTRIES);
        buf.resize(n * METRIC_SIZE, 0);
        let at = u64::from(h.metric_offset) + u64::from(index) * METRIC_SIZE as u64;
        ReadAt::read_exact_at(file, &mut buf, at)?;
        crc = crc32_update(crc, &buf);
        for m in journal_v2::metrics(&buf) {
            f(index, &m);
            index += 1;
        }
    }
    Ok(crc)
}

/// `journalfile_v2_populate_retention_to_mrg()` for one file: the metric list's bounds, its CRC once when the load did
/// not check it, then every metric's retention into the registry (`now_s` + 1 is the newest acceptable time), with
/// C's records. The file's index when it serves, `None` when it is unavailable for good.
/// One file's population job.
struct Job {
    tier: usize,
    path: std::path::PathBuf,
    fileno: u32,
    file: File,
    size: u64,
    /// The load did not check the metric list (`JOURNALFILE_FLAG_METRIC_CRC_CHECK`).
    crc_check: bool,
}

fn populate_file(job: Job, mrg: &Mrg, now_s: i64) -> Populated {
    let Job {
        tier,
        path,
        fileno,
        file,
        size,
        crc_check,
    } = job;
    let started = Instant::now();
    let fail = |first_time_s| Populated {
        fileno,
        index: None,
        first_time_s,
    };
    let mut hb = [0u8; HEADER_SIZE];
    if ReadAt::read_exact_at(&file, &mut hb, 0).is_err() {
        return fail(0);
    }
    let h = Header::decode(&hb);
    if !journal_v2::metric_list_in_bounds(&h, size) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "DBENGINE: journal v2 \"{}\" has out-of-range header offsets (metric_offset={}, metric_count={}, \
             metric_trailer_offset={}, mmap_size={size}); marking unavailable for rebuild",
            path.display(),
            h.metric_offset,
            h.metric_count,
            h.metric_trailer_offset
        );
        return fail(0);
    }
    if crc_check {
        let crc = walk_metric_list(&file, &h, |_, _| {});
        let mut stored = [0u8; TRAILER_SIZE];
        let ok = crc.is_ok_and(|crc| {
            ReadAt::read_exact_at(&file, &mut stored, u64::from(h.metric_trailer_offset)).is_ok()
                && stored == crc_bytes(crc)
        });
        if !ok {
            netdata_log_error!("DBENGINE: metric list CRC32 check: FAILED");
            return fail(0);
        }
    }
    let base = (h.start_time_ut / 1_000_000) as i64;
    let now = now_s + 1;
    let mut sparse = Vec::with_capacity(h.metric_count as usize / SPARSE_EVERY + 1);
    let walked = walk_metric_list(&file, &h, |index, m| {
        if (index as usize).is_multiple_of(SPARSE_EVERY) {
            sparse.push((m.uuid, index));
        }
        mrg.update_retention_by_uuid(
            &m.uuid,
            tier,
            base + i64::from(m.delta_start_s),
            base + i64::from(m.delta_end_s),
            m.update_every_s,
            now,
        );
    });
    if walked.is_err() {
        return fail(0);
    }
    let mut extents = vec![0u8; h.extent_count as usize * EXTENT_SIZE];
    if ReadAt::read_exact_at(&file, &mut extents, u64::from(h.extent_offset)).is_err() {
        return fail(0);
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: journal v2 of tier {tier}, datafile {fileno} populated, size: {:.2} MiB, metrics: {:.2} k, {:.2} ms",
        size as f64 / 1024.0 / 1024.0,
        f64::from(h.metric_count) / 1000.0,
        started.elapsed().as_secs_f64() * 1000.0
    );
    Populated {
        fileno,
        first_time_s: base,
        index: Some(V2Index {
            fileno,
            file,
            size,
            header: h,
            extents: journal_v2::extents(&extents).collect(),
            sparse,
        }),
    }
}

thread_local! {
    /// `nd_log_limit_static_thread_var(erl, 10, 0)` of each progress record.
    static PROGRESS: ErrorLimit = const { ErrorLimit::new(10, 0) };
    static WAITING: ErrorLimit = const { ErrorLimit::new(10, 0) };
}

/// `rrdeng_populate_mrg()` and `populate_mrg_tp_worker()`: the tier's record, then each data file's population as a
/// job on the pool, at most `cpus` at a time, with C's rate-limited progress records while they run. Files without a
/// v2 contribute nothing. The indexes land in the tier, and the headers' earliest start lowers its first time.
pub fn populate(tier: &mut Tier, mrg: &Mrg, pool: &WorkPool, cpus: usize, now_s: i64) {
    let cpus = cpus.max(1);
    let t = tier.config.tier;
    netdata_log_info!(
        "DBENGINE: tier {t}: populating retention to MRG from {} journal files, using a shared pool of {cpus} \
         threads...",
        tier.files.len()
    );
    let total_files = tier.files.len();
    if total_files == 0 {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "DBENGINE: tier {t}: no datafiles to populate MRG"
        );
        return;
    }
    let completed = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::channel::<Populated>();
    let crc_check = !tier.config.journal_check;
    let mut results = Vec::new();
    let mut outstanding = 0usize;
    for df in &tier.files {
        // the shared semaphore of `cpus` slots
        if outstanding == cpus {
            if let Ok(done) = rx.recv() {
                results.push(done);
                outstanding -= 1;
            }
        }
        let fileno = df.fileno;
        let path = tier
            .config
            .path
            .join(file_name(FileKind::JournalV2, 1, fileno));
        let file_job = df.v2.as_ref().and_then(|v2| {
            Some(Job {
                tier: t,
                path,
                fileno,
                file: v2.file.try_clone().ok()?,
                size: v2.size,
                crc_check,
            })
        });
        let (tx, mrg, job_completed) = (tx.clone(), mrg.clone(), Arc::clone(&completed));
        let job = move || {
            let done = match file_job {
                Some(file_job) => populate_file(file_job, &mrg, now_s),
                None => Populated {
                    fileno,
                    index: None,
                    first_time_s: 0,
                },
            };
            job_completed.fetch_add(1, Ordering::AcqRel);
            let _ = tx.send(done);
        };
        if pool.queue(job).is_err() {
            continue;
        }
        outstanding += 1;
        let done = completed.load(Ordering::Acquire);
        PROGRESS.with(|erl| {
            nd_log_limit!(
                erl,
                Source::Daemon,
                Priority::Info,
                "DBENGINE: tier {t}: MRG population completed: {:.2}% ({done}/{total_files})",
                done as f64 * 100.0 / total_files as f64
            );
        });
    }
    drop(tx);
    while outstanding > 0 {
        let done = completed.load(Ordering::Acquire);
        WAITING.with(|erl| {
            nd_log_limit!(erl, Source::Daemon, Priority::Info,
                "DBENGINE: tier {t}: MRG population completed: {:.2}% ({done}/{total_files}), waiting for \
                 {outstanding} workers",
                done as f64 * 100.0 / total_files as f64);
        });
        match rx.recv() {
            Ok(result) => {
                results.push(result);
                outstanding -= 1;
            }
            Err(_) => break,
        }
    }
    for result in results {
        if result.first_time_s > 0 && result.first_time_s < tier.first_time_s {
            tier.first_time_s = result.first_time_s;
        }
        if let Some(index) = result.index {
            tier.indexes.insert(result.fileno, index);
        }
    }
}

/// `rrdeng_readiness_wait()` once the population ended: a tier with no retention starts now, with C's record.
pub fn readiness(tier: &mut Tier, now_s: i64) {
    if tier.first_time_s == i64::MAX {
        tier.first_time_s = now_s;
    }
    netdata_log_info!(
        "DBENGINE: tier {}: ready for data collection and queries",
        tier.config.tier
    );
}

#[cfg(test)]
mod tests;
