//! The main and extent caches (`cache.c` as the engine uses it, D62.5, D65.N3). The main cache indexes pages by tier,
//! metric and start whatever their state: hot pages (being collected) and dirty ones (closed, waiting for their extent)
//! sit in per-tier queues outside the LRU and the budget, as C's hot and dirty queues, and flushing ones (in an extent
//! being written) in none; clean pages (read from disk, gaps, flushed) are the LRU, the least recently used dropped
//! while over the budget when nobody holds them, when a clean page is added and after a flush (C's evictor threads,
//! autoscaling and memory pressure wait for S6). The extent cache holds raw extents by tier, file and block, the oldest
//! dropped first.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use crate::dbengine::RRD_STORAGE_TIERS;
use crate::dbengine::format::page::{Cursor, DiskPage, PageBuilder};
use crate::storage_point::StoragePoint;

/// A page's place in the main cache (`PGC_PAGE_HOT`, `PGC_PAGE_DIRTY`, `PGC_PAGE_CLEAN`, and none of them while
/// flushing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageState {
    Hot,
    Dirty,
    Clean,
    Flushing,
}

impl PageState {
    fn from_u8(v: u8) -> PageState {
        match v {
            0 => PageState::Hot,
            1 => PageState::Dirty,
            2 => PageState::Clean,
            _ => PageState::Flushing,
        }
    }
}

/// A page's data.
#[derive(Debug)]
enum PageData {
    /// `PGD_EMPTY`: a gap, or a page that failed to load.
    Empty,
    Disk(DiskPage),
    /// `pgd_create()`'s data: filled by its collector while hot, kept as it is until the page leaves the cache.
    Collected(Mutex<PageBuilder>),
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn decode(mut cursor: Cursor<'_>, points: usize) -> Vec<(bool, StoragePoint)> {
    (0..points).map(|_| cursor.next_point()).collect()
}

/// A cached page: its times (the end grows while the page is hot; the end and update every can be repaired by
/// queries, as C repairs them in the cache), its data and its state.
#[derive(Debug)]
pub struct CachedPage {
    /// Changed only while nobody else holds the page (`restart_at()`).
    pub start_time_s: i64,
    end_time_s: AtomicI64,
    update_every_s: AtomicU32,
    data: PageData,
    /// What the page counts against the budget or its queue; changed under the cache's lock.
    size: AtomicUsize,
    state: AtomicU8,
    /// The page's key in its hot or dirty queue; changed under the cache's lock.
    seq: AtomicU64,
    /// `accesses > 0`: a search found the page, or it came clean; a flushed page nobody read is the first evicted.
    accessed: AtomicBool,
}

impl CachedPage {
    /// A clean page; `None` for an empty one.
    pub fn new(
        start_time_s: i64,
        end_time_s: i64,
        update_every_s: u32,
        data: Option<DiskPage>,
    ) -> Self {
        let size = 64 + data.as_ref().map_or(0, DiskPage::footprint);
        let data = data.map_or(PageData::Empty, PageData::Disk);
        CachedPage::with_state(
            start_time_s,
            end_time_s,
            update_every_s,
            data,
            size,
            PageState::Clean,
        )
    }

    /// A hot page holding its collector's first point at `start_time_s`.
    pub(crate) fn collected(start_time_s: i64, update_every_s: u32, builder: PageBuilder) -> Self {
        let size = 64 + builder.memory_footprint();
        let data = PageData::Collected(Mutex::new(builder));
        CachedPage::with_state(
            start_time_s,
            start_time_s,
            update_every_s,
            data,
            size,
            PageState::Hot,
        )
    }

    fn with_state(
        start_time_s: i64,
        end_time_s: i64,
        update_every_s: u32,
        data: PageData,
        size: usize,
        state: PageState,
    ) -> Self {
        CachedPage {
            start_time_s,
            end_time_s: AtomicI64::new(end_time_s),
            update_every_s: AtomicU32::new(update_every_s),
            data,
            size: AtomicUsize::new(size),
            state: AtomicU8::new(state as u8),
            seq: AtomicU64::new(0),
            accessed: AtomicBool::new(state != PageState::Hot),
        }
    }

