use std::sync::Arc;

use super::*;
use crate::dbengine::engine::index::journal_index;
use crate::dbengine::engine::testutil::{
    NOW, dirty_page, messages, nth, restarted_engine, seq_values, write_engine,
};

const T0: i64 = NOW - 10_000;

/// A one-page extent of `uuid` from `start`, 1024 points.
fn extent(e: &Arc<Dbengine>, uuid: [u8; 16], start: i64) -> Handle {
    let metric = dirty_page(e, 0, uuid, start, &seq_values(1024, 0));
    e.flush_pages(0, None, true, true);
    metric
}

/// One-page extents of fresh metrics until extents go to file `fileno`.
fn fill_to(e: &Arc<Dbengine>, fileno: u32, next: &mut usize) -> Vec<Handle> {
    let mut held = Vec::new();
    while e.tiers[0].last_file().fileno < fileno {
        held.push(extent(e, nth(*next), T0));
        *next += 1;
    }
    held
}

/// The earliest start the remaining files give `uuid`: its pages in every serving v2 index and its open pages (the
/// exact recalculation D30 asks for).
fn exact_first(td: &TierData, uuid: &[u8; 16]) -> Option<i64> {
    let from_v2 = td
        .v2_from(0)
        .iter()
        .filter_map(|i| {
            i.find(uuid)
                .ok()
                .flatten()
                .map(|m| i.start_time_s() + i64::from(m.delta_start_s))
        })
        .min();
    let from_open = td
        .open()
        .pages(uuid)
        .and_then(|p| p.first_key_value().map(|(s, _)| *s));
    from_v2.into_iter().chain(from_open).min()
}

/// Three files: A and B (the highest id, so the last entry of every list) in file 1 from T0, in file 2 from T0+1024
/// and in file 3 (the last one, open pages) from T0+2048; files 1 and 2 indexed, then a restart, which loads them
/// from their v2 indexes. A deletion of file 1 then gives A and B the exact first time of the files left (file 2's
/// page). C's walk skips B in file 2, and with no clean page of file 2 in its open cache after the restart, C dates
/// B from its open page in file 3 (D30). File 1's other metrics, only there, leave the registry unless held.
#[test]
fn a_deletion_recalculates_the_first_times_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let (a, b) = (nth(0), nth(1 << 20));
    let fillers: Vec<[u8; 16]> = {
        let e = write_engine(&[dir.path()], 0, None);
        let mut next = 1;
        let _ab = (extent(&e, a, T0), extent(&e, b, T0));
        let held = fill_to(&e, 2, &mut next);
        extent(&e, a, T0 + 1024);
        extent(&e, b, T0 + 1024);
        let _more = fill_to(&e, 3, &mut next);
        extent(&e, a, T0 + 2048);
        extent(&e, b, T0 + 2048);
        let ((), _) = netdata_agent_log::capture(|| assert_eq!(journal_index(&e, 0), 2));
        held.iter().map(|h| *h.uuid()).collect()
    };
    let e = restarted_engine(dir.path());
    let td = &e.tiers[0];
    let (ha, hb) = (
        e.mrg.get_and_acquire(&a, 0).unwrap(),
        e.mrg.get_and_acquire(&b, 0).unwrap(),
    );
    assert_eq!((ha.first_time_s(), hb.first_time_s()), (T0, T0));
    // every filler but the first is held
    let held: Vec<Handle> = fillers[1..]
        .iter()
        .map(|u| e.mrg.get_and_acquire(u, 0).unwrap())
        .collect();
    let f1 = td.file(1).unwrap();
    let (pos, journal, v2) = (f1.pos(), f1.journal_pos(), td.v2_of(1).unwrap().size);
    let (count, disk, samples) = (
        td.v2_of(1).unwrap().header.metric_count,
        td.current_disk_space(),
        td.samples(),
    );
    let ((), records) = netdata_agent_log::capture(|| datafile_delete(&e, 0, &f1, true));
    let reclaimed = pos + journal + v2;
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000001 is pending deletion".to_string(),
            format!(
                "DBENGINE: tier 0: recalculating retention for {count} metrics starting with datafile 2"
            ),
            format!("DBENGINE: tier 0: updating metrics registry retention for {count} metrics"),
            "DBENGINE: tier 0: deleting datafile-1-0000000001 to maintain disk quota.".to_string(),
            format!(
                "DBENGINE: tier 0: deleted datafile-1-0000000001 (.ndf, .njf, .njfv2), reclaimed {}.",
                size_to_string(reclaimed, "B", false).unwrap()
            ),
        ],
        "loaded from its v2 index, nothing holds the file: no phase 2"
    );
    assert_eq!(td.filenos(), [2, 3]);
    for kind in [FileKind::Datafile, FileKind::Journal, FileKind::JournalV2] {
        assert!(!td.config.file(kind, 1).exists(), "{kind:?}");
    }
    assert_eq!(td.current_disk_space(), disk - reclaimed);
    assert_eq!(
        (ha.first_time_s(), hb.first_time_s()),
        (T0 + 1024, T0 + 1024),
        "B is file 2's last entry: C dates it from its open page, T0 + 2048"
    );
    assert_eq!(exact_first(td, &a), Some(T0 + 1024));
    assert_eq!(exact_first(td, &b), Some(T0 + 1024));
    assert_eq!(td.first_time_s(), td.v2_of(2).unwrap().start_time_s());
    assert_eq!(
        td.samples(),
        samples - 2 * 1024,
        "A and B lost 1024 one-second samples each; the metrics without data left take none"
    );
    // file 1's other metrics have no data left: the unheld one left the registry, the held ones stay
    assert!(e.mrg.get_and_acquire(&fillers[0], 0).is_none());
    assert!(
        held.iter()
            .filter(|h| exact_first(td, h.uuid()).is_none())
            .all(|h| e.mrg.get_and_acquire(h.uuid(), 0).is_some())
    );
}

