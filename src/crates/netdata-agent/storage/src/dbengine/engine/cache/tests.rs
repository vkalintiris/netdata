use super::*;

const U: [u8; 16] = [1; 16];

/// An empty page at `start`: 64 bytes against the budget.
fn gap(start: i64) -> CachedPage {
    CachedPage::new(start, start + 9, 1, None)
}

/// Over its budget the cache drops the least recently used pages, a search counting as a use, and keeps the ones
/// someone holds.
#[test]
fn eviction_drops_the_least_recently_used_unheld_pages() {
    let cache = MainCache::new(3 * 64);
    for start in [10, 20, 30] {
        drop(cache.add(0, &U, gap(start)));
    }
    let held = cache.search(0, &U, 10, Search::Exact).unwrap();
    drop(cache.search(0, &U, 20, Search::Exact));
    drop(cache.add(0, &U, gap(40)));
    // 30 was the least recently used
    let cached = |start| cache.search(0, &U, start, Search::Exact).is_some();
    assert_eq!([10, 20, 30, 40].map(cached), [true, true, false, true]);
    // 10 is held: after the searches above, 20 is the oldest unheld one
    drop(cache.add(0, &U, gap(50)));
    assert_eq!(cache.len(), 3);
    assert!(cached(10) && !cached(20));
    drop(held);
}

/// With every page held the cache stays over its budget, and an insert steps over a bounded number of them.
#[test]
fn held_pages_keep_the_cache_over_budget() {
    let cache = MainCache::new(64);
    let held: Vec<_> = (0..EVICT_SKIPS as i64 * 3)
        .map(|i| cache.add(0, &U, gap(i * 10)))
        .collect();
    assert_eq!(cache.len(), held.len());
    drop(held);
    drop(cache.add(0, &U, gap(-10)));
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