    /// A page the cache refused, moved to start and end at `start_time_s` for another try.
    pub(crate) fn restart_at(&mut self, start_time_s: i64) {
        self.start_time_s = start_time_s;
        *self.end_time_s.get_mut() = start_time_s;
    }

    pub fn end_time_s(&self) -> i64 {
        self.end_time_s.load(Ordering::Acquire)
    }

    pub fn update_every_s(&self) -> u32 {
        self.update_every_s.load(Ordering::Acquire)
    }

    pub fn state(&self) -> PageState {
        PageState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, state: PageState) {
        self.state.store(state as u8, Ordering::Release);
    }

    /// `pgc_is_page_hot()`.
    pub fn is_hot(&self) -> bool {
        self.state() == PageState::Hot
    }

    fn size(&self) -> usize {
        self.size.load(Ordering::Relaxed)
    }

    /// The data is `PGD_EMPTY`.
    pub fn is_gap(&self) -> bool {
        matches!(self.data, PageData::Empty)
    }

    /// `pgd_is_empty()` of the page's data.
    pub fn is_empty(&self) -> bool {
        match &self.data {
            PageData::Empty => true,
            PageData::Disk(d) => d.is_empty(),
            PageData::Collected(b) => lock(b).is_empty(),
        }
    }

    /// `pgd_slots_used()`.
    pub fn slots_used(&self) -> usize {
        match &self.data {
            PageData::Empty => 0,
            PageData::Disk(d) => d.slots_used(),
            PageData::Collected(b) => lock(b).slots_used(),
        }
    }

    /// `pgdc_reset()` at `position`, then the points up to `entries`; a collected page is read as it is now.
    pub fn points(&self, position: usize, entries: usize) -> Vec<(bool, StoragePoint)> {
        let n = entries.saturating_sub(position);
        match &self.data {
            PageData::Empty => Vec::new(),
            PageData::Disk(d) => decode(d.cursor(position), n),
            PageData::Collected(b) => decode(lock(b).cursor(position), n),
        }
    }

    /// A flushing page as its extent takes it (`pgd_disk_footprint()`, `pgd_copy_to_extent()`): its type, points and
    /// bytes; from now on it takes no more points. `None` for a page not collected here.
    pub(crate) fn extent_data(&self) -> Option<(u8, usize, Vec<u8>)> {
        let mut builder = self.builder()?;
        builder.schedule_for_flushing();
        Some((
            builder.page_type(),
            builder.slots_used(),
            builder.to_extent_bytes(),
        ))
    }

    /// The collector's data; `None` for a page not collected here.
    pub(crate) fn builder(&self) -> Option<MutexGuard<'_, PageBuilder>> {
        match &self.data {
            PageData::Collected(b) => Some(lock(b)),
            _ => None,
        }
    }

    /// `pgc_page_hot_set_end_time_s()` without the size, which `MainCache::grew()` accounts: the collector stores
    /// the point before the end, so queries that read the end find its point.
    pub(crate) fn hot_set_end_time_s(&self, end_time_s: i64) {
        self.end_time_s.store(end_time_s, Ordering::Release);
    }

    /// `pgc_page_fix_update_every()`: a page without an update every takes `update_every_s`; the one in force.
    pub fn fix_update_every(&self, update_every_s: u32) -> u32 {
        match self.update_every_s.compare_exchange(
            0,
            update_every_s,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => update_every_s,
            Err(current) => current,
        }
    }

    /// `pgc_page_fix_end_time_s()`: the page ends earlier (its data holds fewer points than its times say); the end
    /// in force.
    pub fn fix_end_time_s(&self, end_time_s: i64) -> i64 {
        self.end_time_s.store(end_time_s, Ordering::Release);
        end_time_s
    }
}

