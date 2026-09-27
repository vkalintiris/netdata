use super::*;
use crate::dbengine::engine::query::{DEFAULT_PAGES_PER_EXTENT, Priority};
use crate::dbengine::engine::testutil::{A, B, NOW, array_page, cfg, pair_with_extents, points};
use crate::storage_number::{SN_DEFAULT_FLAGS, pack};
use netdata_agent_evloop::work::WorkPool;
use std::path::Path;
use std::sync::atomic::AtomicUsize;

const T0: i64 = NOW - 1000;

fn init(dirs: &[Option<&Path>]) -> InitConfig {
    InitConfig {
        host: "h".into(),
        cache_dir: "/c".into(),
        tiers: dirs
            .iter()
            .enumerate()
            .map(|(t, d)| {
                d.map(|d| TierConfig {
                    tier: t,
                    page_type: TierConfig::default_page_type(t),
                    ..cfg(d)
                })
            })
            .collect(),
        cpus: 4,
        nofile_limit: 1 << 20,
        main_cache_bytes: 1 << 24,
        extent_cache_bytes: 1 << 22,
        pages_per_extent: DEFAULT_PAGES_PER_EXTENT,
        update_every_s: 1,
        stack_size: 256 * 1024,
        timer_period: Duration::from_millis(10),
        rotation: None,
        retention_tiers: vec![true; dirs.len()],
    }
}

/// Tier 0 holds ten points of A from `T0`.
fn tier0_with_a(dir: &Path) {
    let values: Vec<u32> = (0..10)
        .map(|v| pack(f64::from(v), SN_DEFAULT_FLAGS))
        .collect();
    pair_with_extents(dir, 1, &[vec![array_page(A, T0, &values)]]);
}

fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
    records.into_iter().filter_map(|r| r.message).collect()
}

/// Three tiers starting in parallel: the pre-population runs once, before any tier's files are loaded, for every
/// tier; tiers without retention are ready from now; "mrg cleanup" releases what the files did not claim.
#[test]
fn the_first_tier_prepopulates_every_tier_once() {
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    tier0_with_a(dirs[0].path());
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&calls);
    let prepopulate: Prepopulate = Box::new(move |cb| {
        counted.fetch_add(1, Ordering::SeqCst);
        cb(&A);
        cb(&B);
    });
    let paths: Vec<_> = dirs.iter().map(|d| Some(d.path())).collect();
    let rt = Runtime::start(
        init(&paths),
        &WorkPool::new(4, 256 * 1024),
        prepopulate,
        || NOW,
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(rt.storage_tiers(), 3);
    let first: Vec<i64> = rt.engine().tiers.iter().map(|t| t.first_time_s()).collect();
    assert_eq!(first, [T0, NOW, NOW]);
    let ((), records) = netdata_agent_log::capture(|| rt.engine().mrg.prepopulate_cleanup());
    // A in tier 0 was prepopulated before tier 0's load gave it retention: released, not deleted
    assert_eq!(
        messages(records),
        ["MRG DUMP: Prepopulated 6 metrics, released 1, deleted 5"]
    );
    rt.exit();
}

/// A tier that fails ends the tiers in use there, with C's records on the caller's thread; a tier without its
/// directory counts as failed.
#[test]
fn a_failed_tier_limits_the_tiers_in_use() {
    let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
    let not_a_dir = dirs[1].path().join("file");
    std::fs::write(&not_a_dir, b"").unwrap();
    let (rt, records) = netdata_agent_log::capture(|| {
        Runtime::start(
            init(&[Some(dirs[0].path()), Some(&not_a_dir), None]),
            &WorkPool::new(2, 256 * 1024),
            Box::new(|_| {}),
            || NOW,
        )
    });
    assert_eq!(rt.storage_tiers(), 1);
    assert_eq!(
        messages(records),
        [
            format!(
                "DBENGINE on 'h': Failed to initialize multi-host database tier 1 on path '{}'",
                not_a_dir.display()
            ),
            "DBENGINE on 'h': Managed to create 1 tiers instead of 3. Continuing with 1 available."
                .to_string(),
            "DBENGINE: tier 0: ready for data collection and queries".to_string(),
        ]
    );
    assert_eq!(created_tiers(&[true, false, true]), 1);
    assert_eq!(created_tiers(&[false, true]), 0);
    rt.exit();
}

