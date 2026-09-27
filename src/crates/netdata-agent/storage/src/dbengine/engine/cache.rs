//! The main and extent caches (`cache.c` as the engine uses it, D62.5, D65.N3, D84, D86). The main cache indexes pages
//! by tier, metric and start whatever their state: hot pages (being collected) and dirty ones (closed, waiting for their
//! extent) sit in per-tier queues outside the LRU, as C's hot and dirty queues, and flushing ones (in an extent being
//! written) in none; clean pages (read from disk, gaps, flushed) are the LRU. The extent cache holds raw extents by
//! tier, file and block, oldest first. Both autoscale as C's: adders compute the cache's usage against the size it
//! wants and signal its evictor thread (`evict.rs`) under pressure; the evictor drops unheld clean pages, least recently
//! used or oldest first, until the cache is back to its healthy size.
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{
    AtomicBool, AtomicI64, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering,
};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError};

use super::evict::Wakeup;
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
    /// What the page counts in its cache's size and its queue; changed under the cache's lock.
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

/// The main and extent caches' clean sizes in bytes from `[db] dbengine page cache size` and `dbengine extent cache
/// size` (MiB), as `pgc_and_mrg_initialize()` splits them: 70% and 30% of the page cache, the extent share at least
/// 5 MiB (taken from the main one), plus the extent cache size.
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

/// `cache_usage_per1000()`'s thresholds (`pgc_create()`): the evictor is signalled at the severe and aggressive ones,
/// evicts above the healthy one, and each batch at most down to the low one.
pub(crate) const SEVERE: i64 = 1010;
pub(crate) const AGGRESSIVE: i64 = 990;
pub(crate) const HEALTHY: i64 = 980;
const LOW: i64 = 970;

/// `pgc_create()`'s floor of a cache's clean size.
const MIN_CLEAN_SIZE: i64 = 1024 * 1024;

/// What a cache's usage is computed from (its queues' sizes and peaks; C's `evicting` is 0 here, the victims leaving
/// the accounting as they are taken).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Sizes {
    pub hot: i64,
    pub dirty: i64,
    pub clean: i64,
    pub flushing: i64,
    pub hot_max: i64,
    pub dirty_max: i64,
}

/// What bounds a cache (`cache->config`): its clean size, and for the main cache the memory it leaves the system and
/// whether it grows into the rest (a cache with a dynamic target has neither, `pgc_set_dynamic_target_cache_size_callback()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub clean_size: i64,
    pub out_of_memory_protection: i64,
    pub use_all_ram: bool,
}

impl Limits {
    /// A cache of `clean_size` bytes (at least 1 MiB) without memory protection.
    pub fn new(clean_size: usize) -> Limits {
        Limits {
            clean_size: (clean_size as i64).max(MIN_CLEAN_SIZE),
            out_of_memory_protection: 0,
            use_all_ram: false,
        }
    }
}

/// Which threshold a usage crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pressure {
    None,
    Aggressive,
    Severe,
}

/// `cache_usage_per1000()`'s outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Usage {
    pub wanted: i64,
    pub current: i64,
    pub per1000: i64,
    /// What an evictor takes now: 0 at or below the healthy size.
    pub size_to_evict: i64,
    pub pressure: Pressure,
}

/// `pgc_threshold()`.
fn threshold(per1000: i64, wanted: i64, current: i64, clean: i64) -> i64 {
    let current = current.max(clean);
    let wanted = wanted.max(current - clean);
    (wanted.saturating_mul(per1000) / 1000).max(current - clean)
}

/// `pgc_wanted_size()`: twice the hot peak, less when flushing keeps up.
fn wanted_size(hot: i64, hot_max: i64, dirty_max: i64, index: i64) -> i64 {
    let promise = hot_max.max(hot).saturating_mul(2);
    let slow_flushing = hot_max + (dirty_max.saturating_mul(2)).max(hot_max * 2 / 3) + index;
    promise.min(slow_flushing)
}

