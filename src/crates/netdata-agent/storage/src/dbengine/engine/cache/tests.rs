use super::*;
use crate::dbengine::format::descriptor::PAGE_TYPE_ARRAY_32BIT;

const U: [u8; 16] = [1; 16];

/// An empty page at `start`: 64 bytes against the budget.
fn gap(start: i64) -> CachedPage {
    CachedPage::new(start, start + 9, 1, None)
}

impl MainCache {
    /// `add()` of a page no other is cached at.
    fn add_clean(&self, tier: usize, uuid: &[u8; 16], page: CachedPage) -> Arc<CachedPage> {
        self.add(tier, uuid, page).expect("no page at its start")
    }
}

/// Over its budget the cache drops the least recently used pages, a search counting as a use, and keeps the ones
/// someone holds.
#[test]
fn eviction_drops_the_least_recently_used_unheld_pages() {
    let cache = MainCache::new(3 * 64, 109);
    for start in [10, 20, 30] {
        drop(cache.add_clean(0, &U, gap(start)));
    }
    let held = cache.search(0, &U, 10, Search::Exact).unwrap();
    drop(cache.search(0, &U, 20, Search::Exact));
    drop(cache.add_clean(0, &U, gap(40)));
    // 30 was the least recently used
    let cached = |start| cache.search(0, &U, start, Search::Exact).is_some();
    assert_eq!([10, 20, 30, 40].map(cached), [true, true, false, true]);
    // 10 is held: after the searches above, 20 is the oldest unheld one
    drop(cache.add_clean(0, &U, gap(50)));
    assert_eq!(cache.len(), 3);
    assert!(cached(10) && !cached(20));
    drop(held);
}

/// With every page held the cache stays over its budget, and an insert steps over a bounded number of them.
#[test]
fn held_pages_keep_the_cache_over_budget() {
    let cache = MainCache::new(64, 109);
    let held: Vec<_> = (0..EVICT_SKIPS as i64 * 3)
        .map(|i| cache.add_clean(0, &U, gap(i * 10)))
        .collect();
    assert_eq!(cache.len(), held.len());
    drop(held);
    drop(cache.add_clean(0, &U, gap(-10)));
    assert_eq!(cache.len(), 1);
}

/// C's split of the page cache size, with the extent share's floor and the extent cache size added.
#[test]
fn budgets_split_as_c() {
    const MIB: usize = 1024 * 1024;
    assert_eq!(cache_budgets(32, 0), (23_488_080, 10_066_320));
    assert_eq!(cache_budgets(8, 0), (3 * MIB, 5 * MIB));
    assert_eq!(cache_budgets(32, 16), (23_488_080, 10_066_320 + 16 * MIB));
}

/// A hot page of `slots` array slots at `start`, `64 + 4 * slots` bytes.
fn hot(start: i64, slots: usize) -> CachedPage {
    let builder = PageBuilder::new(PAGE_TYPE_ARRAY_32BIT, slots).unwrap();
    CachedPage::collected(start, 1, builder)
}

/// Hot and dirty pages are outside the LRU and the budget (D65.N3): clean pages alone are evicted, the hot page
/// stays found as hot and then as dirty, and the queues count it.
#[test]
fn hot_and_dirty_pages_are_never_evicted() {
    let cache = MainCache::new(3 * 64, 109);
    let h = cache.add(0, &U, hot(5, 10)).unwrap();
    for start in [10, 20, 30] {
        drop(cache.add_clean(0, &U, gap(start)));
    }
    drop(cache.add_clean(0, &U, gap(40)));
    let cached = |start| cache.search(0, &U, start, Search::Exact).is_some();
    assert_eq!([5, 10, 20, 30, 40].map(cached), [true, false, true, true, true]);
    assert_eq!(
        cache.stats(),
        CacheStats {
            clean_bytes: 192,
            hot_entries: 1,
            hot_bytes: 104,
            hot_max_bytes: 104,
            ..CacheStats::default()
        }
    );
    cache.hot_to_dirty(0, &h);
    drop(h);
    for start in [50, 60, 70, 80] {
        drop(cache.add_clean(0, &U, gap(start)));
    }
    assert!(cached(5));
    assert_eq!(cache.search(0, &U, 5, Search::Exact).unwrap().state(), PageState::Dirty);
    assert_eq!(
        cache.stats(),
        CacheStats {
            clean_bytes: 192,
            hot_max_bytes: 104,
            dirty_entries: 1,
            dirty_bytes: 104,
            ..CacheStats::default()
        }
    );
}

