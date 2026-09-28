//! A tier while the engine runs (`struct rrdengine_instance` with its data files): the pairs with their writers and
//! journals, the v2 indexes, the open cache, and the counters. Queries read it; extent writes (`flush.rs`) append to
//! its last pair, create pairs and add open pages. Brief `knowledge/brief-dbengine-s3-commit3-map.md` in the status
//! repository; decisions D65 and D66.

use std::collections::{BTreeMap, HashMap, btree_map};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log_limit, netdata_log_info};

use super::io::{IoFile, write_retrying};
use super::load::{NEW_PAIR_SIZE, Tier, TierConfig, create_pair_files};
use super::v2index::V2Index;
use crate::dbengine::format::BLOCK_SIZE;
use crate::dbengine::format::journal_v2::Page;

/// The indexer's attempts at taking a file, and the pause between them (none in tests).
const INDEXING_ATTEMPTS: u32 = 5;

fn indexing_pause() {
    #[cfg(not(test))]
    std::thread::sleep(std::time::Duration::from_millis(200));
}

/// `rrdeng_atomic_uint64_sub_saturating()`: `value` taken from a tier's `counter`. An underflow is recorded, rate
/// limited by `limit` (C's static of the calling file), and leaves 0.
pub(crate) fn sub_saturating(
    counter: &AtomicU64,
    value: u64,
    tier: usize,
    name: &str,
    reason: &str,
    limit: &ErrorLimit,
) {
    if value == 0 {
        return;
    }
    let mut old = counter.load(Ordering::Relaxed);
    loop {
        let new = if old < value {
            nd_log_limit!(
                limit,
                Source::Daemon,
                Priority::Err,
                "DBENGINE: tier {tier}: {name} counter underflow while {reason} (current={old}, subtract={value}); \
                 saturating to zero"
            );
            0
        } else {
            old - value
        };
        match counter.compare_exchange(old, new, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return,
            Err(now) => old = now,
        }
    }
}

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

/// Where a page joined the open cache within its file: replayed pages at `(0, i)` in replay order, then written ones
/// at `(journal position, i)` in extent order (D65.2, D67.6): the order C's indexer walks them.
pub(crate) type OpenKey = (u64, u32);

/// A file's open page as its list keeps it: metric, start, and the extent's block (to tell it from a replacement).
type FilePage = ([u8; 16], i64, u64);

/// The open cache, by metric and start, and each file's pages in the order they joined.
#[derive(Debug, Default)]
pub(crate) struct OpenList {
    by_metric: HashMap<[u8; 16], BTreeMap<i64, OpenPage>>,
    by_file: BTreeMap<u32, BTreeMap<OpenKey, FilePage>>,
}

impl OpenList {
    /// `pgc_open_add_hot_page()`: a page at a start already held replaces it only if it ends later (the replaced one
    /// leaves its file's list, as C evicts it).
    pub fn add(&mut self, uuid: [u8; 16], start_time_s: i64, page: OpenPage, key: OpenKey) {
        match self.by_metric.entry(uuid).or_default().entry(start_time_s) {
            btree_map::Entry::Vacant(v) => {
                v.insert(page);
            }
            btree_map::Entry::Occupied(mut o) if page.end_time_s > o.get().end_time_s => {
                o.insert(page);
            }
            btree_map::Entry::Occupied(_) => return,
        }
        self.by_file
            .entry(page.fileno)
            .or_default()
            .insert(key, (uuid, start_time_s, page.block));
    }

    /// A metric's open pages by start.
    pub fn pages(&self, uuid: &[u8; 16]) -> Option<&BTreeMap<i64, OpenPage>> {
        self.by_metric.get(uuid)
    }

    /// The page of `uuid` at `start` when it is still the one of `fileno`'s extent at `block`.
    fn current(&self, uuid: &[u8; 16], start: i64, fileno: u32, block: u64) -> Option<&OpenPage> {
        self.by_metric
            .get(uuid)?
            .get(&start)
            .filter(|p| p.fileno == fileno && p.block == block)
    }

    /// A file's current pages: the open-cache uses of it that its hot pages hold (D76.2).
    pub fn current_pages_of(&self, fileno: u32) -> u32 {
        self.by_file.get(&fileno).map_or(0, |list| {
            list.values()
                .filter(|&&(uuid, start, block)| {
                    self.current(&uuid, start, fileno, block).is_some()
                })
                .count() as u32
        })
    }