/// A file whose open pages all belong to metrics that left the registry writes no v2 index: its pages turn clean,
/// as C's rejected pages, so a deletion evicts them and takes the file, as one without an index (no recalculation).
#[test]
fn a_file_of_rejected_pages_is_deleted_as_one_without_an_index() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let mut next = 0;
    let gone = fill_to(&e, 2, &mut next);
    let _held = fill_to(&e, 3, &mut next);
    let td = &e.tiers[0];
    // file 1's metrics leave the registry (the rotation's zero-retention path while their pages were flushed)
    for h in gone {
        h.clear_retention();
        assert!(h.release());
    }
    let (count, _) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(count, 2, "files 1 and 2 processed");
    assert!(td.v2_of(1).is_none() && td.v2_of(2).is_some());
    let f1 = td.file(1).unwrap();
    assert_eq!(
        (td.open().current_pages_of(1), td.open_lockers(&f1)),
        (0, 1),
        "no hot page left, the clean pages hold the file"
    );
    let (pos, journal, disk) = (f1.pos(), f1.journal_pos(), td.current_disk_space());
    let ((), records) = netdata_agent_log::capture(|| datafile_delete(&e, 0, &f1, true));
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000001 is pending deletion".to_string(),
            "DBENGINE: tier 0: datafile-1-0000000001 entered deletion phase-2 (new users blocked)"
                .to_string(),
            "DBENGINE: tier 0: deleting datafile-1-0000000001 to maintain disk quota.".to_string(),
            format!(
                "DBENGINE: tier 0: deleted datafile-1-0000000001 (.ndf, .njf), reclaimed {}.",
                size_to_string(pos + journal, "B", false).unwrap()
            ),
        ]
    );
    assert_eq!(td.filenos(), [2, 3]);
    assert_eq!(td.current_disk_space(), disk - pos - journal);
}

