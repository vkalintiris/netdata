use super::*;
use crate::dbengine::engine::query::{DEFAULT_PAGES_PER_EXTENT, Priority};
use crate::dbengine::engine::testutil::{A, B, NOW, array_page, cfg, pair_with_extents, points};
use crate::storage_number::{SN_DEFAULT_FLAGS, pack};
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
    let first: Vec<i64> = rt.engine().tiers.iter().map(|t| t.first_time_s).collect();
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
    let mut held = engine.query(&metric, T0, T0 + 9, Priority::Normal, NOW);
    rt.quiesce();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !engine.tiers[0].quiesced() {
        assert!(std::time::Instant::now() < deadline, "quiesce");
        std::thread::sleep(Duration::from_millis(1));
    }
    let got = points(&mut engine.query(&metric, T0, T0 + 9, Priority::Normal, NOW));
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