    /// A file's pages in the order they joined, as the indexer takes them (`pgc_open_cache_to_journal_v2()`).
    pub fn file_pages(&self, fileno: u32) -> Vec<Page> {
        let Some(list) = self.by_file.get(&fileno) else {
            return Vec::new();
        };
        list.values()
            .filter_map(|&(uuid, start, block)| {
                let p = self.current(&uuid, start, fileno, block)?;
                Some(Page {
                    uuid,
                    start_time_s: start,
                    end_time_s: p.end_time_s,
                    update_every_s: p.update_every_s,
                    block,
                    extent_bytes: p.bytes,
                    page_length: 0,
                })
            })
            .collect()
    }

    /// A file's pages leave the open cache: its v2 index serves them now.
    pub fn remove_file(&mut self, fileno: u32) {
        let Some(list) = self.by_file.remove(&fileno) else {
            return;
        };
        for (uuid, start, block) in list.into_values() {
            if self.current(&uuid, start, fileno, block).is_none() {
                continue;
            }
            if let Some(pages) = self.by_metric.get_mut(&uuid) {
                pages.remove(&start);
                if pages.is_empty() {
                    self.by_metric.remove(&uuid);
                }
            }
        }
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

/// `DATAFILE_ACQUIRE_REASONS`: what a use of a data file is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reason {
    /// A page of the open cache.
    OpenCache = 0,
    /// A query's page details, until the query is done with them.
    PageDetails = 1,
    /// A retention recalculation reading the file's index.
    Retention = 2,
    /// The indexer writing the file's v2 index.
    Indexing = 3,
}

/// `datafile->users`.
#[derive(Debug)]
struct Users {
    /// New uses are taken: false once a deletion found the file unused.
    available: bool,
    /// A deletion waits for the file: only the open cache may still take it, while extents are written to it.
    pending_deletion: bool,
    lockers: u32,
    by_reason: [u32; 4],
}

/// A use of a data file (`datafile_acquire()` until `datafile_release()`): the file stays until every use is dropped.
#[derive(Debug)]
pub(crate) struct FileUse {
    df: Arc<DataFile>,
    reason: Reason,
}

impl std::ops::Deref for FileUse {
    type Target = Arc<DataFile>;

    fn deref(&self) -> &Arc<DataFile> {
        &self.df
    }
}

impl Drop for FileUse {
    fn drop(&mut self) {
        let mut u = self.df.users();
        debug_assert!(u.lockers > 0, "a released data file use was not taken");
        u.lockers = u.lockers.saturating_sub(1);
        let r = &mut u.by_reason[self.reason as usize];
        *r = r.saturating_sub(1);
    }
}

/// A data file pair while the engine runs (`struct rrdengine_datafile` with its journal).
#[derive(Debug)]
pub struct DataFile {
    pub fileno: u32,
    pub(crate) file: IoFile,
    writers: Mutex<Writers>,
    users: Mutex<Users>,
    /// The file has clean pages in C's open cache (its indexed pages, and the v2 pages queries walked), each of which
    /// holds a use of it there; the Rust open cache keeps none, so one mark stands for them (D76.1).
    clean_open: AtomicBool,
    /// The v1 journal open for writes: only for a reused last file and the pairs created in this run, the files
    /// without a v2 index.
    journal: Option<IoFile>,
    /// `journalfile->unsafe.pos`: taken by each transaction as it is written.
    journal_pos: AtomicU64,
    /// `journalfile->v2.first_time_s` and `last_time_s`.
    first_time_s: AtomicI64,
    last_time_s: AtomicI64,
    /// `JOURNALFILE_FLAG_IS_AVAILABLE`: a v2 index serves the file.
    v2_available: AtomicBool,
}

impl DataFile {
    fn new(
        fileno: u32,
        file: IoFile,
        pos: u64,
        journal: Option<IoFile>,
        journal_pos: u64,
    ) -> DataFile {
        DataFile {
            fileno,
            file,
            writers: Mutex::new(Writers {
                pos,
                running: 0,
                flushed_to_open: 0,
                failed: false,
            }),
            users: Mutex::new(Users {
                available: true,
                pending_deletion: false,
                lockers: 0,
                by_reason: [0; 4],
            }),
            clean_open: AtomicBool::new(false),
            journal,
            journal_pos: AtomicU64::new(journal_pos),
            first_time_s: AtomicI64::new(0),
            last_time_s: AtomicI64::new(0),
            v2_available: AtomicBool::new(false),
        }
    }

