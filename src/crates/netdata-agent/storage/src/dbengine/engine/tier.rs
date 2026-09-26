//! A tier while the engine runs (`struct rrdengine_instance` with its data files): the pairs with their writers and
//! journals, the v2 indexes, the open cache, and the counters. Queries read it; extent writes (`flush.rs`) append to
//! its last pair, create pairs and add open pages. Brief `knowledge/brief-dbengine-s3-commit3-map.md` in the status
//! repository; decisions D65 and D66.

use std::collections::{BTreeMap, HashMap, btree_map};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use super::io::{IoFile, write_retrying};
use super::load::{NEW_PAIR_SIZE, Tier, TierConfig, create_pair_files};
use super::v2index::V2Index;
use crate::dbengine::format::BLOCK_SIZE;

/// A page the open cache holds: a page of a journal not indexed yet (replayed, or written by this run).
#[derive(Debug, Clone, Copy)]
pub(crate) struct OpenPage {
    pub end_time_s: i64,
    pub update_every_s: u32,
    pub fileno: u32,
    /// The extent's first block and its size.
    pub block: u64,
    pub bytes: u32,
}

/// The open cache, by metric and start.
#[derive(Debug, Default)]
pub(crate) struct OpenList {
    by_metric: HashMap<[u8; 16], BTreeMap<i64, OpenPage>>,
}

impl OpenList {
    /// `pgc_open_add_hot_page()`: a page at a start already held replaces it only if it ends later.
    pub fn add(&mut self, uuid: [u8; 16], start_time_s: i64, page: OpenPage) {
        match self.by_metric.entry(uuid).or_default().entry(start_time_s) {
            btree_map::Entry::Vacant(v) => {
                v.insert(page);
            }
            btree_map::Entry::Occupied(mut o) if page.end_time_s > o.get().end_time_s => {
                o.insert(page);
            }
            btree_map::Entry::Occupied(_) => {}
        }
    }

    /// A metric's open pages by start.
    pub fn pages(&self, uuid: &[u8; 16]) -> Option<&BTreeMap<i64, OpenPage>> {
        self.by_metric.get(uuid)
    }
}

/// `datafile->writers` with `datafile->pos`.
#[derive(Debug)]
struct Writers {
    /// Where the next extent goes: the size, rounded up to a block.
    pos: u64,
    /// Extent writes that reserved room in the file and have not finished.
    running: usize,
    /// Finished writes still adding their pages to the open cache.
    flushed_to_open: usize,
    /// A write failed: the file takes no more extents.
    failed: bool,
}

/// A data file pair while the engine runs (`struct rrdengine_datafile` with its journal).
#[derive(Debug)]
pub struct DataFile {
    pub fileno: u32,
    pub(crate) file: IoFile,
    writers: Mutex<Writers>,
    /// The v1 journal open for writes: only for a reused last file and the pairs created in this run, the files
    /// without a v2 index.
    journal: Option<IoFile>,
    /// `journalfile->unsafe.pos`: taken by each transaction as it is written.
    journal_pos: AtomicU64,
}

impl DataFile {
    fn new(fileno: u32, file: IoFile, pos: u64, journal: Option<IoFile>, journal_pos: u64) -> DataFile {
        DataFile {
            fileno,
            file,
            writers: Mutex::new(Writers {
                pos,
                running: 0,
                flushed_to_open: 0,
                failed: false,
            }),
            journal,
            journal_pos: AtomicU64::new(journal_pos),
        }
    }

    fn writers(&self) -> MutexGuard<'_, Writers> {
        self.writers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `datafile->pos`.
    pub fn pos(&self) -> u64 {
        self.writers().pos
    }

    /// `journalfile->unsafe.pos`.
    pub fn journal_pos(&self) -> u64 {
        self.journal_pos.load(Ordering::Acquire)
    }

    /// The extent writes running on the file, and those still adding their pages to the open cache.
    pub fn writers_running(&self) -> (usize, usize) {
        let w = self.writers();
        (w.running, w.flushed_to_open)
    }

    /// `datafile_is_full()`: a failed file, one without a journal to write (a file with a v2 index, D65.6), or one
    /// where `size` more bytes pass the target.
    pub(crate) fn is_full(&self, size: u64, target: u64) -> bool {
        let w = self.writers();
        w.failed || self.journal.is_none() || w.pos + size > target
    }

