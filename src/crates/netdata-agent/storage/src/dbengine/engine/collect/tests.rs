use super::*;
use crate::dbengine::engine::cache::{CacheStats, PageState, Search};
use crate::dbengine::engine::load::{TierConfig, load};
use crate::dbengine::engine::mrg::Mrg;
use crate::dbengine::engine::query::{EngineConfig, Priority as QueryPriority};
use crate::dbengine::engine::testutil::{A, B, NOW, cfg, points, same};
use crate::dbengine::format::descriptor::{PAGE_TYPE_ARRAY_32BIT, PAGE_TYPE_GORILLA_32BIT};
use crate::storage_number::SN_DEFAULT_FLAGS;
use netdata_agent_log::Captured;
use std::path::Path;

/// `T0 % 1024 == 672`.
const T0: i64 = 1_790_180_000;
const S: u64 = 1_000_000;

/// An engine with one empty tier 0 of this page type, its clock at `NOW`.
fn engine(dir: &Path, page_type: u8) -> Arc<Dbengine> {
    let mrg = Mrg::new();
    let tier = load(TierConfig { page_type, ..cfg(dir) }, &mrg, NOW).unwrap();
    Dbengine::new(mrg, vec![tier], EngineConfig::new(|| NOW))
}

/// A collector of `uuid` in tier 0 whose pages end at slot `target` of a page.
fn collector(e: &Arc<Dbengine>, uuid: [u8; 16], update_every_s: u32, target: u64) -> CollectHandle {
    let (metric, _) = e.mrg.add_and_acquire(&uuid, 0, 0, 0, 0);
    CollectHandle::init(e, &metric, update_every_s, Alignment::from_hash(target))
}

impl CollectHandle {
    fn store(&mut self, t_s: i64, v: f64) {
        self.store_next(t_s as u64 * S, v, v, v, 1, 0, SN_DEFAULT_FLAGS);
    }

    fn store_empty(&mut self, t_s: i64) {
        self.store_next(t_s as u64 * S, f64::NAN, f64::NAN, f64::NAN, 1, 0, SN_EMPTY_SLOT);
    }

    /// The hot page: start, end, points, capacity.
    fn hot(&self) -> Option<(i64, i64, usize, usize)> {
        self.page
            .as_ref()
            .map(|p| (p.start_time_s, p.end_time_s(), p.slots_used(), self.entries_max))
    }

    fn retention(&self) -> (i64, i64) {
        let r = self.metric.retention();
        (r.first_time_s, r.last_time_s)
    }
}

/// The values of a page's points, NaN for empty ones.
fn values(page: &CachedPage, entries: usize) -> Vec<f64> {
    page.points(0, entries).into_iter().map(|(_, p)| p.sum).collect()
}

fn same_values(got: &[f64], want: &[f64]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| g == w || g.is_nan() && w.is_nan())
}

fn messages(records: Vec<Captured>) -> Vec<(Source, Priority, String)> {
    records
        .into_iter()
        .filter_map(|r| r.message.map(|m| (r.source, r.priority, m)))
        .collect()
}

/// C's slot formula: pages end at the target slot, at least a third of a page and at least 3 slots long.
#[test]
fn slots_end_pages_at_the_target() {
    let cases = [
        (1024, 77, T0, 429),
        (1024, 506, T0, 858),
        (1024, 700, T0, 341),
        (1024, 1013, T0, 341),
        (1024, 672, T0, 1024),
        (1024, 77, T0 + 429, 1024),
        (1024, 700, T0 + 341, 711),
        (128, 0, T0, 96),
        (128, 40, T0, 42),
        (24, 10, T0, 8),
        (24, 8, T0, 24),
        (24, 3, T0, 19),
        (8, 1, T0, 3),
    ];
    for (max, target, t, want) in cases {
        assert_eq!(
            page_slots(max, Alignment::from_hash(target), t),
            want,
            "{max} {target} {t}"
        );
    }
}

/// The D65.1 key: XXH3 of the host GUID, the chart id and the tier.
#[test]
fn alignments_hash_the_chart() {
    const GUID: &str = "5a1e0000-0000-4000-8000-0000000000a0";
    assert_eq!(XxHash3_64::oneshot(b""), 0x2d06_8005_38d3_94c2);
    assert_eq!(Alignment::new(GUID, "s3.c0", 0), Alignment::from_hash(0x1fe3_a4cb_7f68_b2d8));
    assert_eq!(Alignment::new(GUID, "s3.c0", 0).target(1024), 728);
    assert_eq!(Alignment::new(GUID, "s3.c0", 1).target(128), 75);
    assert_eq!(Alignment::new(GUID, "s3.c1", 0).target(1024), 180);
}

