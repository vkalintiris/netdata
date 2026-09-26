use super::*;

const A: [u8; 16] = [0xaa; 16];
const B: [u8; 16] = [0xbb; 16];

fn times(m: &Metric) -> (i64, i64, i64, u32) {
    (
        m.first_time_s.load(Ordering::Relaxed),
        m.latest_time_s_clean.load(Ordering::Relaxed),
        m.latest_time_s_hot.load(Ordering::Relaxed),
        m.update_every_s(),
    )
}

/// `mrg_unittest()`: adding a metric twice returns the one held, per tier (C's sections); lookups find it.
#[test]
fn a_metric_is_added_once_per_tier() {
    let mrg = Mrg::new();
    let (m1, added) = mrg.add_and_acquire(&A, 0, 2, 3, 4);
    assert!(added);
    let (m2, added) = mrg.add_and_acquire(&A, 0, 7, 8, 9);
    assert!(!added && std::ptr::eq(&*m1, &*m2));
    assert_eq!(times(&m2), (2, 3, 0, 4), "the second add changes nothing");
    let m3 = mrg.get_and_acquire(&A, 0).unwrap();
    assert!(std::ptr::eq(&*m1, &*m3));
    let (t1, added) = mrg.add_and_acquire(&A, 1, 2, 3, 4);
    assert!(added && !std::ptr::eq(&*m1, &*t1));
    assert!(mrg.get_and_acquire(&B, 0).is_none());
    assert_eq!(mrg.entries(), 2);
}

/// `metric_release()`: the last holder of a metric without retention removes it; one with retention stays with no
/// holder; a metric held elsewhere stays.
#[test]
fn releases_remove_metrics_without_retention() {
    let mrg = Mrg::new();
    let (kept, _) = mrg.add_and_acquire(&A, 0, 100, 200, 1);
    assert!(!kept.release());
    assert!(mrg.get_and_acquire(&A, 0).is_some(), "retention keeps it");

    let (gone, _) = mrg.add_and_acquire(&B, 0, 0, 0, 0);
    let other = gone.dup();
    assert!(!gone.release(), "still held");
    assert!(other.release());
    assert!(mrg.get_and_acquire(&B, 0).is_none());

    // dropping is releasing
    drop(mrg.add_and_acquire(&B, 1, 0, 0, 0).0);
    assert_eq!(mrg.entries(), 1);
    // first after last is no retention
    let (inverted, _) = mrg.add_and_acquire(&B, 2, 300, 200, 1);
    assert!(inverted.release());
}

/// The setters' conditions (`mrg_metric_expand_retention()`, `set_clean_latest_time_s()`, the first time's smart
/// read and its write-back, `set_first_time_s()`'s `LONG_MAX`).
#[test]
fn setters_follow_cs_conditions() {
    let mrg = Mrg::new();
    let (m, _) = mrg.add_and_acquire(&A, 0, 100, 200, 1);
    m.expand_retention(150, 180, 5);
    assert_eq!(times(&m), (100, 200, 0, 1), "inside: nothing moves");
    m.expand_retention(90, 250, 5);
    assert_eq!(times(&m), (90, 250, 0, 5));
    m.expand_retention(0, 260, 0);
    assert_eq!(
        times(&m),
        (90, 260, 0, 5),
        "no update every: the last time only"
    );
    m.expand_retention(i64::MAX, 0, 7);
    assert_eq!(
        times(&m),
        (90, 260, 0, 5),
        "an update every without a last time only when unset"
    );

    let (n, _) = mrg.add_and_acquire(&B, 0, -5, 0, 0);
    assert_eq!(times(&n), (0, 0, 0, 0), "negative times start at 0");
    n.expand_retention(0, 0, 3);
    assert_eq!(n.update_every_s(), 3);
    assert_eq!(n.first_time_s(), 0);
    assert!(n.set_hot_latest_time_s(50));
    assert_eq!(n.first_time_s(), 50, "the hot time, written back");
    assert_eq!(times(&n), (50, 0, 50, 3));
    assert!(n.set_clean_latest_time_s(40));
    assert_eq!(
        times(&n),
        (40, 40, 50, 3),
        "an earlier clean time pulls the first time"
    );
    assert_eq!(
        n.retention(),
        Retention {
            first_time_s: 40,
            last_time_s: 50,
            update_every_s: 3
        }
    );
    assert!(!n.set_first_time_s(-1) && n.set_first_time_s(i64::MAX));
    assert_eq!(n.first_time_s(), 40, "unset again, then the clean time");
    assert!(!n.set_first_time_s_if_bigger(10) && n.set_first_time_s_if_bigger(45));
    assert!(!n.set_update_every_s_if_zero(9) && n.set_update_every(9));
    n.clear_retention();
    assert!(!n.has_retention());
}

