use std::time::Duration;

use super::*;
use crate::dbengine::format::descriptor::PAGE_TYPE_ARRAY_32BIT;

const U: [u8; 16] = [1; 16];
const MIB: i64 = 1024 * 1024;
const QUARTER: usize = 256 * 1024;

/// An empty clean page at `start` counting `size` bytes.
fn sized(start: i64, size: usize) -> CachedPage {
    CachedPage::with_state(start, start + 9, 1, PageData::Empty, size, PageState::Clean)
}

/// A cache of the 1 MiB clean size floor, without memory protection.
fn small() -> MainCache {
    MainCache::new(Limits::new(0), 109, None)
}

impl MainCache {
    /// `add()` of a page no other is cached at.
    fn add_clean(&self, tier: usize, uuid: &[u8; 16], page: CachedPage) -> Arc<CachedPage> {
        self.add(tier, uuid, page).expect("no page at its start")
    }

    fn cached(&self, start: i64) -> bool {
        self.search(0, &U, start, Search::Exact).is_some()
    }
}

/// The signals a cache's evictor got so far.
fn signals(w: &Wakeup) -> u64 {
    w.wait(u64::MAX, Duration::ZERO)
}

/// Adders never evict: over its size the cache waits for its evictor, whose pass drops the least recently used pages
/// nobody holds (a search counts as a use; a held page moves to the recent end) down to the healthy size.
#[test]
fn the_pass_drops_the_least_recently_used_unheld_pages() {
    let cache = small();
    let held = cache.add_clean(0, &U, sized(10, QUARTER));
    for start in [20, 30, 40, 50] {
        drop(cache.add_clean(0, &U, sized(start, QUARTER)));
    }
    drop(cache.search(0, &U, 20, Search::Exact));
    assert_eq!(cache.len(), 5);
    // 1.25 MiB of a wanted 1 MiB: 293602 bytes to evict, two quarters
    assert_eq!(cache.evict_pass(), 1250);
    let cached = [10, 20, 30, 40, 50].map(|start| cache.cached(start));
    assert_eq!(cached, [true, true, false, false, true]);
    drop(held);
}