/// A page fills to its aligned size and turns dirty; the next starts a whole page that ends at the target again;
/// metrics of one alignment get the same sizes.
#[test]
fn pages_fill_then_align() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let mut a = collector(&e, A, 1, 77);
    for i in 0..429 {
        a.store(T0 + i, i as f64);
    }
    assert_eq!(a.hot(), None);
    assert_eq!(e.main.dirty_queue(0), [(A, T0)]);
    let dirty = e.main.search(0, &A, T0, Search::Exact).unwrap();
    assert_eq!(
        (dirty.end_time_s(), dirty.slots_used(), dirty.state()),
        (T0 + 428, 429, PageState::Dirty)
    );
    a.store(T0 + 429, 0.0);
    assert_eq!(a.hot(), Some((T0 + 429, T0 + 429, 1, 1024)));

    let mut b = collector(&e, B, 1, 700);
    for i in 0..1053 {
        b.store(T0 + i, 0.0);
    }
    let size = |start| e.main.search(0, &B, start, Search::Exact).unwrap().slots_used();
    assert_eq!((size(T0), size(T0 + 341)), (341, 711));
    assert_eq!(b.hot(), Some((T0 + 1052, T0 + 1052, 1, 1024)));

    let mut c = collector(&e, [0xcc; 16], 1, 77);
    c.store(T0, 0.0);
    assert_eq!(c.hot(), Some((T0, T0, 1, 429)));
}

/// `rrdeng_store_metric_next()`'s cases on an open page [T0, T0 + 9] of 429 slots.
#[test]
fn points_append_fill_gaps_or_close_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let open = |uuid: u8| {
        let mut h = collector(&e, [uuid; 16], 1, 77);
        for i in 0..10 {
            h.store(T0 + i, i as f64);
        }
        assert_eq!(h.hot(), Some((T0, T0 + 9, 10, 429)));
        h
    };

    let mut h = open(1);
    h.store(T0 + 10, 10.0);
    assert_eq!(h.hot(), Some((T0, T0 + 10, 11, 429)));

    // a gap the page holds is filled with empty points
    let mut h = open(2);
    h.store(T0 + 13, 13.0);
    assert_eq!(h.hot(), Some((T0, T0 + 13, 14, 429)));
    let page = h.page.clone().unwrap();
    let want: Vec<f64> = (0..10).map(f64::from).chain([f64::NAN; 3]).chain([13.0]).collect();
    assert!(same_values(&values(&page, 14), &want));
    assert!(page.points(10, 13).iter().all(|(_, p)| p.count == 1));
    drop(page);

    // the largest gap that fits, then the point that fills the page
    let mut h = open(3);
    h.store(T0 + 427, 427.0);
    assert_eq!(h.hot(), Some((T0, T0 + 427, 428, 429)));
    h.store(T0 + 428, 428.0);
    assert_eq!(h.hot(), None);
    assert_eq!(e.main.search(0, &[3; 16], T0, Search::Exact).unwrap().slots_used(), 429);

    // one more does not fit: the page closes and the point starts a new one
    let mut h = open(4);
    h.store(T0 + 428, 428.0);
    assert_eq!(h.hot(), Some((T0 + 428, T0 + 428, 1, 341)));
    let closed = e.main.search(0, &[4; 16], T0, Search::Exact).unwrap();
    assert_eq!((closed.state(), closed.slots_used()), (PageState::Dirty, 10));

    // older and repeated points are dropped
    let mut h = open(5);
    h.store(T0 + 9, 99.0);
    h.store(T0 + 5, 99.0);
    assert_eq!(h.hot(), Some((T0, T0 + 9, 10, 429)));
    assert_eq!(h.retention().1, T0 + 9);

    // with an update every of 10: an unaligned step and a step too small close the page
    for (uuid, next, slots, why) in [(6, 15, 414, "unaligned"), (7, 5, 424, "too small")] {
        let mut h = collector(&e, [uuid; 16], 10, 77);
        h.store(T0, 1.0);
        h.store(T0 + next, 2.0);
        assert_eq!(h.hot(), Some((T0 + next, T0 + next, 1, slots)), "{why}");
    }

    // without a page the point starts one where it is, whatever the step
    let mut h = open(8);
    h.flush_current_page();
    h.store(T0 + 16, 16.0);
    assert_eq!(h.hot(), Some((T0 + 16, T0 + 16, 1, 413)));
}