/// `PGC_SEARCH`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Search {
    /// The page starting at the time, else the one before it that holds the time, else the next one.
    Closest,
    /// The page starting after the time.
    Next,
    /// The page starting at the time.
    Exact,
}

/// The main and extent caches' budgets in bytes from `[db] dbengine page cache size` and `dbengine extent cache size`
/// (MiB), as `pgc_and_mrg_initialize()` splits them: 70% and 30% of the page cache, the extent share at least 5 MiB
/// (taken from the main one), plus the extent cache size.
pub fn cache_budgets(page_cache_mb: i32, extent_cache_mb: i32) -> (usize, usize) {
    const MIB: usize = 1024 * 1024;
    let target = (page_cache_mb as usize).wrapping_mul(MIB);
    let (mut main, mut extent) = (target / 100 * 70, target / 100 * 30);
    if extent < 5 * MIB {
        extent = 5 * MIB;
        main = target.wrapping_sub(extent);
    }
    extent = extent.wrapping_add((extent_cache_mb as usize).wrapping_mul(MIB));
    (main, extent)
}

type PageKey = (usize, [u8; 16]);

/// How many held pages one eviction pass steps over before it gives up: the cache then stays over its budget until
/// queries release their pages, as C's does, without rescanning every held page on each insert.
const EVICT_SKIPS: usize = 64;

#[derive(Debug)]
struct Entry {
    page: Arc<CachedPage>,
    /// The page's place in `MainInner::lru`, for clean pages.
    tick: Option<i64>,
}

/// A hot or dirty queue of a tier (`linked_list_in_sections_judy`): pages in the order they joined, with their
/// metric.
#[derive(Debug, Default)]
struct Queue {
    pages: BTreeMap<u64, ([u8; 16], Arc<CachedPage>)>,
}

#[derive(Debug, Default)]
struct MainInner {
    pages: HashMap<PageKey, BTreeMap<i64, Entry>>,
    /// Clean pages by last access, least recent first.
    lru: BTreeMap<i64, (PageKey, i64)>,
    /// The most recent tick, and the least recent one (C prepends clean pages nobody accessed).
    tick: i64,
    front: i64,
    /// What the clean pages take, which the budget bounds.
    bytes: usize,
    hot: [Queue; RRD_STORAGE_TIERS],
    dirty: [Queue; RRD_STORAGE_TIERS],
    seq: u64,
    hot_bytes: usize,
    /// The hot queue's peak size, updated when a page joins it (`pgc_queue_add()`).
    hot_max_bytes: usize,
    dirty_bytes: usize,
    /// Bumped whenever a tier's dirty queue reaches a multiple of the pages per extent.
    dirty_version: u64,
    /// The version the last flush that walked every queue saw.
    last_version_checked: u64,
    flushing_entries: usize,
    flushing_bytes: usize,
}