/// Dirty pages queue per tier in the order they closed; the version moves when a tier's queue reaches a multiple of
/// the pages per extent.
#[test]
fn dirty_pages_queue_per_tier_with_a_version() {
    const B: [u8; 16] = [2; 16];
    let cache = MainCache::new(1 << 20, 2);
    let close = |tier: usize, uuid: &[u8; 16], start: i64| {
        let page = cache.add(tier, uuid, hot(start, 10)).unwrap();
        cache.hot_to_dirty(tier, &page);
        cache.stats().dirty_version
    };
    assert_eq!(
        [close(0, &U, 10), close(0, &B, 10), close(0, &U, 20), close(1, &U, 10)],
        [0, 1, 1, 1]
    );
    assert_eq!(cache.dirty_queue(0), [(U, 10), (B, 10), (U, 20)]);
    assert_eq!(cache.dirty_queue(1), [(U, 10)]);
}

/// A hot page closed without data leaves the cache, unless someone holds it: then it stays as a clean page, the
/// first the budget evicts.
#[test]
fn empty_hot_pages_leave_unless_held() {
    let cache = MainCache::new(1 << 20, 109);
    let page = cache.add(0, &U, hot(10, 10)).unwrap();
    assert!(cache.to_clean_evict_or_release(0, page));
    assert!(cache.is_empty());
    // the peak stays
    assert_eq!(
        cache.stats(),
        CacheStats {
            hot_max_bytes: 104,
            ..CacheStats::default()
        }
    );

    let page = cache.add(0, &U, hot(10, 10)).unwrap();
    let held = Arc::clone(&page);
    assert!(!cache.to_clean_evict_or_release(0, page));
    assert_eq!(held.state(), PageState::Clean);
    assert!(cache.search(0, &U, 10, Search::Exact).is_some());
    assert_eq!(
        cache.stats(),
        CacheStats {
            clean_bytes: 104,
            hot_max_bytes: 104,
            ..CacheStats::default()
        }
    );
    drop(held);
    // the least recently used: a new page over the budget drops it first
    let cache2 = MainCache::new(104 + 64, 109);
    drop(cache2.add_clean(0, &U, gap(20)));
    let page = cache2.add(0, &U, hot(10, 10)).unwrap();
    let held = Arc::clone(&page);
    cache2.to_clean_evict_or_release(0, page);
    drop(held);
    drop(cache2.add_clean(0, &U, gap(30)));
    let cached = |start| cache2.search(0, &U, start, Search::Exact).is_some();
    assert_eq!([10, 20, 30].map(cached), [false, true, true]);
}

/// A page added at a start already cached comes back with its data, and the cached page keeps its LRU place (C's
/// add does not count as an access).
#[test]
fn a_conflicting_add_returns_the_page_untouched() {
    let cache = MainCache::new(3 * 64, 109);
    for start in [10, 20, 30] {
        drop(cache.add_clean(0, &U, gap(start)));
    }
    let page = hot(10, 10);
    page.builder().unwrap().append(1.0, 1.0, 1.0, 1, 0, 0);
    let conflict = cache.add(0, &U, page).unwrap_err();
    assert_eq!(conflict.existing.start_time_s, 10);
    assert!(conflict.existing.is_gap() && !conflict.existing.is_hot());
    assert_eq!(conflict.page.slots_used(), 1);
    drop(conflict);
    drop(cache.add_clean(0, &U, gap(40)));
    let cached = |start| cache.search(0, &U, start, Search::Exact).is_some();
    assert_eq!([10, 20, 30, 40].map(cached), [false, true, true, true]);
}