/// With every clean page held the pass frees nothing; once released, the next one does.
#[test]
fn held_pages_keep_the_cache_over_its_size() {
    let cache = small();
    let held: Vec<_> = (0..5)
        .map(|i| cache.add_clean(0, &U, sized(i * 10, QUARTER)))
        .collect();
    cache.evict_pass();
    assert_eq!(cache.len(), 5);
    drop(held);
    cache.evict_pass();
    assert_eq!(cache.len(), 3);
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

/// Hot and dirty pages are never evicted (D65.N3): the pass drops clean pages alone, the hot page stays found as hot
/// and then as dirty, and the queues and their peaks count it.
#[test]
fn hot_and_dirty_pages_are_never_evicted() {
    let cache = small();
    let h = cache.add(0, &U, hot(5, 10)).unwrap();
    for start in [10, 20, 30, 40, 50] {
        drop(cache.add_clean(0, &U, sized(start, QUARTER)));
    }
    cache.evict_pass();
    assert_eq!(
        [5, 10, 20, 30, 40, 50].map(|start| cache.cached(start)),
        [true, false, false, true, true, true]
    );
    assert_eq!(
        cache.stats(),
        CacheStats {
            clean_bytes: 3 * QUARTER,
            hot_entries: 1,
            hot_bytes: 104,
            hot_max_bytes: 104,
            ..CacheStats::default()
        }
    );
    cache.hot_to_dirty(0, &h);
    drop(h);
    for start in [60, 70] {
        drop(cache.add_clean(0, &U, sized(start, QUARTER)));
    }
    cache.evict_pass();
    assert_eq!(
        cache.search(0, &U, 5, Search::Exact).unwrap().state(),
        PageState::Dirty
    );
    assert_eq!(
        cache.stats(),
        CacheStats {
            clean_bytes: 3 * QUARTER,
            hot_max_bytes: 104,
            dirty_entries: 1,
            dirty_bytes: 104,
            dirty_max_bytes: 104,
            ..CacheStats::default()
        }
    );
}

/// Dirty pages queue per tier in the order they closed; the version moves when a tier's queue reaches a multiple of
/// the pages per extent.
#[test]
fn dirty_pages_queue_per_tier_with_a_version() {
    const B: [u8; 16] = [2; 16];
    let cache = MainCache::new(Limits::new(1 << 20), 2, None);
    let close = |tier: usize, uuid: &[u8; 16], start: i64| {
        let page = cache.add(tier, uuid, hot(start, 10)).unwrap();
        cache.hot_to_dirty(tier, &page);
        cache.stats().dirty_version
    };
    assert_eq!(
        [
            close(0, &U, 10),
            close(0, &B, 10),
            close(0, &U, 20),
            close(1, &U, 10)
        ],
        [0, 1, 1, 1]
    );
    assert_eq!(cache.dirty_queue(0), [(U, 10), (B, 10), (U, 20)]);
    assert_eq!(cache.dirty_queue(1), [(U, 10)]);
}

/// A hot page closed without data leaves the cache, unless someone holds it: then it stays as a clean page, the
/// first the pass evicts.
#[test]
fn empty_hot_pages_leave_unless_held() {
    let cache = small();
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
    assert!(cache.cached(10));
    assert_eq!(
        cache.stats(),
        CacheStats {
            clean_bytes: 104,
            hot_max_bytes: 104,
            ..CacheStats::default()
        }
    );
    drop(held);
    // the least recently used: over the size, the pass drops it first
    let cache2 = small();
    drop(cache2.add_clean(0, &U, sized(20, QUARTER)));
    let page = cache2.add(0, &U, hot(10, 10)).unwrap();
    let held = Arc::clone(&page);
    cache2.to_clean_evict_or_release(0, page);
    drop(held);
    for start in [30, 40, 50, 60] {
        drop(cache2.add_clean(0, &U, sized(start, QUARTER)));
    }
    cache2.evict_pass();
    let cached = [10, 20, 30, 40].map(|start| cache2.cached(start));
    assert_eq!(cached, [false, false, false, true]);
}

/// A page added at a start already cached comes back with its data, and the cached page keeps its LRU place (C's
/// add does not count as an access).
#[test]
fn a_conflicting_add_returns_the_page_untouched() {
    let cache = small();
    for start in [10, 20, 30] {
        drop(cache.add_clean(0, &U, sized(start, QUARTER)));
    }
    let page = hot(10, 10);
    page.builder().unwrap().append(1.0, 1.0, 1.0, 1, 0, 0);
    let conflict = cache.add(0, &U, page).unwrap_err();
    assert_eq!(conflict.existing.start_time_s, 10);
    assert!(conflict.existing.is_gap() && !conflict.existing.is_hot());
    assert_eq!(conflict.page.slots_used(), 1);
    drop(conflict);
    for start in [40, 50] {
        drop(cache.add_clean(0, &U, sized(start, QUARTER)));
    }
    cache.evict_pass();
    let cached = [10, 20, 30, 40, 50].map(|start| cache.cached(start));
    assert_eq!(cached, [false, false, true, true, true]);
}

/// `cache_usage_per1000()`'s branches, with C's numbers (index 0, nothing held, D84.5).
#[test]
fn the_usage_as_c() {
    struct Case {
        sizes: Sizes,
        clean_size: i64,
        oom: i64,
        use_all_ram: bool,
        available: Option<u64>,
        want: (i64, i64, i64, i64, Pressure),
    }
    let sizes = |hot, hot_max, dirty, dirty_max, clean| Sizes {
        hot,
        dirty,
        clean,
        flushing: 0,
        hot_max,
        dirty_max,
    };
    let case = |sizes, clean_size, want| Case {
        sizes,
        clean_size,
        oom: 0,
        use_all_ram: false,
        available: None,
        want,
    };
    const GIB: i64 = 1024 * MIB;
    let cases = [
        // the hot peak's two thirds
        case(
            sizes(10 * MIB, 10 * MIB, 0, 0, 0),
            MIB,
            (17476266, 10485760, 600, 0, Pressure::None),
        ),
        // twice the hot size when above its peak
        case(
            sizes(6 * MIB, 5 * MIB, 0, 4 * MIB, 0),
            MIB,
            (12582912, 6291456, 500, 0, Pressure::None),
        ),
        // the dirty peak
        case(
            sizes(8 * MIB, 8 * MIB, 2 * MIB, 8 * MIB, 0),
            MIB,
            (16777216, 10485760, 625, 0, Pressure::None),
        ),
        // the clean size's floor, over it
        case(
            sizes(0, 0, 0, 0, 24 * MIB),
            23488080,
            (23488080, 25165824, 1071, 2382387, Pressure::Severe),
        ),
        // little memory left: shrink by what it lacks
        Case {
            oom: GIB,
            available: Some(768 * MIB as u64),
            ..case(
                sizes(100 * MIB, 100 * MIB, 0, 0, 412 * MIB),
                MIB,
                (268435456, 536870912, 2000, 276488520, Pressure::Severe),
            )
        },
        // lacking more than the cache holds: its minimum, every clean page to evict
        Case {
            oom: GIB,
            available: Some(100 * MIB as u64),
            ..case(
                sizes(100 * MIB, 100 * MIB, 0, 0, 412 * MIB),
                MIB,
                (104857600, 536870912, 5120, 432013312, Pressure::Severe),
            )
        },
        // an almost empty cache: 64 KiB
        Case {
            oom: GIB,
            available: Some(512 * MIB as u64),
            ..case(
                sizes(0, 0, 0, 0, 40000),
                MIB,
                (65536, 40000, 610, 0, Pressure::None),
            )
        },
        // use all ram: grow into what the protection leaves
        Case {
            oom: GIB,
            use_all_ram: true,
            available: Some(3 * GIB as u64),
            ..case(
                sizes(100 * MIB, 100 * MIB, 0, 0, 0),
                23488080,
                (2252341248, 104857600, 46, 0, Pressure::None),
            )
        },
        Case {
            oom: GIB,
            use_all_ram: true,
            available: Some((GIB + 100000) as u64),
            ..case(
                sizes(10 * MIB, 10 * MIB, 0, 0, 20 * MIB),
                MIB,
                (31557280, 31457280, 996, 846719, Pressure::Aggressive),
            )
        },
        // ample memory without use all ram, and unknown memory: as without protection
        Case {
            oom: GIB,
            available: Some(3 * GIB as u64),
            ..case(
                sizes(10 * MIB, 10 * MIB, 0, 0, 0),
                23488080,
                (33973840, 10485760, 308, 0, Pressure::None),
            )
        },
        Case {
            oom: GIB,
            ..case(
                sizes(10 * MIB, 10 * MIB, 0, 0, 0),
                23488080,
                (33973840, 10485760, 308, 0, Pressure::None),
            )
        },
        // hot pages alone over the wanted size: nothing to evict, no signal
        Case {
            oom: GIB,
            available: Some(512 * MIB as u64),
            ..case(
                sizes(10 * MIB, MIB, 0, 0, 0),
                MIB,
                (10485760, 10485760, 1000, 0, Pressure::None),
            )
        },
    ];
    for (i, c) in cases.iter().enumerate() {
        let limits = Limits {
            clean_size: c.clean_size,
            out_of_memory_protection: c.oom,
            use_all_ram: c.use_all_ram,
        };
        let u = usage(&c.sizes, &limits, None, c.available);
        assert_eq!(
            (u.wanted, u.current, u.per1000, u.size_to_evict, u.pressure),
            c.want,
            "case {i}"
        );
    }
}

/// Around the thresholds of a clean cache of 1 MiB, whose healthy size is 1027604 bytes.
#[test]
fn the_thresholds_as_c() {
    for (clean, want) in [
        (1027604, (979, 0, Pressure::None)),
        (1027605, (980, 10487, Pressure::None)),
        (1038090, (989, 20972, Pressure::None)),
        (1038091, (990, 20973, Pressure::Aggressive)),
        (1059061, (1009, 41943, Pressure::Aggressive)),
        (1059062, (1010, 41944, Pressure::Severe)),
    ] {
        let sizes = Sizes {
            clean,
            ..Sizes::default()
        };
        let u = usage(&sizes, &Limits::new(0), None, None);
        assert_eq!((u.per1000, u.size_to_evict, u.pressure), want, "{clean}");
    }
}

/// The batches grow as C's: from 16 doubling to 64, from 4 doubling to 16, else one page.
#[test]
fn batches_grow_as_c() {
    for (last, per1000, want) in [
        (0, 1171, 16),
        (16, 1109, 32),
        (32, 1010, 64),
        (64, 1010, 64),
        (32, 984, 1),
        (0, 995, 4),
        (4, 995, 8),
        (64, 995, 16),
        (1, 980, 1),
    ] {
        assert_eq!(batch_pages(last, per1000), want, "{last} {per1000}");
    }
}

/// A pass stops at the healthy size, not the low one: 300 pages of 4 KiB lose 50, 70 of 16 KiB lose 8.
#[test]
fn a_pass_stops_at_the_healthy_size() {
    for (count, size, left) in [(300, 4096, 250), (70, 16384, 62)] {
        let cache = small();
        for i in 0..count {
            drop(cache.add_clean(0, &U, sized(i * 10, size)));
        }
        cache.evict_pass();
        assert_eq!(cache.len(), left as usize, "{count} x {size}");
    }
}

fn no_memory() -> Option<u64> {
    Some(0)
}

/// With no memory left the cache shrinks to its minimum: every clean page goes, the hot one stays.
#[test]
fn low_memory_shrinks_the_cache_to_its_minimum() {
    let limits = Limits {
        out_of_memory_protection: 1024 * MIB,
        ..Limits::new(0)
    };
    let cache = MainCache::new(limits, 109, Some(no_memory));
    let h = CachedPage::with_state(1, 1, 1, PageData::Empty, 262144, PageState::Hot);
    let h = cache.add(0, &U, h).unwrap();
    for i in 0..64 {
        drop(cache.add_clean(0, &U, sized(10 + i * 10, 16384)));
    }
    assert_eq!(cache.evict_pass(), 5000);
    let s = cache.stats();
    assert_eq!((s.clean_bytes, s.hot_bytes), (0, 262144));
    assert_eq!(
        (
            cache.usage.wanted.load(Ordering::Relaxed),
            cache.usage.per1000.load(Ordering::Relaxed)
        ),
        (262144, 1000)
    );
    drop(h);
}

/// Adders signal the evictor once the cache is above its healthy size at the aggressive threshold or more, not
/// while another thread computes the usage.
#[test]
fn adders_signal_at_the_aggressive_threshold() {
    let cache = small();
    for i in 0..253 {
        drop(cache.add_clean(0, &U, sized(i * 10, 4096)));
    }
    // 988 per mille: above the healthy size, not signalled
    assert_eq!(signals(cache.wakeup()), 0);
    drop(cache.add_clean(0, &U, sized(2530, 4096)));
    assert_eq!(signals(cache.wakeup()), 1);
    let computing = cache.usage.lock.lock().unwrap();
    drop(cache.add_clean(0, &U, sized(2540, 4096)));
    assert_eq!(signals(cache.wakeup()), 1);
    drop(computing);
    drop(cache.add_clean(0, &U, sized(2550, 4096)));
    assert_eq!(signals(cache.wakeup()), 2);
}

/// The extent cache's target follows the main cache's (`dynamic_extent_cache_size()`), its clean size the floor.
#[test]
fn the_extent_target_as_c() {
    let extents = ExtentCache::new(10066320);
    for ((wanted, current), want) in [
        ((64 * MIB, 40 * MIB), 45298464),
        ((10 * MIB, 10 * MIB), 10066320),
        ((17476266, 10485760), 12233386),
    ] {
        let main = small();
        main.usage.store(&Usage {
            wanted,
            current,
            per1000: 0,
            size_to_evict: 0,
            pressure: Pressure::None,
        });
        assert_eq!(
            extents.usage_at(0, main.extent_target()).wanted,
            want,
            "{wanted} {current}"
        );
    }
}

/// The extent cache's pass drops the oldest extents nobody holds; adding past the target signals.
#[test]
fn the_extent_pass_drops_the_oldest_unheld_extents() {
    let extents = ExtentCache::new(0);
    let target = 5 * MIB;
    let held = extents.add((0, 1, 0), vec![0; MIB as usize], target);
    for block in 1..6 {
        drop(extents.add((0, 1, block), vec![0; MIB as usize], target));
    }
    // the fifth reaches 1000 per mille, the sixth 1200
    assert_eq!(signals(extents.wakeup()), 2);
    extents.evict_pass(target);
    let cached = [0, 1, 2, 3, 4, 5].map(|block| extents.get((0, 1, block)).is_some());
    assert_eq!(cached, [true, false, false, true, true, true]);
    drop(held);
}
