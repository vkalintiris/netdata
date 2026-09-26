//! The read path's caches (`cache.c` as the main and extent caches use it, D62.5): pages by tier, metric and start,
//! including empty gap pages (`PGD_EMPTY`), and raw extents by tier, file and block. Both hold what they are given
//! within a byte budget, dropping the oldest entries nobody holds; evictor threads, autoscaling and memory pressure
//! wait for S6.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::dbengine::format::page::DiskPage;

/// A cached page: its times (the end and update every can be repaired by queries, as C repairs them in the cache),
/// and its data, `None` for an empty page (a gap, or a page that failed to load).
#[derive(Debug)]
pub struct CachedPage {
    pub start_time_s: i64,
    end_time_s: AtomicI64,
    update_every_s: AtomicU32,
    pub data: Option<DiskPage>,
    /// What the page counts against the budget.
    size: usize,
}

impl CachedPage {
    pub fn new(
        start_time_s: i64,
        end_time_s: i64,
        update_every_s: u32,
        data: Option<DiskPage>,
    ) -> Self {
        let size = 64 + data.as_ref().map_or(0, |d| d.slots_used() * 4);
        CachedPage {
            start_time_s,
            end_time_s: AtomicI64::new(end_time_s),
            update_every_s: AtomicU32::new(update_every_s),
            data,
            size,
        }
    }

    pub fn end_time_s(&self) -> i64 {
        self.end_time_s.load(Ordering::Acquire)
    }

    pub fn update_every_s(&self) -> u32 {
        self.update_every_s.load(Ordering::Acquire)
    }

    /// `pgd_is_empty()` of the page's data.
    pub fn is_empty(&self) -> bool {
        self.data.as_ref().is_none_or(DiskPage::is_empty)
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

type PageKey = (usize, [u8; 16]);

#[derive(Debug, Default)]
struct MainInner {
    pages: HashMap<PageKey, BTreeMap<i64, Arc<CachedPage>>>,
    order: VecDeque<(PageKey, i64)>,
    bytes: usize,
}

/// The main cache.
#[derive(Debug)]
pub struct MainCache {
    inner: Mutex<MainInner>,
    budget: usize,
}

impl MainCache {
    pub fn new(budget: usize) -> Self {
        MainCache {
            inner: Mutex::default(),
            budget,
        }
    }

    fn lock(&self) -> MutexGuard<'_, MainInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `pgc_page_get_and_acquire()`.
    pub fn search(
        &self,
        tier: usize,
        uuid: &[u8; 16],
        time_s: i64,
        mode: Search,
    ) -> Option<Arc<CachedPage>> {
        let inner = self.lock();
        let pages = inner.pages.get(&(tier, *uuid))?;
        let next = || pages.range(time_s + 1..).next().map(|(_, p)| Arc::clone(p));
        match mode {
            Search::Exact => pages.get(&time_s).cloned(),
            Search::Next => next(),
            Search::Closest => pages
                .get(&time_s)
                .cloned()
                .or_else(|| {
                    pages
                        .range(..time_s)
                        .next_back()
                        .filter(|(_, p)| time_s <= p.end_time_s())
                        .map(|(_, p)| Arc::clone(p))
                })
                .or_else(next),
        }
    }

    /// `pgc_page_add_and_acquire()`: a page already cached at the same start wins over the new one.
    pub fn add(&self, tier: usize, uuid: &[u8; 16], page: CachedPage) -> Arc<CachedPage> {
        let mut inner = self.lock();
        let key = (tier, *uuid);
        if let Some(held) = inner
            .pages
            .get(&key)
            .and_then(|p| p.get(&page.start_time_s))
        {
            return Arc::clone(held);
        }
        let start = page.start_time_s;
        inner.bytes += page.size;
        let page = Arc::new(page);
        inner
            .pages
            .entry(key)
            .or_default()
            .insert(start, Arc::clone(&page));
        inner.order.push_back((key, start));
        self.evict(&mut inner);
        page
    }

    /// Drops the oldest pages nobody holds while over the budget.
    fn evict(&self, inner: &mut MainInner) {
        let mut kept = VecDeque::new();
        while inner.bytes > self.budget {
            let Some((key, start)) = inner.order.pop_front() else {
                break;
            };
            let removable = inner
                .pages
                .get(&key)
                .and_then(|p| p.get(&start))
                .is_some_and(|p| Arc::strong_count(p) == 1);
            if !removable {
                kept.push_back((key, start));
                continue;
            }
            if let Some(pages) = inner.pages.get_mut(&key) {
                if let Some(p) = pages.remove(&start) {
                    inner.bytes -= p.size;
                }
                if pages.is_empty() {
                    inner.pages.remove(&key);
                }
            }
        }
        while let Some(entry) = kept.pop_back() {
            inner.order.push_front(entry);
        }
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
