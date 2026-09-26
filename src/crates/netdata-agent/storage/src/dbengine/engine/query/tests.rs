use super::*;
use crate::dbengine::engine::load::load;
use crate::dbengine::engine::testutil::{A, NOW, array_page, cfg, pair, pair_with_extents};
use crate::dbengine::engine::v2index::{populate, readiness};
use crate::storage_number::{SN_DEFAULT_FLAGS, pack};
use std::path::Path;

fn values(from: u32, n: u32) -> Vec<u32> {
    (from..from + n)
        .map(|v| pack(f64::from(v), SN_DEFAULT_FLAGS))
        .collect()
}

/// An engine over one tier made of these pairs (the last one reused, with its pages open), populated.
fn engine(dir: &Path, pool: Option<WorkPool>) -> Arc<Dbengine> {
    let mrg = Mrg::new();
    let mut tier = load(cfg(dir), &mrg, NOW).unwrap();
    populate(&mut tier, &mrg, &WorkPool::new(2, 256 * 1024), 2, NOW);
    readiness(&mut tier, NOW);
    Arc::new(Dbengine {
        mrg,
        tiers: vec![TierData::new(tier)],
        main: MainCache::new(64 * 1024 * 1024),
        extents: ExtentCache::new(16 * 1024 * 1024),
        pool,
        update_every_s: 1,
    })
}

/// Every point of a query, as (end time, value or NaN); each counts as one point, empty ones too.
fn points(q: &mut Query) -> Vec<(i64, f64)> {
    let mut out = Vec::new();
    while !q.is_finished() {
        let p = q.next_metric();
        assert_eq!(p.count, 1, "at {}", p.end_time_s);
        out.push((p.end_time_s, p.sum));
    }
    out
}

fn same(got: &[(i64, f64)], want: &[(i64, f64)]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| g.0 == w.0 && ((g.1 - w.1).abs() < 1e-9 || g.1.is_nan() && w.1.is_nan()))
}

const T0: i64 = NOW - 1000;

/// Two pages of one metric with a gap between them, indexed (file 1) and open (file 2): the points follow the pages,
/// the gap reads empty, one trailing empty point ends the pages, and a window outside the retention reads nothing.
#[test]
fn points_follow_the_pages_with_gaps() {
    let dir = tempfile::tempdir().unwrap();
    pair_with_extents(dir.path(), 1, &[vec![array_page(A, T0, &values(100, 10))]]);
    pair_with_extents(
        dir.path(),
        2,
        &[vec![array_page(A, T0 + 20, &values(200, 10))]],
    );
    drop(load(cfg(dir.path()), &Mrg::new(), NOW).unwrap());
    let e = engine(dir.path(), Some(WorkPool::new(2, 256 * 1024)));
    let metric = e.mrg.get_and_acquire(&A, 0).unwrap();
    assert_eq!(
        (
            metric.retention().first_time_s,
            metric.retention().last_time_s
        ),
        (T0, T0 + 29)
    );

    let mut q = e.query(&metric, T0 + 5, T0 + 24, Priority::Normal, NOW);
    let mut want: Vec<(i64, f64)> = (5..10).map(|i| (T0 + i, 100.0 + i as f64)).collect();
    // the gap: the next page starts at T0 + 20, points jump there
    want.extend((20..25).map(|i| (T0 + i, 200.0 + (i - 20) as f64)));
    let got = points(&mut q);
    assert!(same(&got, &want), "{got:?}");
    // the gap between the pages is now a cached empty page
    assert!(
        e.main
            .search(0, &A, T0 + 10, Search::Exact)
            .is_some_and(|p| p.is_empty())
    );

    // past the last page: the window ends with the retention, so no trailing point
    let mut q = e.query(&metric, T0 + 27, T0 + 40, Priority::Synchronous, NOW);
    let got = points(&mut q);
    assert!(
        same(
            &got,
            &[(T0 + 27, 207.0), (T0 + 28, 208.0), (T0 + 29, 209.0)]
        ),
        "{got:?}"
    );

    let mut q = e.query(&metric, T0 - 100, T0 - 50, Priority::Normal, NOW);
    assert_eq!(q.end_time_s, 0, "outside the retention");
    assert!(q.is_finished());
    assert_eq!(points(&mut q), []);
}

/// A corrupt extent: its requested pages read empty, with C's records once, cached as empty so that the same query
/// again logs nothing.
#[test]
fn a_corrupt_extent_reads_empty_and_is_cached() {
    let dir = tempfile::tempdir().unwrap();
    pair_with_extents(dir.path(), 1, &[vec![array_page(A, T0, &values(100, 10))]]);
    pair(dir.path(), 2, 1, vec![]);
    drop(load(cfg(dir.path()), &Mrg::new(), NOW).unwrap());
    let data = dir.path().join("datafile-1-0000000001.ndf");
    let mut bytes = std::fs::read(&data).unwrap();
    // inside the page payload, which the CRC covers (the extent is under a block, padded)
    bytes[BLOCK_SIZE + 60] ^= 0xff;
    std::fs::write(&data, bytes).unwrap();
    let e = engine(dir.path(), None);
    let metric = e.mrg.get_and_acquire(&A, 0).unwrap();
    let (got, records) = netdata_agent_log::capture(|| {
        points(&mut e.query(&metric, T0, T0 + 9, Priority::Synchronous, NOW))
    });
    // no page to read: one trailing empty point, at the query's end
    assert!(same(&got, &[(T0 + 9, f64::NAN)]), "{got:?}");
    let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(messages.len(), 2, "{messages:?}");
    assert!(messages[0].starts_with(
        "DBENGINE: error while reading extent from datafile 1 of tier 0, at offset 4096 ("
    ));
    assert!(messages[0].contains(" bytes) to extract page (PD) from "));
    assert!(messages[0].ends_with(&format!(
        "  of metric {}: CRC32 checksum FAILED",
        uuid_text(&A)
    )));
    assert!(messages[1].starts_with(&format!(
        "DBENGINE: metric '{}' loaded invalid page of type 0 from ",
        uuid_text(&A)
    )));
    let (_, records) = netdata_agent_log::capture(|| {
        points(&mut e.query(&metric, T0, T0 + 9, Priority::Synchronous, NOW))
    });
    assert!(records.is_empty(), "the empty pages are cached");
}

/// C's choice among the pages around a time.
#[test]
fn the_page_for_a_time_is_cs() {
    let pd = |first: i64, last: i64, ue: u32| Pd {
        first_time_s: first,
        last_time_s: last,
        update_every_s: ue,
        status: 0,
        page: None,
        extent: None,
    };
    let mut list = BTreeMap::new();
    list.insert(10, pd(10, 19, 1));
    list.insert(15, pd(15, 30, 5));
    let mut gaps = 0;
    // no page starts at 16: the last one starting before it holds it
    assert_eq!(find_page_for_time(&list, 16, &mut gaps, 0, 0), Some(15));
    // 12: the next page is in the future, the previous one holds it
    assert_eq!(find_page_for_time(&list, 12, &mut gaps, 0, 0), Some(10));
    assert_eq!(gaps, 0);
    // before every page: the next one, a gap
    assert_eq!(find_page_for_time(&list, 5, &mut gaps, 0, 0), Some(10));
    // after every page: the last one, in the past, a gap
    assert_eq!(find_page_for_time(&list, 40, &mut gaps, 0, 0), Some(15));
    assert_eq!(gaps, 2);
    // a processed page stops the search back
    list.get_mut(&15).unwrap().status = PROCESSED;
    assert_eq!(find_page_for_time(&list, 20, &mut gaps, PROCESSED, 0), None);
}