    /// `journalfile->v2.first_time_s`.
    pub fn first_time_s(&self) -> i64 {
        self.first_time_s.load(Ordering::Acquire)
    }

    /// `journalfile->v2.last_time_s`.
    pub fn last_time_s(&self) -> i64 {
        self.last_time_s.load(Ordering::Acquire)
    }

    pub(crate) fn set_times(&self, first_time_s: i64, last_time_s: i64) {
        self.first_time_s.store(first_time_s, Ordering::Release);
        self.last_time_s.store(last_time_s, Ordering::Release);
    }

    /// A written extent's newest point moves the file's last time.
    pub(crate) fn extend_last_time(&self, last_time_s: i64) {
        self.last_time_s.fetch_max(last_time_s, Ordering::AcqRel);
    }

    /// Whether a v2 index serves the file.
    pub fn v2_available(&self) -> bool {
        self.v2_available.load(Ordering::Acquire)
    }

    fn writers(&self) -> MutexGuard<'_, Writers> {
        self.writers.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn users(&self) -> MutexGuard<'_, Users> {
        self.users.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `datafile_acquire()`: a use of the file for `reason`, none once the file is unavailable; while its deletion is
    /// pending, only the open cache may take it, and only while extents are still being written to it.
    pub(crate) fn acquire(self: &Arc<Self>, reason: Reason) -> Option<FileUse> {
        let mut u = self.users();
        if !u.available {
            return None;
        }
        if u.pending_deletion {
            let w = self.writers();
            if reason != Reason::OpenCache || (w.running == 0 && w.flushed_to_open == 0) {
                return None;
            }
        }
        u.lockers += 1;
        u.by_reason[reason as usize] += 1;
        Some(FileUse {
            df: Arc::clone(self),
            reason,
        })
    }

    /// The uses the file has, and those for `reason`.
    pub(crate) fn lockers(&self, reason: Option<Reason>) -> u32 {
        let u = self.users();
        reason.map_or(u.lockers, |r| u.by_reason[r as usize])
    }

    /// `datafile->users.pending_deletion`.
    pub fn pending_deletion(&self) -> bool {
        self.users().pending_deletion
    }

    /// C's open cache took clean pages of the file (D76.1): its hot pages turned clean, keeping their uses.
    pub(crate) fn mark_clean_open(&self) {
        self.clean_open.store(true, Ordering::Release);
    }