    pub(crate) fn writer_started(&self) {
        self.writers().running += 1;
    }

    pub(crate) fn writer_finished(&self) {
        self.writers().running -= 1;
    }

    /// The write is done: its pages go to the open cache.
    pub(crate) fn writer_flushing_to_open(&self) {
        let mut w = self.writers();
        w.running -= 1;
        w.flushed_to_open += 1;
    }

    pub(crate) fn flushed_to_open(&self) {
        self.writers().flushed_to_open -= 1;
    }

    /// `datafile_mark_failed()`.
    pub(crate) fn mark_failed(&self) {
        self.writers().failed = true;
    }

    /// Room for `size` bytes at the file's position; where they go.
    pub(crate) fn advance(&self, size: u64) -> u64 {
        let mut w = self.writers();
        let at = w.pos;
        w.pos += size;
        at
    }

    /// `journalfile_v1_extent_write()`: the transaction's block taken at the journal's position (for good, even when
    /// the write fails), then written with C's retries.
    pub(crate) fn append_journal(&self, block: &[u8; BLOCK_SIZE]) -> io::Result<()> {
        let at = self
            .journal_pos
            .fetch_add(BLOCK_SIZE as u64, Ordering::AcqRel);
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
        write_retrying(journal, block, at)
    }
}

/// A tier as the engine runs it.
#[derive(Debug)]
pub struct TierData {
    pub config: TierConfig,
    /// The pairs by file number (`ctx->datafiles`).
    files: RwLock<BTreeMap<u32, Arc<DataFile>>>,
    /// `njfv2idx`: the serving v2 files by their end, then number.
    pub(crate) v2: BTreeMap<(i64, u32), V2Index>,
    open: RwLock<OpenList>,
    /// `ctx->atomic.last_fileno`: the newest file number, whose successor a new pair takes.
    last_fileno: AtomicU32,
    /// `ctx->atomic.last_flush_fileno`: the newest file an extent went to.
    last_flush_fileno: AtomicU32,
    /// `ctx->atomic.transaction_id`: the next transaction's id.
    transaction_id: AtomicU64,
    /// `ctx->atomic.first_time_s`.
    pub first_time_s: i64,
    /// `ctx->atomic.current_disk_space`.
    current_disk_space: AtomicU64,
    /// `ctx->atomic.samples`.
    samples: AtomicU64,
    /// `ctx->atomic.needs_indexing`: a file other than the last one has pages to index.
    needs_indexing: AtomicBool,
    /// `ctx->atomic.extents_currently_being_flushed`.
    extents_in_flight: AtomicUsize,
    /// `ctx->atomic.collectors_running`: the tier's collection handles.
    collectors_running: AtomicUsize,
    /// `ctx->quiesce.enabled`: the tier is shutting down, new queries get no preparation.
    quiesced: AtomicBool,
    /// `ctx->atomic.inflight_queries`: queries holding a preparation, which the tier's shutdown waits for.
    pub(crate) inflight: Arc<AtomicUsize>,
}

fn read<T>(l: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    l.read().unwrap_or_else(PoisonError::into_inner)
}

fn write<T>(l: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    l.write().unwrap_or_else(PoisonError::into_inner)
}

impl TierData {
    /// A tier after its startup and population.
    pub(crate) fn new(tier: Tier) -> TierData {
        let mut open = OpenList::default();
        for (fileno, p) in tier.open_pages {
            let page = OpenPage {
                end_time_s: p.end_time_s,
                update_every_s: p.update_every_s,
                fileno,
                block: p.block,
                bytes: p.extent_bytes,
            };
            open.add(p.uuid, p.start_time_s, page);
        }
        let files = tier
            .files
            .into_iter()
            .map(|p| {
                let file = DataFile::new(p.fileno, p.file, p.pos, p.journal, p.journal_pos);
                (p.fileno, Arc::new(file))
            })
            .collect();
        TierData {
            config: tier.config,
            files: RwLock::new(files),
            v2: tier
                .indexes
                .into_values()
                .map(|index| ((index.end_time_s(), index.fileno), index))
                .collect(),
            open: RwLock::new(open),
            last_fileno: AtomicU32::new(tier.last_fileno),
            last_flush_fileno: AtomicU32::new(0),
            transaction_id: AtomicU64::new(tier.transaction_id),
            first_time_s: tier.first_time_s,
            current_disk_space: AtomicU64::new(tier.current_disk_space),
            samples: AtomicU64::new(tier.samples),
            needs_indexing: AtomicBool::new(false),
            extents_in_flight: AtomicUsize::new(0),
            collectors_running: AtomicUsize::new(0),
            quiesced: AtomicBool::new(false),
            inflight: Arc::default(),
        }
    }