impl MainInner {
    /// `page_has_been_accessed()`: marks a cached page as accessed, a clean one as the most recently used; returns
    /// any cached page.
    fn touch(&mut self, key: PageKey, start: i64) -> Option<Arc<CachedPage>> {
        let entry = self.pages.get_mut(&key)?.get_mut(&start)?;
        entry.page.accessed.store(true, Ordering::Relaxed);
        if let Some(tick) = entry.tick {
            self.tick += 1;
            self.lru.remove(&tick);
            entry.tick = Some(self.tick);
            self.lru.insert(self.tick, (key, start));
        }
        Some(Arc::clone(&entry.page))
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// `pgc_page_add_and_acquire()` found a page at the same start: the held one, and the page given back.
#[derive(Debug)]
pub struct Conflict {
    pub existing: Arc<CachedPage>,
    pub page: Box<CachedPage>,
}

/// The main cache's accounting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub clean_bytes: usize,
    pub hot_entries: usize,
    pub hot_bytes: usize,
    pub hot_max_bytes: usize,
    pub dirty_entries: usize,
    pub dirty_bytes: usize,
    pub dirty_version: u64,
    pub flushing_entries: usize,
    pub flushing_bytes: usize,
}

/// One tier's dirty pages, oldest first, taken by the flusher for one extent.
#[derive(Debug, Clone)]
pub struct Batch {
    pub tier: usize,
    pub pages: Vec<([u8; 16], Arc<CachedPage>)>,
}

/// The main cache.
#[derive(Debug)]
pub struct MainCache {
    inner: Mutex<MainInner>,
    budget: usize,
    /// `max_dirty_pages_per_call`: `rrdeng_pages_per_extent`.
    pages_per_extent: usize,
    /// `flushing_critical()`'s verdict, kept as the sizes change so that collectors and queries read it unlocked.
    critical: AtomicBool,
}

impl MainCache {
    pub fn new(budget: usize, pages_per_extent: usize) -> Self {
        MainCache {
            inner: Mutex::default(),
            budget,
            pages_per_extent: pages_per_extent.max(1),
            critical: AtomicBool::new(false),
        }
    }

    /// After the dirty size or the hot peak changed.
    fn note_sizes(&self, inner: &MainInner) {
        self.critical
            .store(inner.dirty_bytes > inner.hot_max_bytes, Ordering::Relaxed);
    }

    fn lock(&self) -> MutexGuard<'_, MainInner> {
        lock(&self.inner)
    }

    /// `pgc_page_get_and_acquire()`.
    pub fn search(
        &self,
        tier: usize,
        uuid: &[u8; 16],
        time_s: i64,
        mode: Search,
    ) -> Option<Arc<CachedPage>> {
        let mut inner = self.lock();
        let key = (tier, *uuid);
        let pages = inner.pages.get(&key)?;
        let next = || pages.range(time_s + 1..).next().map(|(s, _)| *s);
        let start = match mode {
            Search::Exact => pages.contains_key(&time_s).then_some(time_s),
            Search::Next => next(),
            Search::Closest if pages.contains_key(&time_s) => Some(time_s),
            Search::Closest => pages
                .range(..time_s)
                .next_back()
                .filter(|(_, e)| time_s <= e.page.end_time_s())
                .map(|(s, _)| *s)
                .or_else(next),
        }?;
        inner.touch(key, start)
    }

    /// `pgc_page_add_and_acquire()`: a page already cached at the same start wins over the new one, which comes back
    /// untouched; neither moves in the LRU. A hot page joins its tier's hot queue, a clean one the LRU.
    pub fn add(
        &self,
        tier: usize,
        uuid: &[u8; 16],
        page: CachedPage,
    ) -> Result<Arc<CachedPage>, Conflict> {
        let mut inner = self.lock();
        let key = (tier, *uuid);
        let start = page.start_time_s;
        if let Some(e) = inner.pages.get(&key).and_then(|p| p.get(&start)) {
            return Err(Conflict {
                existing: Arc::clone(&e.page),
                page: Box::new(page),
            });
        }
        let page = Arc::new(page);
        let size = page.size();
        let tick = if page.is_hot() {
            let seq = inner.next_seq();
            page.seq.store(seq, Ordering::Relaxed);
            inner.hot[tier]
                .pages
                .insert(seq, (*uuid, Arc::clone(&page)));
            inner.hot_bytes += size;
            inner.hot_max_bytes = inner.hot_max_bytes.max(inner.hot_bytes);
            self.note_sizes(&inner);
            None
        } else {
            inner.bytes += size;
            inner.tick += 1;
            let tick = inner.tick;
            inner.lru.insert(tick, (key, start));
            Some(tick)
        };
        inner.pages.entry(key).or_default().insert(
            start,
            Entry {
                page: Arc::clone(&page),
                tick,
            },
        );
        if tick.is_some() {
            self.evict(&mut inner);
        }
        Ok(page)
    }