/// A file only a query walked holds the query's clean pages in C's open cache: a deletion evicts them, blocks the file
/// (phase 2) and takes it. Once the file takes no uses, a query for a metric the main cache does not hold finds nothing
/// there and adds no clean page of it.
#[test]
fn a_query_walk_holds_a_file_until_its_pages_are_evicted() {
    use crate::dbengine::engine::testutil::points;
    use crate::query::Priority;
    let dir = tempfile::tempdir().unwrap();
    {
        let e = write_engine(&[dir.path()], 0, None);
        let mut next = 0;
        let _held = fill_to(&e, 3, &mut next);
        let ((), _) = netdata_agent_log::capture(|| assert_eq!(journal_index(&e, 0), 2));
    }
    let e = restarted_engine(dir.path());
    let td = &e.tiers[0];
    let f1 = td.file(1).unwrap();
    assert_eq!(td.open_lockers(&f1), 0, "loaded from its v2 index");
    let walk = |n| {
        let metric = e.mrg.get_and_acquire(&nth(n), 0).unwrap();
        points(&mut e.query(&metric, T0, T0 + 1023, Priority::Synchronous))
            .iter()
            .filter(|(_, v)| !v.is_nan())
            .count()
    };
    assert_eq!(walk(0), 1024);
    assert_eq!(td.open_lockers(&f1), 1, "the walk's clean pages");
    let (deletable, records) = netdata_agent_log::capture(|| td.acquire_for_deletion(&f1));
    assert!(deletable);
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: datafile-1-0000000001 is pending deletion",
            "DBENGINE: tier 0: datafile-1-0000000001 entered deletion phase-2 (new users blocked)"
        ]
    );
    // another metric of the file, which the main cache does not hold (C purges no cache at a deletion)
    assert_eq!(walk(1), 0, "the file takes no uses");
    assert_eq!(td.open_lockers(&f1), 0, "nor clean pages");
}

/// A metric no remaining file holds leaves the registry once nobody holds it; one with a dirty page in the main cache
/// keeps that page's retention (`mrg_metric_has_zero_disk_retention()`).
#[test]
fn a_metric_without_retention_left_leaves_the_registry() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let (gone, dirty) = (nth(0), nth(1));
    drop(extent(&e, gone, T0));
    drop(extent(&e, dirty, T0));
    let mut next = 2;
    let held = fill_to(&e, 2, &mut next);
    let ((), _) = netdata_agent_log::capture(|| assert_eq!(journal_index(&e, 0), 1));
    let kept = dirty_page(&e, 0, dirty, T0 + 5000, &seq_values(10, 0));
    let td = &e.tiers[0];
    let ((), _) = netdata_agent_log::capture(|| datafile_delete(&e, 0, &td.file(1).unwrap(), true));
    assert!(e.mrg.get_and_acquire(&gone, 0).is_none());
    let r = kept.retention();
    assert_eq!(
        (r.first_time_s, kept.latest_clean_time_s()),
        (T0 + 5000, T0 + 5009)
    );
    drop(held);
}