    pub fn tier(&self) -> usize {
        self.config.tier
    }

    /// A pair by its number, for extent reads.
    pub fn file(&self, fileno: u32) -> Option<Arc<DataFile>> {
        read(&self.files).get(&fileno).cloned()
    }

    pub fn has_file(&self, fileno: u32) -> bool {
        read(&self.files).contains_key(&fileno)
    }

    /// The pair extents are written to (a tier always has one).
    pub fn last_file(&self) -> Arc<DataFile> {
        read(&self.files)
            .values()
            .next_back()
            .cloned()
            .expect("a tier has a pair")
    }

    /// The open cache.
    pub(crate) fn open(&self) -> RwLockReadGuard<'_, OpenList> {
        read(&self.open)
    }

    pub(crate) fn open_mut(&self) -> RwLockWriteGuard<'_, OpenList> {
        write(&self.open)
    }

    /// `create_new_datafile_pair()` at run time, under the extent reserve: the pair after the newest file number,
    /// counted and listed. Whether it was created.
    pub(crate) fn create_pair(&self) -> bool {
        let fileno = self.last_fileno() + 1;
        let Some((file, journal)) = create_pair_files(&self.config, fileno) else {
            return false;
        };
        self.current_disk_space
            .fetch_add(NEW_PAIR_SIZE, Ordering::AcqRel);
        let pair = DataFile::new(
            fileno,
            file,
            BLOCK_SIZE as u64,
            Some(journal),
            BLOCK_SIZE as u64,
        );
        write(&self.files).insert(fileno, Arc::new(pair));
        self.last_fileno.store(fileno, Ordering::Release);
        true
    }

    pub fn last_fileno(&self) -> u32 {
        self.last_fileno.load(Ordering::Acquire)
    }

    pub fn last_flush_fileno(&self) -> u32 {
        self.last_flush_fileno.load(Ordering::Acquire)
    }

    pub(crate) fn flushed_to(&self, fileno: u32) {
        self.last_flush_fileno
            .fetch_max(fileno, Ordering::AcqRel);
    }

    /// A transaction id, taken for good.
    pub(crate) fn next_transaction_id(&self) -> u64 {
        self.transaction_id.fetch_add(1, Ordering::AcqRel)
    }

    pub fn current_disk_space(&self) -> u64 {
        self.current_disk_space.load(Ordering::Acquire)
    }

    pub(crate) fn add_disk_space(&self, bytes: u64) {
        self.current_disk_space
            .fetch_add(bytes, Ordering::AcqRel);
    }

    pub fn needs_indexing(&self) -> bool {
        self.needs_indexing.load(Ordering::Acquire)
    }

    pub(crate) fn set_needs_indexing(&self) {
        self.needs_indexing.store(true, Ordering::Release);
    }

    /// The extents being written.
    pub fn extents_in_flight(&self) -> usize {
        self.extents_in_flight.load(Ordering::Acquire)
    }

    pub(crate) fn extent_started(&self) {
        self.extents_in_flight.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn extent_finished(&self) {
        self.extents_in_flight.fetch_sub(1, Ordering::AcqRel);
    }

    /// The collection handles open on the tier.
    pub fn collectors_running(&self) -> usize {
        self.collectors_running.load(Ordering::Acquire)
    }

    pub(crate) fn collector_started(&self) {
        self.collectors_running.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn collector_finished(&self) {
        self.collectors_running.fetch_sub(1, Ordering::AcqRel);
    }

    /// The samples of the pages replayed, populated and closed.
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    pub(crate) fn add_samples(&self, samples: u64) {
        self.samples.fetch_add(samples, Ordering::Relaxed);
    }

    /// `RRDENG_OPCODE_CTX_QUIESCE`: queries started from now on read nothing, and written extents no longer reach
    /// the open cache.
    pub fn quiesce(&self) {
        self.quiesced.store(true, Ordering::Release);
    }

    pub fn quiesced(&self) -> bool {
        self.quiesced.load(Ordering::Acquire)
    }

    /// The queries in flight.
    pub fn inflight(&self) -> usize {
        self.inflight.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests;