    /// `page_set_dirty()` of a hot page: it leaves the hot queue for the end of its tier's dirty queue.
    pub fn hot_to_dirty(&self, tier: usize, page: &CachedPage) {
        let mut inner = self.lock();
        self.move_to_dirty(&mut inner, tier, page.seq.load(Ordering::Relaxed));
    }

    /// `all_hot_pages_to_dirty()`: every hot page of the tier turns dirty, in the order they turned hot.
    pub fn all_hot_to_dirty(&self, tier: usize) {
        let mut inner = self.lock();
        let seqs: Vec<u64> = inner.hot[tier].pages.keys().copied().collect();
        for seq in seqs {
            self.move_to_dirty(&mut inner, tier, seq);
        }
    }

    fn move_to_dirty(&self, inner: &mut MainInner, tier: usize, seq: u64) {
        let Some((uuid, page)) = inner.hot[tier].pages.remove(&seq) else {
            return;
        };
        let size = page.size();
        inner.hot_bytes -= size;
        page.set_state(PageState::Dirty);
        let seq = inner.next_seq();
        page.seq.store(seq, Ordering::Relaxed);
        inner.dirty[tier].pages.insert(seq, (uuid, page));
        inner.dirty_bytes += size;
        if inner.dirty[tier]
            .pages
            .len()
            .is_multiple_of(self.pages_per_extent)
        {
            inner.dirty_version += 1;
        }
        self.note_sizes(inner);
    }

    /// `flushing_critical()`: the dirty pages take more than the hot ones ever did.
    pub fn flushing_critical(&self) -> bool {
        self.critical.load(Ordering::Relaxed)
    }

    /// `pgc_hot_and_dirty_entries()`: the pages not yet on disk.
    pub fn hot_and_dirty_entries(&self) -> usize {
        let s = self.stats();
        s.hot_entries + s.dirty_entries + s.flushing_entries
    }

    /// `pgc_page_to_clean_evict_or_release()` of a hot page without data: it leaves the cache unless someone else
    /// holds it, then it stays as the least recently used clean page. Whether it left.
    pub fn to_clean_evict_or_release(&self, tier: usize, page: Arc<CachedPage>) -> bool {
        let mut inner = self.lock();
        let (seq, start) = (page.seq.load(Ordering::Relaxed), page.start_time_s);
        drop(page);
        let Some((uuid, page)) = inner.hot[tier].pages.remove(&seq) else {
            return false;
        };
        let size = page.size();
        inner.hot_bytes -= size;
        drop(page);
        let key = (tier, uuid);
        let Some(pages) = inner.pages.get_mut(&key) else {
            return false;
        };
        let held = pages
            .get(&start)
            .is_some_and(|e| Arc::strong_count(&e.page) > 1);
        if !held {
            pages.remove(&start);
            if pages.is_empty() {
                inner.pages.remove(&key);
            }
            return true;
        }
        inner.front -= 1;
        let tick = inner.front;
        if let Some(e) = inner.pages.get_mut(&key).and_then(|p| p.get_mut(&start)) {
            e.page.set_state(PageState::Clean);
            e.tick = Some(tick);
        }
        inner.lru.insert(tick, (key, start));
        inner.bytes += size;
        false
    }

    /// A collected page's data grew by `delta` bytes: the queue it is in counts them (C
    /// `pgc_page_hot_set_end_time_s()`), without a new peak.
    pub fn grew(&self, page: &CachedPage, delta: usize) {
        let mut inner = self.lock();
        page.size.fetch_add(delta, Ordering::Relaxed);
        match page.state() {
            PageState::Hot => inner.hot_bytes += delta,
            PageState::Dirty => inner.dirty_bytes += delta,
            PageState::Clean => inner.bytes += delta,
            PageState::Flushing => inner.flushing_bytes += delta,
        }
        self.note_sizes(&inner);
    }