/// `mrg_metric_prepopulate()` and its cleanup: metrics of the database wait without retention; those that got
/// retention stay, the rest go, and C's record counts them.
#[test]
fn prepopulation_as_c() {
    let mrg = Mrg::new();
    let (known, _) = mrg.add_and_acquire(&B, 0, 10, 20, 1);
    mrg.prepopulate(&A, 0);
    mrg.prepopulate(&A, 1);
    mrg.prepopulate(&B, 0); // already known: not counted
    drop(known);
    assert_eq!(mrg.entries(), 3);
    mrg.get_and_acquire(&A, 1)
        .unwrap()
        .expand_retention(100, 200, 1);
    let ((), records) = netdata_agent_log::capture(|| mrg.prepopulate_cleanup());
    let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        messages,
        ["MRG DUMP: Prepopulated 2 metrics, released 1, deleted 1"]
    );
    assert!(mrg.get_and_acquire(&A, 0).is_none() && mrg.get_and_acquire(&A, 1).is_some());
    let ((), records) = netdata_agent_log::capture(|| mrg.prepopulate_cleanup());
    assert!(records.is_empty(), "nothing to report the second time");
}

/// `mrg_update_metric_retention_and_granularity_by_uuid()`: times repaired against now with C's records, a new
/// metric counted whole, a known one by the samples it gained.
#[test]
fn population_from_v2_as_c() {
    let mrg = Mrg::new();
    let (samples, records) =
        netdata_agent_log::capture(|| mrg.update_retention_by_uuid(&A, 0, 100, 400, 10, 300));
    assert_eq!(samples, 20, "last fixed to now: (300 - 100) / 10");
    let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        messages,
        ["DBENGINE JV2: wrong last time on-disk (100 - 400, now 300), fixing last time to now"]
    );
    let m = mrg.get_and_acquire(&A, 0).unwrap();
    assert_eq!(
        m.retention(),
        Retention {
            first_time_s: 100,
            last_time_s: 300,
            update_every_s: 10
        }
    );
    drop(m);
    assert_eq!(
        mrg.update_retention_by_uuid(&A, 0, 50, 300, 10, 1000),
        5,
        "five samples earlier"
    );
    assert_eq!(
        mrg.update_retention_by_uuid(&A, 0, 60, 200, 10, 1000),
        0,
        "inside"
    );
    assert_eq!(
        mrg.update_retention_by_uuid(&A, 0, 60, 200, 0, 1000),
        0,
        "no update every: no samples"
    );

    // a pre-populated metric is not new: its samples are the difference from nothing
    mrg.prepopulate(&B, 0);
    assert_eq!(mrg.update_retention_by_uuid(&B, 0, 100, 200, 10, 1000), 10);
    // first after last becomes last; the rate limit keeps the record once a second
    let (_, records) =
        netdata_agent_log::capture(|| mrg.update_retention_by_uuid(&B, 1, 500, 200, 10, 1000));
    let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        messages,
        [
            "DBENGINE JV2: wrong first time on-disk (500 - 200, now 1000), fixing first time to last time"
        ]
    );
    assert_eq!(
        mrg.get_and_acquire(&B, 1).map(|m| m.retention()),
        Some(Retention {
            first_time_s: 200,
            last_time_s: 200,
            update_every_s: 10
        })
    );
}

/// The v1 replay's view: a known metric's update every, and each replayed page added or expanded.
#[test]
fn replay_reads_and_expands_the_registry() {
    let mrg = Mrg::new();
    mrg.add_and_acquire(&A, 2, 100, 200, 60).0.release();
    let mut replay = mrg.tier(2);
    assert_eq!((replay.update_every(&A), replay.update_every(&B)), (60, 0));
    let page = |start, end, ue| ValidatedPage {
        start_time_s: start,
        end_time_s: end,
        update_every_s: ue,
        page_length: 0,
        point_size: 0,
        entries: 0,
        page_type: 0,
        valid: true,
        updated: false,
    };
    replay.replayed(&A, &page(40, 260, 60));
    replay.replayed(&B, &page(10, 20, 1));
    assert_eq!(
        mrg.get_and_acquire(&A, 2).map(|m| m.retention()),
        Some(Retention {
            first_time_s: 40,
            last_time_s: 260,
            update_every_s: 60
        })
    );
    assert_eq!(
        mrg.get_and_acquire(&B, 2).map(|m| m.retention()),
        Some(Retention {
            first_time_s: 10,
            last_time_s: 20,
            update_every_s: 1
        })
    );
}

/// Two holders releasing at once: the last one out removes a metric without retention.
#[test]
fn concurrent_releases_remove_the_metric() {
    let mrg = Mrg::new();
    for _ in 0..200 {
        let (h1, _) = mrg.add_and_acquire(&A, 0, 0, 0, 0);
        let h2 = h1.dup();
        let t = std::thread::spawn(move || drop(h2));
        drop(h1);
        t.join().unwrap();
        assert!(mrg.get_and_acquire(&A, 0).is_none());
        assert_eq!(mrg.entries(), 0);
    }
}