/// After quiesce new queries read nothing but their trailing point, and the exit waits for the queries in flight.
#[test]
fn quiesce_stops_queries_and_exit_waits_for_them() {
    let dir = tempfile::tempdir().unwrap();
    tier0_with_a(dir.path());
    let rt = Runtime::start(
        init(&[Some(dir.path())]),
        &WorkPool::new(2, 256 * 1024),
        Box::new(|_| {}),
        || NOW,
    );
    let engine = Arc::clone(rt.engine());
    let metric = engine.mrg.get_and_acquire(&A, 0).unwrap();
    let mut held = engine.query(&metric, T0, T0 + 9, Priority::Normal);
    rt.quiesce();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !engine.tiers[0].quiesced() {
        assert!(std::time::Instant::now() < deadline, "quiesce");
        std::thread::sleep(Duration::from_millis(1));
    }
    let got = points(&mut engine.query(&metric, T0, T0 + 9, Priority::Normal));
    assert!(
        got.len() == 1 && got[0].0 == T0 + 9 && got[0].1.is_nan(),
        "{got:?}"
    );
    // the query started before the quiesce still reads its points
    assert_eq!(points(&mut held).len(), 10);
    let exiting = std::thread::spawn(move || rt.exit());
    std::thread::sleep(Duration::from_millis(50));
    assert!(!exiting.is_finished(), "a query is in flight");
    drop(held);
    exiting.join().unwrap();
}

/// A running engine over one tier of array pages with 524,288-byte data files, its timer at 10 ms.
fn running(dir: &Path, pool: &WorkPool) -> Runtime {
    let mut cfg = init(&[Some(dir)]);
    cfg.tiers = vec![Some(crate::dbengine::engine::testutil::write_cfg(0, dir))];
    Runtime::start(cfg, pool, Box::new(|_| {}), || NOW)
}