    /// `flush_pages()`: batches of pages-per-extent dirty pages from each tier's queue head (fewer only with `all`),
    /// tiers in order (only `section`'s when given), each counted by `save_init` under the lock and written by `save`
    /// after it; the pages turn clean after `save`, whatever became of the write. Without `all` it gives up while
    /// fewer pages wait than an extent takes, or when nothing changed since the last walk that went through; without
    /// `all` and `wait` also when the cache is busy. `max_flushes` (0 for any) bounds the batches, one more running
    /// as C's test is `>`. Whether it stopped before the queues ran short.
    pub fn flush_pages(
        &self,
        max_flushes: usize,
        section: Option<usize>,
        wait: bool,
        all: bool,
        mut save_init: impl FnMut(usize),
        mut save: impl FnMut(&Batch),
    ) -> bool {
        let optimal = self.pages_per_extent;
        let mut inner = if !all && !wait {
            match self.inner.try_lock() {
                Ok(inner) => inner,
                Err(TryLockError::Poisoned(p)) => p.into_inner(),
                Err(TryLockError::WouldBlock) => return false,
            }
        } else {
            self.lock()
        };
        let version_at_entry = inner.dirty_version;
        let entries: usize = inner.dirty.iter().map(|q| q.pages.len()).sum();
        if !all && (entries < optimal || inner.last_version_checked == version_at_entry) {
            return false;
        }
        let max_flushes = if all || max_flushes == 0 {
            usize::MAX
        } else {
            max_flushes
        };
        let (mut flushes, mut stopped) = (0, false);
        // `JudyLFirstThenNext()`: after a batch the same tier again, else the next one
        let (mut from, mut first) = (section.unwrap_or(0), true);
        loop {
            let start = if first { from } else { from + 1 };
            first = false;
            let Some(tier) = (start..RRD_STORAGE_TIERS).find(|&t| !inner.dirty[t].pages.is_empty())
            else {
                break;
            };
            from = tier;
            if section.is_some_and(|s| s != tier) {
                break;
            }
            if !all && inner.dirty[tier].pages.len() < optimal {
                continue;
            }
            if !all && flushes > max_flushes {
                stopped = true;
                break;
            }
            let seqs: Vec<u64> = inner.dirty[tier]
                .pages
                .keys()
                .take(optimal)
                .copied()
                .collect();
            let mut pages = Vec::with_capacity(seqs.len());
            for seq in seqs {
                if let Some((uuid, page)) = inner.dirty[tier].pages.remove(&seq) {
                    let size = page.size();
                    inner.dirty_bytes -= size;
                    inner.flushing_entries += 1;
                    inner.flushing_bytes += size;
                    page.set_state(PageState::Flushing);
                    pages.push((uuid, page));
                }
            }
            first = true;
            self.note_sizes(&inner);
            save_init(tier);
            drop(inner);
            let batch = Batch { tier, pages };
            save(&batch);
            flushes += 1;
            inner = self.lock();
            self.flushed(&mut inner, batch);
        }
        if !stopped && version_at_entry > inner.last_version_checked {
            inner.last_version_checked = version_at_entry;
        }
        stopped
    }

    /// `page_set_clean()` of a flushed batch: pages someone read become the most recently used, the others the least;
    /// then the budget holds.
    fn flushed(&self, inner: &mut MainInner, batch: Batch) {
        for (uuid, page) in batch.pages {
            let size = page.size();
            inner.flushing_entries -= 1;
            inner.flushing_bytes -= size;
            page.set_state(PageState::Clean);
            let tick = if page.accessed.load(Ordering::Relaxed) {
                inner.tick += 1;
                inner.tick
            } else {
                inner.front -= 1;
                inner.front
            };
            let key = (batch.tier, uuid);
            let start = page.start_time_s;
            if let Some(e) = inner.pages.get_mut(&key).and_then(|p| p.get_mut(&start)) {
                e.tick = Some(tick);
                inner.lru.insert(tick, (key, start));
                inner.bytes += size;
            }
        }
        self.evict(inner);
    }