/// `cache_usage_per1000()` of an autoscaling cache, without C's index overhead and held-page size (D84.5): `target` is
/// a dynamic target (the extent cache's), `available` the system's available memory when its total is known.
pub(crate) fn usage(s: &Sizes, l: &Limits, target: Option<i64>, available: Option<u64>) -> Usage {
    let index = 0;
    let current = s.hot + s.dirty + s.clean + s.flushing;
    let mut wanted = match target {
        Some(target) => wanted_size(s.hot, s.hot, s.dirty, index).max(target),
        None => wanted_size(s.hot, s.hot_max, s.dirty_max, index),
    };
    wanted = wanted.max(s.hot + s.dirty + index + l.clean_size);
    let min1 = s.hot + s.dirty + index;
    let min2 = if current > s.clean {
        current - s.clean
    } else {
        min1
    };
    let min_cache = min1.max(min2);
    if let (true, Some(available)) = (l.out_of_memory_protection != 0, available) {
        let available = available as i64;
        if available < l.out_of_memory_protection {
            let must_lose = l.out_of_memory_protection - available;
            wanted = if current > must_lose {
                current - must_lose
            } else {
                min_cache
            };
        } else if l.use_all_ram {
            wanted = current.saturating_add(available - l.out_of_memory_protection);
        }
    }
    // never below the minimum, nor below 64 KiB for an empty cache
    let wanted = wanted.max(min_cache).max(65536);
    let per1000 = current.saturating_mul(1000) / wanted;
    let (size_to_evict, pressure) = if current > threshold(HEALTHY, wanted, current, s.clean) {
        let low = threshold(LOW, wanted, current, s.clean);
        let pressure = if per1000 >= SEVERE {
            Pressure::Severe
        } else if per1000 >= AGGRESSIVE {
            Pressure::Aggressive
        } else {
            Pressure::None
        };
        ((current - low).min(s.clean), pressure)
    } else {
        (0, Pressure::None)
    };
    Usage {
        wanted,
        current,
        per1000,
        size_to_evict,
        pressure,
    }
}

/// `evict_pages()`' batch size after `last`: from 16 doubling to 64 under severe pressure, from 4 doubling to 16
/// under aggressive pressure, else 1.
fn batch_pages(last: usize, per1000: i64) -> usize {
    if per1000 >= SEVERE {
        (if last == 0 { 16 } else { last * 2 }).min(64)
    } else if per1000 >= AGGRESSIVE {
        (if last == 0 { 4 } else { last * 2 }).min(16)
    } else {
        1
    }
}

/// `evict_pages(cache, 0, 0, true, false)` of a cache: above its healthy size, batches of what `take` evicts (their
/// size from the usage each recomputes, C's schedule) until the cache is back to it or a batch frees nothing.
/// `usage_now` reads the sizes and computes the usage with no lock of the cache held, as C computes before it takes the
/// clean queue's lock. The per mille the pass began with; its computations signal nobody (D86.2).
fn evict_batches(usage_now: impl Fn() -> Usage, mut take: impl FnMut(usize, i64) -> usize) -> i64 {
    let first = usage_now();
    if first.per1000 < HEALTHY {
        return first.per1000;
    }
    let mut pages = 0;
    loop {
        let u = usage_now();
        if u.size_to_evict == 0 {
            break;
        }
        pages = batch_pages(pages, u.per1000);
        if take(pages, u.size_to_evict) == 0 {
            break;
        }
    }
    first.per1000
}

/// The last usage a cache computed (`cache->usage.per1000`, `stats.wanted_cache_size`, `stats.current_cache_size`),
/// behind the lock that one computation at a time takes (`cache->usage.spinlock`).
#[derive(Debug, Default)]
struct Published {
    lock: Mutex<()>,
    wanted: AtomicI64,
    current: AtomicI64,
    per1000: AtomicI64,
}

impl Published {
    fn store(&self, u: &Usage) {
        self.wanted.store(u.wanted, Ordering::Relaxed);
        self.current.store(u.current, Ordering::Relaxed);
        self.per1000.store(u.per1000, Ordering::Relaxed);
    }

    /// The evictor's computation (`cache_usage_per1000(cache, &size_to_evict)`): it waits for the lock, and signals
    /// nobody (D86.2).
    fn compute(&self, compute: impl FnOnce() -> Usage) -> Usage {
        let _computing = lock(&self.lock);
        let u = compute();
        self.store(&u);
        u
    }