/// The first page with data sets the metric's first time to its start; a page of empty points leaves the cache and
/// sets no first time, though its points move the metric's last time.
#[test]
fn the_first_page_with_data_sets_the_retention() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let mut h = collector(&e, A, 1, 77);
    h.store_empty(T0);
    h.store(T0 + 1, 1.0);
    assert_eq!(h.retention(), (T0, T0 + 1));

    let mut h = collector(&e, B, 1, 77);
    for i in 0..3 {
        h.store_empty(T0 + i);
    }
    h.flush_current_page();
    assert!(e.main.search(0, &B, T0, Search::Exact).is_none());
    assert_eq!(e.main.dirty_queue(0), []);
    assert_eq!(e.tiers[0].samples(), 0);
    assert_eq!(h.retention(), (T0 + 2, T0 + 2));
    assert!(!h.finalize());
}

/// A page starting more than a second after now records no first time; its close sets the clean time, which then
/// is the first time too.
#[test]
fn pages_created_in_the_future_record_no_first_time() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    for (i, (from, to, want)) in [
        (NOW + 2, NOW + 6, (NOW + 6, NOW + 6)),
        (NOW - 4, NOW, (NOW - 4, NOW)),
        (NOW + 1, NOW + 3, (NOW + 1, NOW + 3)),
    ]
    .into_iter()
    .enumerate()
    {
        let mut h = collector(&e, [i as u8 + 1; 16], 1, 77);
        for t in from..=to {
            h.store(t, 1.0);
        }
        h.flush_current_page();
        assert_eq!(h.retention(), want, "{from}");
    }
}

/// A page already cached at the start: C's WARNING, and the new page one update every earlier with its first point
/// there, the gap to the next point filled.
#[test]
fn a_page_cached_at_the_start_moves_the_new_one_back() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    drop(e.main.add(0, &A, CachedPage::new(T0, T0 + 9, 0, None)).unwrap());
    let mut h = collector(&e, A, 1, 77);
    let ((), records) = netdata_agent_log::capture(|| {
        h.store(T0, 10.0);
        h.store(T0 + 1, 11.0);
    });
    assert_eq!(
        messages(records),
        [(
            Source::Daemon,
            Priority::Warning,
            "DBENGINE: metric 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa' new page from 1790180000 to 1790180000, update \
             every 1, has a conflict in main cache with existing not-hot gap page from 1790180000 to 1790180009, \
             update every 0 - is it collected more than once?"
                .to_string()
        )]
    );
    assert_eq!(h.hot(), Some((T0 - 1, T0 + 1, 3, 429)));
    let page = e.main.search(0, &A, T0 - 1, Search::Exact).unwrap();
    assert!(same_values(&values(&page, 3), &[10.0, f64::NAN, 11.0]));
    assert_eq!(h.retention(), (T0 - 1, T0 + 1));

    // two collectors of one metric: the second meets the first one's hot page
    let mut first = collector(&e, B, 1, 77);
    let mut second = collector(&e, B, 1, 77);
    first.store(T0, 1.0);
    let ((), records) = netdata_agent_log::capture(|| second.store(T0, 2.0));
    assert_eq!(
        messages(records)[0].2,
        "DBENGINE: metric 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb' new page from 1790180000 to 1790180000, update every \
         1, has a conflict in main cache with existing hot page from 1790180000 to 1790180000, update every 1 - is \
         it collected more than once?"
    );
    assert_eq!(second.hot().map(|h| h.0), Some(T0 - 1));
}

/// `finalize()`: true only for a metric without any retention; the collector count follows the handles.
#[test]
fn finalize_says_whether_the_metric_has_retention() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let h = collector(&e, A, 1, 77);
    assert_eq!(e.tiers[0].collectors_running(), 1);
    assert!(h.finalize());
    assert_eq!(e.tiers[0].collectors_running(), 0);

    let mut h = collector(&e, B, 1, 77);
    h.store(T0, 1.0);
    assert!(!h.finalize());

    let mut h = collector(&e, [0xcc; 16], 1, 77);
    h.store_empty(T0);
    assert!(!h.finalize());

    let (metric, _) = e.mrg.add_and_acquire(&[0xdd; 16], 0, T0 - 100, T0, 1);
    let h = CollectHandle::init(&e, &metric, 1, Alignment::from_hash(77));
    assert!(!h.finalize());

    // a dropped handle does finalize's work
    let mut h = collector(&e, [0xee; 16], 1, 77);
    h.store(T0, 1.0);
    drop(h);
    assert_eq!(e.tiers[0].collectors_running(), 0);
    assert_eq!(e.main.stats().hot_entries, 0);
}