    /// Drops the least recently used pages nobody holds while over the budget. A held page moves to the recent end,
    /// and a pass steps over at most `EVICT_SKIPS` held pages.
    fn evict(&self, inner: &mut MainInner) {
        let mut skipped = 0;
        while inner.bytes > self.budget && skipped < EVICT_SKIPS {
            let Some((_, (key, start))) = inner.lru.pop_first() else {
                break;
            };
            let Some(pages) = inner.pages.get_mut(&key) else {
                continue;
            };
            match pages.get_mut(&start) {
                Some(e) if Arc::strong_count(&e.page) > 1 => {
                    inner.tick += 1;
                    e.tick = Some(inner.tick);
                    inner.lru.insert(inner.tick, (key, start));
                    skipped += 1;
                }
                Some(_) => {
                    if let Some(e) = pages.remove(&start) {
                        inner.bytes -= e.page.size();
                    }
                    if pages.is_empty() {
                        inner.pages.remove(&key);
                    }
                }
                None => {}
            }
        }
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.lock();
        let entries = |q: &[Queue]| q.iter().map(|q| q.pages.len()).sum();
        CacheStats {
            clean_bytes: inner.bytes,
            hot_entries: entries(&inner.hot),
            hot_bytes: inner.hot_bytes,
            hot_max_bytes: inner.hot_max_bytes,
            dirty_entries: entries(&inner.dirty),
            dirty_bytes: inner.dirty_bytes,
            dirty_version: inner.dirty_version,
            flushing_entries: inner.flushing_entries,
            flushing_bytes: inner.flushing_bytes,
        }
    }

    /// A tier's dirty queue, oldest first, as (metric, start), for tests.
    #[cfg(test)]
    pub(crate) fn dirty_queue(&self, tier: usize) -> Vec<([u8; 16], i64)> {
        self.lock().dirty[tier]
            .pages
            .values()
            .map(|(uuid, page)| (*uuid, page.start_time_s))
            .collect()
    }

    /// Pages held, for tests.
    pub fn len(&self) -> usize {
        self.lock().pages.values().map(BTreeMap::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

type ExtentKey = (usize, u32, u64);

#[derive(Debug, Default)]
struct ExtentInner {
    extents: HashMap<ExtentKey, Arc<Vec<u8>>>,
    order: VecDeque<ExtentKey>,
    bytes: usize,
}

/// The extent cache: extents as read from their data files, by tier, file number and block.
#[derive(Debug)]
pub struct ExtentCache {
    inner: Mutex<ExtentInner>,
    budget: usize,
}

impl ExtentCache {
    pub fn new(budget: usize) -> Self {
        ExtentCache {
            inner: Mutex::default(),
            budget,
        }
    }

    fn lock(&self) -> MutexGuard<'_, ExtentInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self, key: ExtentKey) -> Option<Arc<Vec<u8>>> {
        self.lock().extents.get(&key).cloned()
    }

    /// An extent read from disk; one cached meanwhile wins.
    pub fn add(&self, key: ExtentKey, bytes: Vec<u8>) -> Arc<Vec<u8>> {
        let mut inner = self.lock();
        if let Some(held) = inner.extents.get(&key) {
            return Arc::clone(held);
        }
        inner.bytes += bytes.len();
        let bytes = Arc::new(bytes);
        inner.extents.insert(key, Arc::clone(&bytes));
        inner.order.push_back(key);
        while inner.bytes > self.budget {
            let Some(old) = inner.order.pop_front() else {
                break;
            };
            if old == key {
                inner.order.push_back(old);
                break;
            }
            if let Some(e) = inner.extents.remove(&old) {
                inner.bytes -= e.len();
            }
        }
        bytes
    }
}

#[cfg(test)]
mod tests;