/// Waits up to 10 s for `done`.
fn eventually(mut done: impl FnMut() -> bool) -> bool {
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until {
        if done() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

/// `DBEV`'s decisions: flushers up to one per CPU; one index queued at a time and one running, none while shutting
/// down.
#[test]
fn dbev_schedules_as_c() {
    let mut s = Sched {
        cpus: 2,
        tiers: vec![TierSched::default(); 2],
        ..Sched::default()
    };
    assert_eq!(
        [s.flush_main(), s.flush_main(), s.flush_main()],
        [true, true, false]
    );
    s.flush_done();
    assert!(s.flush_main());

    assert!(!s.check_and_schedule(0, false));
    assert!(s.check_and_schedule(0, true));
    assert!(!s.check_and_schedule(0, true), "already queued");
    assert!(s.journal_index(0, false));
    assert!(s.check_and_schedule(0, true), "queued again while running");
    assert!(!s.journal_index(0, false), "one runs at a time");
    s.index_done(0);
    assert!(s.check_and_schedule(0, true));
    assert!(!s.journal_index(0, true), "a quiesced tier");
    assert!(!s.tiers[1].pending_index && !s.tiers[1].indexing);
}

/// The timer's flushers write whole batches only.
#[test]
fn the_timer_flushes_whole_batches() {
    use crate::dbengine::engine::testutil::{dirty_page, file_reports, nth, seq_values};
    let dir = tempfile::tempdir().unwrap();
    let rt = running(dir.path(), &WorkPool::new(4, 256 * 1024));
    let e = Arc::clone(rt.engine());
    let mut m: Vec<_> = (0..108)
        .map(|i| dirty_page(&e, 0, nth(i), T0, &seq_values(10, i)))
        .collect();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(e.main.stats().dirty_entries, 108);
    m.push(dirty_page(&e, 0, nth(108), T0, &seq_values(10, 108)));
    assert!(eventually(|| e.main.stats().dirty_entries == 0));
    assert!(eventually(|| e.tiers[0].extents_in_flight() == 0));
    assert_eq!(
        file_reports(dir.path())[0]["pages_per_extent"],
        serde_json::json!({"109": 1})
    );
    rt.exit();
}

/// A rotation leaves the old file to index: `DBEV` indexes it on its own; a quiesced tier indexes nothing more.
#[test]
fn rotations_get_indexed() {
    use crate::dbengine::engine::testutil::{dirty_page, nth, seq_values};
    let dir = tempfile::tempdir().unwrap();
    let rt = running(dir.path(), &WorkPool::new(4, 256 * 1024));
    let e = Arc::clone(rt.engine());
    let v2 = |n: u32| dir.path().join(format!("journalfile-1-{n:010}.njfv2"));
    let fill = |from: usize| {
        (from..from + 64)
            .map(|i| {
                let m = dirty_page(&e, 0, nth(i), T0, &seq_values(1024, i));
                e.flush_dirty(0);
                m
            })
            .collect::<Vec<_>>()
    };
    let _m = fill(0);
    assert!(eventually(|| e.tiers[0]
        .file(1)
        .is_some_and(|df| df.v2_available())));
    assert!(v2(1).exists());
    rt.quiesce();
    assert!(eventually(|| e.tiers[0].quiesced()));
    let _m2 = fill(64);
    std::thread::sleep(Duration::from_millis(200));
    assert!(!v2(2).exists());
    rt.exit();
}

/// `flush_everything()`: nothing to flush says nothing; a dirty-only flush without waiting says so; a waiting one
/// reports its progress and completion; the collector wait says it waits.
#[test]
fn flush_everything_reports_as_c() {
    use crate::dbengine::engine::collect::{Alignment, CollectHandle};
    use crate::dbengine::engine::testutil::{dirty_page, nth, seq_values};
    let dir = tempfile::tempdir().unwrap();
    let rt = running(dir.path(), &WorkPool::new(4, 256 * 1024));
    let e = Arc::clone(rt.engine());
    let (_, records) = netdata_agent_log::capture(|| rt.flush_everything(true, true, false));
    assert_eq!(messages(records), Vec::<String>::new());

    let _m = dirty_page(&e, 0, nth(0), T0, &seq_values(10, 0));
    let (_, records) = netdata_agent_log::capture(|| rt.flush_everything(false, false, true));
    assert_eq!(messages(records), ["Flushing DBENGINE only dirty pages..."]);
    assert!(eventually(|| e.main.stats().dirty_entries == 0));

    let (metric, _) = e.mrg.add_and_acquire(&nth(1), 0, 0, 0, 0);
    let mut h = CollectHandle::init(&e, &metric, 1, Alignment::new("g", "c", 0));
    h.store_next(T0 as u64 * 1_000_000, 1.0, 1.0, 1.0, 1, 0, SN_DEFAULT_FLAGS);
    let (_, records) = netdata_agent_log::capture(|| rt.flush_everything(true, true, false));
    let records = messages(records);
    assert_eq!(records[0], "Flushing DBENGINE hot & dirty pages...");
    // the capture skips the rate limit: the collector still runs through the 50 checks
    let waits = records[1..]
        .iter()
        .take_while(|r| *r == "waiting for 1 collectors to finish")
        .count();
    assert_eq!(waits, 50);
    assert!(
        records[51].starts_with("DBENGINE: flushing at "),
        "{records:?}"
    );
    assert_eq!(records.last().unwrap(), "DBENGINE: flushing completed!");
    drop(h);
    rt.exit();
}

/// A tier's exit waits up to a second for its collectors (C's record once), then puts every page on disk.
#[test]
fn a_tier_exit_waits_for_collectors_then_flushes() {
    use crate::dbengine::engine::collect::{Alignment, CollectHandle};
    let dir = tempfile::tempdir().unwrap();
    let rt = running(dir.path(), &WorkPool::new(4, 256 * 1024));
    let e = Arc::clone(rt.engine());
    let (metric, _) = e.mrg.add_and_acquire(&A, 0, 0, 0, 0);
    let mut h = CollectHandle::init(&e, &metric, 1, Alignment::new("g", "c", 0));
    for t in T0..T0 + 10 {
        h.store_next(t as u64 * 1_000_000, 1.0, 1.0, 1.0, 1, 0, SN_DEFAULT_FLAGS);
    }
    let finisher = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(250));
        h.finalize()
    });
    let (_, records) = netdata_agent_log::capture(|| tier_exit(&e, 0, &rt.dbev.tx));
    assert!(!finisher.join().unwrap());
    assert_eq!(
        messages(records)[0],
        "DBENGINE: waiting for collectors to finish on tier 0..."
    );
    assert_eq!(e.main.hot_and_dirty_entries(), 0);
    let got = points(&mut e.query(&metric, T0, T0 + 9, Priority::Normal));
    assert_eq!(got.len(), 10);
    rt.exit();
}