/// A new update every closes the page; the next page carries it; the same one changes nothing.
#[test]
fn a_new_update_every_closes_the_page() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let mut h = collector(&e, A, 1, 77);
    for i in 0..5 {
        h.store(T0 + i, 1.0);
    }
    h.change_collection_frequency(2);
    assert_eq!(h.hot(), None);
    let closed = e.main.search(0, &A, T0, Search::Exact).unwrap();
    assert_eq!((closed.state(), closed.slots_used()), (PageState::Dirty, 5));
    assert_eq!(h.metric.update_every_s(), 2);
    h.store(T0 + 6, 1.0);
    let page = e.main.search(0, &A, T0 + 6, Search::Exact).unwrap();
    assert_eq!(page.update_every_s(), 2);
    h.change_collection_frequency(2);
    assert!(page.is_hot());
}

/// Closed pages count their intervals as samples.
#[test]
fn closed_pages_count_samples() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let mut h = collector(&e, A, 1, 77);
    for i in 0..429 {
        h.store(T0 + i, 1.0);
    }
    assert_eq!(e.tiers[0].samples(), 428);
    let mut h = collector(&e, B, 1, 77);
    h.store(T0, 1.0);
    h.change_collection_frequency(5);
    assert_eq!(e.tiers[0].samples(), 428);
}

/// Queries read the hot page as it grows, the filled gaps as empty points, for array and gorilla pages.
#[test]
fn hot_pages_answer_queries() {
    for page_type in [PAGE_TYPE_ARRAY_32BIT, PAGE_TYPE_GORILLA_32BIT] {
        let dir = tempfile::tempdir().unwrap();
        let e = engine(dir.path(), page_type);
        let mut h = collector(&e, A, 1, 77);
        let query = |h: &CollectHandle, to: i64| {
            points(&mut e.query(h.metric(), T0, T0 + to, QueryPriority::Normal, NOW))
        };
        for i in 0..10 {
            h.store(T0 + i, i as f64);
        }
        let want: Vec<(i64, f64)> = (0..10).map(|i| (T0 + i, i as f64)).collect();
        assert!(same(&query(&h, 9), &want), "{page_type}");
        for i in 10..15 {
            h.store(T0 + i, i as f64);
        }
        let want: Vec<(i64, f64)> = (0..15).map(|i| (T0 + i, i as f64)).collect();
        assert!(same(&query(&h, 14), &want), "{page_type}");
        h.store(T0 + 17, 17.0);
        let mut want = want;
        want.extend([(T0 + 15, f64::NAN), (T0 + 16, f64::NAN), (T0 + 17, 17.0)]);
        assert!(same(&query(&h, 17), &want), "{page_type}");
    }
}

/// A dirty page and the hot page after it read as one series.
#[test]
fn dirty_and_hot_pages_read_as_one_series() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let mut h = collector(&e, A, 1, 77);
    for i in 0..=500 {
        h.store(T0 + i, i as f64);
    }
    let want: Vec<(i64, f64)> = (0..=500).map(|i| (T0 + i, i as f64)).collect();
    let got = points(&mut e.query(h.metric(), T0, T0 + 500, QueryPriority::Normal, NOW));
    assert!(same(&got, &want));
}

/// A hot page of empty points: the metric's first time is its last point's, and the query reads one empty point.
#[test]
fn an_empty_hot_page_reads_one_empty_point() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_ARRAY_32BIT);
    let mut h = collector(&e, A, 1, 77);
    for i in 0..3 {
        h.store_empty(T0 + i);
    }
    let got = points(&mut e.query(h.metric(), T0, T0 + 2, QueryPriority::Normal, NOW));
    assert!(same(&got, &[(T0 + 2, f64::NAN)]), "{got:?}");
}

/// A gorilla page's new buffer counts in its queue at the append that adds it; the peak waits for the next hot page.
#[test]
fn gorilla_growth_counts_in_the_hot_queue() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path(), PAGE_TYPE_GORILLA_32BIT);
    let mut h = collector(&e, A, 1, 672);
    h.store(T0, 0.0);
    let before = e.main.stats();
    let mut i = 1;
    while e.main.stats().hot_bytes == before.hot_bytes {
        h.store(T0 + i, (i * 7919 % 1000) as f64 + 0.5);
        i += 1;
    }
    assert_eq!(
        e.main.stats(),
        CacheStats {
            hot_bytes: before.hot_bytes + 512,
            ..before
        }
    );
    assert_eq!(h.hot().unwrap().3, 1024);
}