    /// A query's page of the file joins C's open cache as a clean page, which takes a use of the file
    /// (`datafile_acquire(OPEN_CACHE)`) and so is refused as that is.
    pub(crate) fn add_clean_open(&self) {
        if self.clean_open.load(Ordering::Acquire) {
            return;
        }
        let u = self.users();
        if !u.available {
            return;
        }
        if u.pending_deletion {
            let w = self.writers();
            if w.running == 0 && w.flushed_to_open == 0 {
                return;
            }
        }
        self.clean_open.store(true, Ordering::Release);
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
    /// the write fails), then written with C's retries; where it went.
    pub(crate) fn append_journal(&self, block: &[u8; BLOCK_SIZE]) -> io::Result<u64> {
        let at = self
            .journal_pos
            .fetch_add(BLOCK_SIZE as u64, Ordering::AcqRel);
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| io::Error::from_raw_os_error(libc::EBADF))?;
        write_retrying(journal, block, at).map(|()| at)
    }
}

/// A tier as the engine runs it.
#[derive(Debug)]
pub struct TierData {
    pub config: TierConfig,
    /// The pairs by file number (`ctx->datafiles`).
    files: RwLock<BTreeMap<u32, Arc<DataFile>>>,
    /// `njfv2idx`: the serving v2 files by their end, then number.
    v2: RwLock<BTreeMap<(i64, u32), Arc<V2Index>>>,
    open: RwLock<OpenList>,
    /// `ctx->atomic.last_fileno`: the newest file number, whose successor a new pair takes.
    last_fileno: AtomicU32,
    /// `ctx->atomic.last_flush_fileno`: the newest file an extent went to.
    last_flush_fileno: AtomicU32,
    /// `ctx->atomic.transaction_id`: the next transaction's id.
    transaction_id: AtomicU64,
    /// `ctx->atomic.first_time_s`: the readiness sets it, a deletion moves it to the remaining files' oldest start.
    first_time_s: AtomicI64,
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
        for (i, (fileno, p)) in (0u32..).zip(tier.open_pages) {
            let page = OpenPage {
                end_time_s: p.end_time_s,
                update_every_s: p.update_every_s,
                fileno,
                block: p.block,
                bytes: p.extent_bytes,
            };
            open.add(p.uuid, p.start_time_s, page, (0, i));
        }
        let files = tier
            .files
            .into_iter()
            .map(|p| {
                let file = DataFile::new(p.fileno, p.file, p.pos, p.journal, p.journal_pos);
                // a v2 index whose population failed is unmapped, which clears its times (D76.4)
                if p.v2.is_some() && !tier.indexes.contains_key(&p.fileno) {
                    file.set_times(0, 0);
                } else {
                    file.set_times(p.first_time_s, p.last_time_s);
                }
                if p.clean_open {
                    file.mark_clean_open();
                }
                // a file whose population failed serves nothing: it looks unindexed, as C clears its flag
                file.v2_available
                    .store(tier.indexes.contains_key(&p.fileno), Ordering::Release);
                (p.fileno, Arc::new(file))
            })
            .collect();
        TierData {
            config: tier.config,
            files: RwLock::new(files),
            v2: RwLock::new(
                tier.indexes
                    .into_values()
                    .map(|index| ((index.end_time_s(), index.fileno), Arc::new(index)))
                    .collect(),
            ),
            open: RwLock::new(open),
            last_fileno: AtomicU32::new(tier.last_fileno),
            last_flush_fileno: AtomicU32::new(0),
            transaction_id: AtomicU64::new(tier.transaction_id),
            first_time_s: AtomicI64::new(tier.first_time_s),
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

    /// A pair by its number, while it is listed (queries read through the uses of it they take).
    pub fn file(&self, fileno: u32) -> Option<Arc<DataFile>> {
        read(&self.files).get(&fileno).cloned()
    }

    pub fn has_file(&self, fileno: u32) -> bool {
        read(&self.files).contains_key(&fileno)
    }

    /// The pairs' numbers, in order.
    pub fn filenos(&self) -> Vec<u32> {
        read(&self.files).keys().copied().collect()
    }

    /// The pairs, for a query to take uses of them without holding the list.
    pub(crate) fn files_snapshot(&self) -> BTreeMap<u32, Arc<DataFile>> {
        read(&self.files).clone()
    }

    /// The oldest pair (`get_first_ctx_datafile()`).
    pub(crate) fn first_file(&self) -> Option<Arc<DataFile>> {
        read(&self.files).values().next().cloned()
    }

    /// The pair after `fileno` (`get_next_datafile()`).
    pub(crate) fn next_fileno(&self, fileno: u32) -> Option<u32> {
        read(&self.files)
            .range(fileno + 1..)
            .next()
            .map(|(f, _)| *f)
    }

    /// The first pair from `fileno` on that can be taken for `reason`, passing over those that cannot.
    pub(crate) fn acquire_from(&self, fileno: u32, reason: Reason) -> Option<FileUse> {
        read(&self.files)
            .range(fileno..)
            .find_map(|(_, df)| df.acquire(reason))
    }

    /// `datafile_list_delete_unsafe()`: the pair leaves the list.
    pub(crate) fn remove_file(&self, fileno: u32) -> Option<Arc<DataFile>> {
        write(&self.files).remove(&fileno)
    }

    /// The v2 index serving a file.
    pub(crate) fn v2_of(&self, fileno: u32) -> Option<Arc<V2Index>> {
        read(&self.v2)
            .values()
            .find(|i| i.fileno == fileno)
            .cloned()
    }

    /// `njfv2idx_remove()`: the file's v2 index stops serving (queries holding it keep reading it).
    pub(crate) fn remove_v2(&self, fileno: u32) -> Option<Arc<V2Index>> {
        let mut v2 = write(&self.v2);
        let key = v2
            .iter()
            .find(|(_, i)| i.fileno == fileno)
            .map(|(k, _)| *k)?;
        v2.remove(&key)
    }

    /// `datafile_acquire()` of a listed pair.
    pub(crate) fn acquire(&self, fileno: u32, reason: Reason) -> Option<FileUse> {
        self.file(fileno)?.acquire(reason)
    }

    /// The open cache's uses of a file: its current hot pages and its clean-page mark (D76.1, D76.2).
    pub(crate) fn open_lockers(&self, df: &DataFile) -> u32 {
        read(&self.open).current_pages_of(df.fileno)
            + u32::from(df.clean_open.load(Ordering::Acquire))
    }

    /// `datafile_acquire_for_deletion()`: marks the file pending deletion (recorded once); true when no extent is being
    /// written to it and nothing uses it, which then makes it unavailable. While uses remain its clean open-cache
    /// pages are evicted, and once no extent is being written the file takes no new uses (recorded once). The open
    /// cache's uses are the file's current hot pages and its clean pages (D76.1, D76.2).
    ///
    /// Both passes read the hot pages, the users and the writer counters under one read guard of the open list: a
    /// flush joins its pages to the open list before its writer counter drops, so a file with an extent in flight
    /// shows either (C reads its lockers and writers under the users lock for the same reason).
    pub(crate) fn acquire_for_deletion(&self, df: &DataFile) -> bool {
        let tier = self.tier();
        let (marked, deletable, evict) = {
            let open = read(&self.open);
            let hot = open.current_pages_of(df.fileno);
            let mut u = df.users();
            let marked = !u.pending_deletion;
            u.pending_deletion = true;
            let (running, flushed) = df.writers_running();
            let lockers = u.lockers + hot + u32::from(df.clean_open.load(Ordering::Acquire));
            let deletable = running == 0 && flushed == 0 && lockers == 0;
            if deletable {
                u.available = false;
            }
            (marked, deletable, lockers != 0)
        };
        if marked {
            netdata_log_info!(
                "DBENGINE: tier {tier}: datafile-1-{:010} is pending deletion",
                df.fileno
            );
        }
        if deletable {
            return true;
        }
        if evict {
            // pgc_open_evict_clean_pages_of_datafile()
            df.clean_open.store(false, Ordering::Release);
        }
        let open = read(&self.open);
        let hot = open.current_pages_of(df.fileno);
        let mut u = df.users();
        let (running, flushed) = df.writers_running();
        if running != 0 || flushed != 0 {
            return false;
        }
        if u.available {
            u.available = false;
            netdata_log_info!(
                "DBENGINE: tier {tier}: datafile-1-{:010} entered deletion phase-2 (new users blocked)",
                df.fileno
            );
        }
        u.lockers + hot == 0
    }

    /// The pair extents are written to (a tier always has one).
    pub fn last_file(&self) -> Arc<DataFile> {
        read(&self.files)
            .values()
            .next_back()
            .cloned()
            .expect("a tier has a pair")
    }

    /// The v2 files a query from `start_s` walks: the one ending last before it (`JudyLPrev()`), then every later one.
    pub(crate) fn v2_from(&self, start_s: i64) -> Vec<Arc<V2Index>> {
        let v2 = read(&self.v2);
        let first = v2.range(..(start_s, 0)).next_back().map(|(k, _)| *k);
        let files = match first {
            Some(k) => v2.range(k..),
            None => v2.range(..),
        };
        files.map(|(_, index)| Arc::clone(index)).collect()
    }

    /// `njfv2idx_add()`: a v2 index this run wrote starts serving its file.
    pub(crate) fn add_v2(&self, df: &DataFile, index: V2Index) {
        write(&self.v2).insert((index.end_time_s(), index.fileno), Arc::new(index));
        df.v2_available.store(true, Ordering::Release);
    }

    /// `release_and_aquire_next_datafile_for_indexing()`: the first file after `after` (from the first without one)
    /// without a v2 index, walking while a file is neither the last one nor the one extents last went to, taken for
    /// indexing in five attempts 200 ms apart; a file that cannot be taken is recorded and passed over. The list is not
    /// held while the attempts wait (C holds it; here that would stall the extent writer's new pairs).
    pub(crate) fn next_for_indexing(&self, after: Option<u32>) -> Option<FileUse> {
        let mut from = after.map_or(0, |f| f + 1);
        loop {
            let df = {
                let files = read(&self.files);
                let (last, flushed) = (self.last_fileno(), self.last_flush_fileno());
                let mut candidate = None;
                for df in files.range(from..).map(|(_, df)| df) {
                    if df.fileno == last || df.fileno == flushed {
                        return None;
                    }
                    if !df.v2_available() {
                        candidate = Some(Arc::clone(df));
                        break;
                    }
                }
                candidate?
            };
            // C pauses after every refusal, the last one included
            for _ in 0..INDEXING_ATTEMPTS {
                if let Some(use_) = df.acquire(Reason::Indexing) {
                    return Some(use_);
                }
                indexing_pause();
            }
            netdata_log_info!(
                "DBENGINE: tier {}: datafile-1-{:010} cannot be locked for indexing after retries; skipping",
                self.tier(),
                df.fileno
            );
            from = df.fileno + 1;
        }
    }

    /// `rrdeng_ctx_tier_cap_exceeded()` with two files at least: the first file's newest point older than the time
    /// quota, or the space the tier will take with its last file full over the space quota (C adds a share of the
    /// database files, which `get_total_database_space()` gives as 0, D67.5).
    pub(crate) fn cap_exceeded(&self, now_s: i64) -> bool {
        let files = read(&self.files);
        let (Some(first), Some(last)) = (files.values().next(), files.values().next_back()) else {
            return false;
        };
        if files.len() < 2 {
            return false;
        }
        if self.config.max_retention_s != 0 {
            let t = match first.last_time_s() {
                0 => first.first_time_s(),
                t => t,
            };
            if t != 0 && t <= now_s - self.config.max_retention_s {
                return true;
            }
        }
        self.config.max_disk_space != 0
            && self.used_disk_space_after(last.pos()) > self.config.max_disk_space
    }

    /// `rrdeng_get_used_disk_space()`: the space the tier will take with its last file full, its files plus a file's
    /// target less what the last file holds (nothing without files); C's share of the database files is 0 (D67.5).
    pub fn used_disk_space(&self) -> u64 {
        let active = read(&self.files)
            .values()
            .next_back()
            .map_or(0, |last| last.pos());
        self.used_disk_space_after(active)
    }

    /// `rrdeng_get_used_disk_space(having_lock)`: with the files already read.
    fn used_disk_space_after(&self, active: u64) -> u64 {
        self.current_disk_space()
            .wrapping_add(self.config.target_datafile_size())
            .wrapping_sub(active)
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
        self.last_flush_fileno.fetch_max(fileno, Ordering::AcqRel);
    }

    /// A transaction id, taken for good.
    pub(crate) fn next_transaction_id(&self) -> u64 {
        self.transaction_id.fetch_add(1, Ordering::AcqRel)
    }

    pub fn current_disk_space(&self) -> u64 {
        self.current_disk_space.load(Ordering::Acquire)
    }

    pub(crate) fn add_disk_space(&self, bytes: u64) {
        self.current_disk_space.fetch_add(bytes, Ordering::AcqRel);
    }

    /// `ctx_current_disk_space_decrease()`: a plain subtraction, which wraps as C's.
    pub(crate) fn sub_disk_space(&self, bytes: u64) {
        self.current_disk_space.fetch_sub(bytes, Ordering::AcqRel);
    }

    /// `ctx->atomic.first_time_s`.
    pub fn first_time_s(&self) -> i64 {
        self.first_time_s.load(Ordering::Relaxed)
    }

    pub(crate) fn set_first_time_s(&self, first_time_s: i64) {
        self.first_time_s.store(first_time_s, Ordering::Relaxed);
    }

    pub fn needs_indexing(&self) -> bool {
        self.needs_indexing.load(Ordering::Acquire)
    }

    pub(crate) fn set_needs_indexing(&self) {
        self.needs_indexing.store(true, Ordering::Release);
    }

    pub(crate) fn clear_needs_indexing(&self) {
        self.needs_indexing.store(false, Ordering::Release);
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

    /// `rrdeng_atomic_uint64_sub_saturating()` of the samples.
    pub(crate) fn sub_samples_saturating(&self, value: u64, reason: &str) {
        static UNDERFLOW: ErrorLimit = ErrorLimit::new(60, 0);
        sub_saturating(
            &self.samples,
            value,
            self.tier(),
            "samples",
            reason,
            &UNDERFLOW,
        );
    }

    /// `rrdeng_global_first_time_s()`: the tier's oldest time, 0 while unknown.
    pub fn global_first_time_s(&self) -> i64 {
        match self.first_time_s() {
            i64::MAX => 0,
            t if t < 0 => 0,
            t => t,
        }
    }

    /// `rrdeng_get_directory_free_bytes_space()`: the bytes of the tier's filesystem free to unprivileged users, less
    /// 5%; 0 when the filesystem cannot be read.
    pub fn directory_free_bytes(&self) -> u64 {
        let space = netdata_agent_sys::disk_space(&self.config.path);
        let free = if space.total_bytes > 0 { space.free_bytes } else { 0 };
        free - free * 5 / 100
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