/// A tier's shutdown waits for its extents in flight too, and says so once, counting the queries.
#[test]
fn a_tier_shutdown_waits_for_its_extents() {
    let dir = tempfile::tempdir().unwrap();
    let rt = running(dir.path(), &WorkPool::new(4, 256 * 1024));
    let e = Arc::clone(rt.engine());
    e.tiers[0].extent_started();
    let helper = {
        let e = Arc::clone(&e);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            e.tiers[0].extent_finished();
        })
    };
    let (_, records) = netdata_agent_log::capture(|| ctx_shutdown_wait(&e, 0));
    helper.join().unwrap();
    assert_eq!(
        messages(records),
        ["DBENGINE: waiting for 0 inflight queries to finish to shutdown tier 0..."]
    );
    rt.exit();
}

/// `check_and_schedule_db_rotation()` and `DATABASE_ROTATE`: a tier over its caps gets one rotation queued (asking
/// again while queued is recorded), one deletion runs at a time, none with two files or once under the caps, and its
/// end lets the next be queued.
#[test]
fn dbev_schedules_rotations_as_c() {
    let mut s = Sched {
        cpus: 1,
        tiers: vec![TierSched::default(); 1],
        ..Sched::default()
    };
    assert!(!s.schedule_rotation(0, || false));
    assert!(s.schedule_rotation(0, || true));
    let (again, records) = netdata_agent_log::capture(|| s.schedule_rotation(0, || true));
    assert!(!again);
    assert_eq!(
        messages(records),
        ["DBENGINE: tier 0 is already pending rotation"]
    );
    assert!(!s.database_rotate(0, 2, || true), "two files");
    assert!(s.schedule_rotation(0, || true));
    assert!(s.database_rotate(0, 3, || true));
    assert!(s.schedule_rotation(0, || true), "queued while deleting");
    assert!(!s.database_rotate(0, 3, || true), "one deletion at a time");
    s.rotate_done(0);
    assert!(s.schedule_rotation(0, || true));
    assert!(!s.database_rotate(0, 3, || false), "under the caps by then");
}

/// A tier over its size quota deletes its oldest pairs by itself, each once indexed, until it is under the quota (it
/// never deletes below three files).
#[test]
fn a_tier_over_its_quota_deletes_its_oldest_pairs() {
    use crate::dbengine::engine::testutil::{dirty_page, nth, seq_values, write_cfg};
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = init(&[Some(dir.path())]);
    cfg.tiers = vec![Some(TierConfig {
        max_disk_space: 2 << 20,
        ..write_cfg(0, dir.path())
    })];
    let rt = Runtime::start(cfg, &WorkPool::new(4, 256 * 1024), Box::new(|_| {}), || NOW);
    let e = Arc::clone(rt.engine());
    let mut held = Vec::new();
    for i in 0..5 * 63 {
        held.push(dirty_page(&e, 0, nth(i), T0, &seq_values(1024, i)));
        e.flush_dirty(0);
    }
    let ndf = dir.path().join("datafile-1-0000000001.ndf");
    assert!(eventually(
        || !ndf.exists() && e.tiers[0].filenos().first() > Some(&1)
    ));
    assert!(eventually(|| !e.tiers[0].cap_exceeded(NOW)));
    assert!(e.tiers[0].filenos().len() >= 2);
    rt.exit();
}