    /// An adder's computation (`cache_usage_per1000(cache, NULL)`): none while another runs; the evictor signalled
    /// under pressure.
    fn note(&self, wakeup: &Wakeup, compute: impl FnOnce() -> Usage) {
        let _computing = match self.lock.try_lock() {
            Ok(guard) => guard,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return,
        };
        let u = compute();
        self.store(&u);
        if u.pressure != Pressure::None {
            wakeup.signal();
        }
    }
}

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
    /// What the clean pages take.
    bytes: usize,
    hot: [Queue; RRD_STORAGE_TIERS],
    dirty: [Queue; RRD_STORAGE_TIERS],
    seq: u64,
    hot_bytes: usize,
    /// The hot queue's peak size, updated when a page joins it (`pgc_queue_add()`).
    hot_max_bytes: usize,
    dirty_bytes: usize,
    /// The dirty queue's peak size, updated when a page joins it.
    dirty_max_bytes: usize,
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

    fn sizes(&self) -> Sizes {
        Sizes {
            hot: self.hot_bytes as i64,
            dirty: self.dirty_bytes as i64,
            clean: self.bytes as i64,
            flushing: self.flushing_bytes as i64,
            hot_max: self.hot_max_bytes as i64,
            dirty_max: self.dirty_max_bytes as i64,
        }
    }

    /// One batch of `evict_pages_with_filter()`: from the least recently used end, the clean pages nobody else holds,
    /// until `max_pages` of them or `max_size` bytes; a held one moves to the recent end, and the scan stops when it
    /// comes back to the first one it moved. The victims leave the index and the accounting.
    fn take_victims(&mut self, max_pages: usize, max_size: i64) -> Vec<Arc<CachedPage>> {
        let (mut victims, mut size, mut first_moved) = (Vec::new(), 0i64, None);
        while let Some((&tick, &(key, start))) = self.lru.first_key_value() {
            if first_moved == Some(tick) {
                break;
            }
            self.lru.remove(&tick);
            let Some(pages) = self.pages.get_mut(&key) else {
                continue;
            };
            match pages.get_mut(&start) {
                Some(e) if Arc::strong_count(&e.page) > 1 => {
                    self.tick += 1;
                    e.tick = Some(self.tick);
                    self.lru.insert(self.tick, (key, start));
                    first_moved.get_or_insert(self.tick);
                    continue;
                }
                Some(_) => {}
                None => continue,
            }
            let Some(e) = pages.remove(&start) else {
                continue;
            };
            if pages.is_empty() {
                self.pages.remove(&key);
            }
            let bytes = e.page.size();
            self.bytes -= bytes;
            size += bytes as i64;
            victims.push(e.page);
            if victims.len() >= max_pages || size >= max_size {
                break;
            }
        }
        victims
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
    pub dirty_max_bytes: usize,
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

/// The system's available memory while its total is known (`os_system_memory(false)`).
pub type SystemMemory = fn() -> Option<u64>;

/// How the caches are sized (`pgc_and_mrg_initialize()`'s inputs).
#[derive(Debug, Clone, Copy)]
pub struct CacheConfig {
    /// The clean sizes (`cache_budgets()`), floors of what each cache wants.
    pub main_clean_size: usize,
    pub extent_clean_size: usize,
    /// `dbengine_out_of_memory_protection` and `dbengine_use_all_ram_for_caches`, the main cache's.
    pub out_of_memory_protection: u64,
    pub use_all_ram: bool,
    pub system_memory: Option<SystemMemory>,
}

impl CacheConfig {
    /// Caches of these clean sizes without memory protection.
    pub fn new(main_clean_size: usize, extent_clean_size: usize) -> CacheConfig {
        CacheConfig {
            main_clean_size,
            extent_clean_size,
            out_of_memory_protection: 0,
            use_all_ram: false,
            system_memory: None,
        }
    }

    /// The main cache.
    pub fn main(&self, pages_per_extent: usize) -> MainCache {
        let limits = Limits {
            out_of_memory_protection: self.out_of_memory_protection as i64,
            use_all_ram: self.use_all_ram,
            ..Limits::new(self.main_clean_size)
        };
        MainCache::new(limits, pages_per_extent, self.system_memory)
    }
}

/// The main cache.
#[derive(Debug)]
pub struct MainCache {
    inner: Mutex<MainInner>,
    limits: Limits,
    memory: Option<SystemMemory>,
    /// `max_dirty_pages_per_call`: `rrdeng_pages_per_extent`.
    pages_per_extent: usize,
    /// `flushing_critical()`'s verdict, kept as the sizes change so that collectors and queries read it unlocked.
    critical: AtomicBool,
    usage: Published,
    wakeup: Arc<Wakeup>,
    /// Tests of other parts that want no clean pages kept: unheld ones leave whenever the usage is computed.
    #[cfg(test)]
    keep_no_clean: AtomicBool,
}

impl MainCache {
    /// A cache within `limits`, reading the system's memory through `memory` for its out of memory protection.
    pub fn new(limits: Limits, pages_per_extent: usize, memory: Option<SystemMemory>) -> Self {
        MainCache {
            inner: Mutex::default(),
            limits,
            memory,
            pages_per_extent: pages_per_extent.max(1),
            critical: AtomicBool::new(false),
            usage: Published::default(),
            wakeup: Arc::default(),
            #[cfg(test)]
            keep_no_clean: AtomicBool::new(false),
        }
    }

    /// Its evictor's signal.
    pub(crate) fn wakeup(&self) -> &Arc<Wakeup> {
        &self.wakeup
    }

    /// From now on the cache keeps no clean page nobody holds (the tests' "no main cache").
    #[cfg(test)]
    pub(crate) fn keep_no_clean_pages(&self) {
        self.keep_no_clean.store(true, Ordering::Relaxed);
    }

    /// The available memory, read with no lock of the cache held.
    fn available(&self) -> Option<u64> {
        self.memory.and_then(|f| f())
    }

    /// An adder's usage (`evict_pages_inline()` of a cache that never evicts inline) from the sizes it left, the
    /// memory read before the usage lock is tried.
    fn note_usage(&self, sizes: Sizes) {
        #[cfg(test)]
        if self.keep_no_clean.load(Ordering::Relaxed) {
            self.free_all_unreferenced_clean_pages();
        }
        let available = self.available();
        self.usage.note(&self.wakeup, || {
            usage(&sizes, &self.limits, None, available)
        });
    }

    /// `dynamic_extent_cache_size()`: 30% of what the main cache wants (at least 5 MiB), plus what it has yet to take.
    pub fn extent_target(&self) -> i64 {
        let wanted = self.usage.wanted.load(Ordering::Relaxed);
        let current = self.usage.current.load(Ordering::Relaxed);
        (wanted / 100 * 30).max(5 * 1024 * 1024) + (wanted - current).max(0)
    }

    /// `pgc_evict_thread()`'s pass over the least recently used clean pages nobody holds, dropped outside the lock.
    pub(crate) fn evict_pass(&self) -> i64 {
        evict_batches(
            || {
                let available = self.available();
                let sizes = self.lock().sizes();
                self.usage
                    .compute(|| usage(&sizes, &self.limits, None, available))
            },
            |pages, size| {
                let victims = self.lock().take_victims(pages, size);
                victims.len()
            },
        )
    }

    /// `free_all_unreferenced_clean_pages()`: every clean page nobody holds leaves.
    #[cfg(test)]
    pub(crate) fn free_all_unreferenced_clean_pages(&self) {
        let victims = self.lock().take_victims(usize::MAX, i64::MAX);
        drop(victims);
    }

    /// After the dirty size or the hot peak changed.
    fn note_sizes(&self, inner: &MainInner) {
        self.critical
            .store(inner.dirty_bytes > inner.hot_max_bytes, Ordering::Relaxed);
    }

    fn lock(&self) -> MutexGuard<'_, MainInner> {
        lock(&self.inner)
    }

    /// The walk of `mrg_metric_has_zero_disk_retention()` over a metric's pages: the earliest start above 0 of its hot
    /// and dirty pages (0 without one) and the latest end of its dirty ones (0 without one); a page being flushed is
    /// neither. Clean pages are not touched in the LRU, which only orders evictions.
    pub(crate) fn metric_span(&self, tier: usize, uuid: &[u8; 16]) -> (i64, i64) {
        let inner = self.lock();
        let (mut first, mut end) = (i64::MAX, 0);
        if let Some(pages) = inner.pages.get(&(tier, *uuid)) {
            for (&start, entry) in pages {
                let state = entry.page.state();
                if matches!(state, PageState::Hot | PageState::Dirty) && start > 0 && start < first
                {
                    first = start;
                }
                if state == PageState::Dirty {
                    end = end.max(entry.page.end_time_s());
                }
            }
        }
        (if first == i64::MAX { 0 } else { first }, end)
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
    /// untouched; neither moves in the LRU. A hot page joins its tier's hot queue, a clean one the LRU, after which the
    /// usage is computed.
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
            let conflict = Conflict {
                existing: Arc::clone(&e.page),
                page: Box::new(page),
            };
            // C computes the usage after any clean add, a conflicting one too
            if !conflict.page.is_hot() {
                let sizes = inner.sizes();
                drop(inner);
                self.note_usage(sizes);
            }
            return Err(conflict);
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
            let sizes = inner.sizes();
            drop(inner);
            self.note_usage(sizes);
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
        inner.dirty_max_bytes = inner.dirty_max_bytes.max(inner.dirty_bytes);
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
        let sizes = inner.sizes();
        drop(inner);
        self.note_usage(sizes);
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
            // the flushed pages are clean: the usage, outside the lock
            let sizes = inner.sizes();
            drop(inner);
            self.note_usage(sizes);
            inner = self.lock();
        }
        if !stopped && version_at_entry > inner.last_version_checked {
            inner.last_version_checked = version_at_entry;
        }
        stopped
    }

    /// `page_set_clean()` of a flushed batch: pages someone read become the most recently used, the others the least.
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
    }

    /// `rrdeng_pages_per_extent`.
    pub fn pages_per_extent(&self) -> usize {
        self.pages_per_extent
    }

    /// The bytes of the pages held, clean, hot, dirty and being flushed: C's `pgc_get_statistics().size` without its
    /// per-page overhead.
    pub fn bytes(&self) -> usize {
        let inner = self.lock();
        inner.bytes + inner.hot_bytes + inner.dirty_bytes + inner.flushing_bytes
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
            dirty_max_bytes: inner.dirty_max_bytes,
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

/// The extent cache: extents as read from their data files, by tier, file number and block. Its target follows the
/// main cache's (`dynamic_extent_cache_size()`), and its evictor drops the oldest extents nobody holds.
#[derive(Debug)]
pub struct ExtentCache {
    inner: Mutex<ExtentInner>,
    limits: Limits,
    usage: Published,
    wakeup: Arc<Wakeup>,
}

impl ExtentCache {
    /// A cache of at least `clean_size` bytes.
    pub fn new(clean_size: usize) -> Self {
        ExtentCache {
            inner: Mutex::default(),
            limits: Limits::new(clean_size),
            usage: Published::default(),
            wakeup: Arc::default(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ExtentInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Its evictor's signal.
    pub(crate) fn wakeup(&self) -> &Arc<Wakeup> {
        &self.wakeup
    }

    pub fn get(&self, key: ExtentKey) -> Option<Arc<Vec<u8>>> {
        self.lock().extents.get(&key).cloned()
    }

    /// The bytes of the extents held: C's extent cache `size` without its per-page overhead.
    pub fn bytes(&self) -> usize {
        self.lock().bytes
    }

    /// Its usage against `target`, the main cache's `extent_target()`: every extent is clean.
    fn usage_at(&self, bytes: usize, target: i64) -> Usage {
        let sizes = Sizes {
            clean: bytes as i64,
            ..Sizes::default()
        };
        usage(&sizes, &self.limits, Some(target), None)
    }

    /// An extent read from disk; one cached meanwhile wins. Then the usage against `target`.
    pub fn add(&self, key: ExtentKey, bytes: Vec<u8>, target: i64) -> Arc<Vec<u8>> {
        let mut inner = self.lock();
        let extent = match inner.extents.get(&key) {
            Some(cached) => Arc::clone(cached),
            None => {
                inner.bytes += bytes.len();
                let bytes = Arc::new(bytes);
                inner.extents.insert(key, Arc::clone(&bytes));
                inner.order.push_back(key);
                bytes
            }
        };
        let held = inner.bytes;
        drop(inner);
        self.usage
            .note(&self.wakeup, || self.usage_at(held, target));
        extent
    }

    /// Its evictor's pass, as the main cache's, over the oldest extents; one someone holds moves to the newest end.
    /// `target` is read for each computation, as C calls its callback.
    pub(crate) fn evict_pass(&self, target: impl Fn() -> i64) -> i64 {
        evict_batches(
            || {
                let (target, bytes) = (target(), self.bytes());
                self.usage.compute(|| self.usage_at(bytes, target))
            },
            |max, size| {
                let victims = self.lock().take_victims(max, size);
                victims.len()
            },
        )
    }
}

impl ExtentInner {
    /// One batch: the oldest extents nobody else holds, until `max` of them or `max_size` bytes; a held one moves to
    /// the newest end, and the scan stops when it comes back to the first one it moved.
    fn take_victims(&mut self, max: usize, max_size: i64) -> Vec<Arc<Vec<u8>>> {
        let (mut victims, mut size, mut first_moved) = (Vec::new(), 0i64, None);
        while let Some(key) = self.order.pop_front() {
            // back at the first one moved, whether or not it is still held
            if first_moved == Some(key) {
                self.order.push_front(key);
                break;
            }
            let Some(extent) = self.extents.get(&key) else {
                continue;
            };
            if Arc::strong_count(extent) > 1 {
                first_moved.get_or_insert(key);
                self.order.push_back(key);
                continue;
            }
            let Some(extent) = self.extents.remove(&key) else {
                continue;
            };
            self.bytes -= extent.len();
            size += extent.len() as i64;
            victims.push(extent);
            if victims.len() >= max || size >= max_size {
                break;
            }
        }
        victims
    }
}

#[cfg(test)]
mod tests;