/// A file that cannot be taken (its hot pages) is retried 30 times, recorded 29 times, and left pending; a missing
/// `.njf` makes a partial deletion; a shutting-down tier skips the recalculation.
#[test]
fn deletions_wait_give_up_and_report_partial_results() {
    let dir = tempfile::tempdir().unwrap();
    let e = write_engine(&[dir.path()], 0, None);
    let mut next = 0;
    let _held = fill_to(&e, 3, &mut next);
    let td = &e.tiers[0];
    // file 1 is not indexed: its hot pages hold it
    let f1 = td.file(1).unwrap();
    let hot = td.open().current_pages_of(1);
    let ((), records) = netdata_agent_log::capture(|| datafile_delete(&e, 0, &f1, true));
    let records = messages(records);
    assert_eq!(records.len(), 2 + 29 + 1);
    assert_eq!(
        records[2],
        format!(
            "DBENGINE: tier 0: waiting for datafile-1-0000000001 to be available for deletion, in use by {hot} users."
        )
    );
    assert_eq!(
        records[31],
        format!(
            "DBENGINE: tier 0: datafile-1-0000000001 could not be acquired for deletion after 30 attempts ({hot} \
             lockers remain) - will retry on next rotation"
        )
    );
    assert_eq!(td.filenos(), [1, 2, 3]);
    assert!(f1.pending_deletion());

    // file 2 indexed, its .njf gone, the tier shutting down
    let f2 = td.file(2).unwrap();
    let ((), _) = netdata_agent_log::capture(|| {
        let use_ = td.next_for_indexing(Some(1)).unwrap();
        assert_eq!(use_.fileno, 2);
    });
    let (indexed, _) = netdata_agent_log::capture(|| journal_index(&e, 0));
    assert_eq!(indexed, 1, "file 1 passed over, file 2 indexed");
    std::fs::remove_file(td.config.file(FileKind::Journal, 2)).unwrap();
    let (pos, v2) = (f2.pos(), td.v2_of(2).unwrap().size);
    td.quiesce();
    let ((), records) = netdata_agent_log::capture(|| datafile_delete(&e, 0, &f2, !td.quiesced()));
    assert_eq!(
        messages(records)[2..],
        [
            "DBENGINE: tier 0: deleting datafile-1-0000000002 to maintain disk quota.".to_string(),
            format!(
                "DBENGINE: tier 0: partial delete of datafile-1-0000000002 - removed: .ndf, .njfv2, failed: .njf, \
                 reclaimed {}.",
                size_to_string(pos + v2, "B", false).unwrap()
            ),
        ]
    );
}

/// `database_rotate_tp_worker()`: the oldest pair goes, then the rotation hook runs; it also runs after a deletion
/// that gave up (a use of the file held through its 30 attempts).
#[test]
fn a_rotation_deletes_the_oldest_pair_then_calls_the_hook() {
    use crate::dbengine::engine::load::load;
    use crate::dbengine::engine::mrg::Mrg;
    use crate::dbengine::engine::query::{EngineConfig, RotationHook};
    use crate::dbengine::engine::testutil::write_cfg;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tempfile::tempdir().unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let mrg = Mrg::new();
    let tiers = vec![load(write_cfg(0, dir.path()), &mrg, NOW).unwrap()];
    let e = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            main_cache_bytes: 0,
            rotation: Some(RotationHook(Arc::new(move || {
                counted.fetch_add(1, Ordering::Relaxed);
            }))),
            ..EngineConfig::new(|| NOW)
        },
    );
    let mut next = 0;
    let _held = fill_to(&e, 3, &mut next);
    let td = &e.tiers[0];
    let ((), _) = netdata_agent_log::capture(|| assert_eq!(journal_index(&e, 0), 2));
    // a query's use of file 1 holds it: the deletion gives up
    let use_ = td.acquire(1, Reason::PageDetails).unwrap();
    let ((), _) = netdata_agent_log::capture(|| database_rotate(&e, 0));
    assert_eq!(
        (td.filenos(), calls.load(Ordering::Relaxed)),
        (vec![1, 2, 3], 1)
    );
    // released, it goes
    drop(use_);
    let ((), _) = netdata_agent_log::capture(|| database_rotate(&e, 0));
    assert_eq!(
        (td.filenos(), calls.load(Ordering::Relaxed)),
        (vec![2, 3], 2)
    );
}

/// `rrdeng_retention_samples_delta()`: the intervals between the times, none for an unknown, empty or inverted one.
#[test]
fn retention_samples_are_intervals() {
    for (first, last, ue, want) in [
        (100, 160, 10, 6),
        (100, 165, 10, 6),
        (0, 160, 10, 0),
        (100, 0, 10, 0),
        (100, 100, 10, 0),
        (100, 160, 0, 0),
    ] {
        assert_eq!(
            retention_samples_delta(0, first, last, ue, "r"),
            want,
            "{first} {last} {ue}"
        );
    }
    let (n, records) =
        netdata_agent_log::capture(|| retention_samples_delta(0, 160, 100, 10, "testing"));
    assert_eq!(n, 0);
    assert_eq!(
        messages(records),
        [
            "DBENGINE: tier 0: invalid retention interval while testing (first=160, last=100, update_every=10); not \
          updating sample counter"
        ]
    );
}
