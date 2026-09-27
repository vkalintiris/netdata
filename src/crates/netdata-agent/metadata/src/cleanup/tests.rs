use super::*;
use crate::open::SqliteSettings;

const HOST: [u8; 16] = [0xaa; 16];
const OTHER: [u8; 16] = [0xbb; 16];

fn db(settings: &SqliteSettings) -> (tempfile::TempDir, MetaDb) {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaDb::open(dir.path(), settings).unwrap();
    (dir, meta)
}

/// A chart of `host` in `context` with a label and the dimensions `dims`.
fn chart(meta: &MetaDb, host: [u8; 16], chart_id: u8, context: &str, dims: &[u8]) {
    let c = meta.lock();
    c.execute(
        "INSERT INTO chart (chart_id, host_id, context) VALUES (?, ?, ?)",
        rusqlite::params![&[chart_id; 16][..], &host[..], context],
    )
    .unwrap();
    c.execute(
        "INSERT INTO chart_label (chart_id, label_key, label_value) VALUES (?, 'k', 'v')",
        [&[chart_id; 16][..]],
    )
    .unwrap();
    for &d in dims {
        c.execute(
            "INSERT INTO dimension (dim_id, chart_id) VALUES (?, ?)",
            rusqlite::params![&[d; 16][..], &[chart_id; 16][..]],
        )
        .unwrap();
    }
}

fn queue(meta: &MetaDb, host: [u8; 16], context: &str) {
    meta.schedule_host_ctx_cleanup(&[(host, context.to_string())], || false);
}

/// The first byte of each dimension, chart and chart label id, and each queued (host, context).
type Tables = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<(u8, String)>);

fn state(meta: &MetaDb) -> Tables {
    let c = meta.lock();
    let ids = |sql: &str| -> Vec<u8> {
        let mut stmt = c.prepare(sql).unwrap();
        stmt.query_map([], |r| r.get::<_, Vec<u8>>(0).map(|b| b[0]))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    let mut stmt = c
        .prepare("SELECT host_id, context FROM ctx_metadata_cleanup ORDER BY host_id, context")
        .unwrap();
    let queued = stmt
        .query_map([], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?[0], r.get::<_, String>(1)?))
        })
        .unwrap()
        .map(Result::unwrap)
        .collect();
    (
        ids("SELECT dim_id FROM dimension ORDER BY dim_id"),
        ids("SELECT chart_id FROM chart ORDER BY chart_id"),
        ids("SELECT chart_id FROM chart_label ORDER BY chart_id"),
        queued,
    )
}

/// The scan of C's `cleanup_host_context_metadata()`: a queued context's dimensions without retention go (and with
/// the last one their chart and its labels, by the triggers); its cleanup row goes; a queued context with no chart
/// stays queued; other contexts and hosts are not touched.
#[test]
fn queued_contexts_lose_the_dimensions_without_retention() {
    let (_dir, meta) = db(&SqliteSettings::default());
    chart(&meta, HOST, 1, "x", &[0x11, 0x12]);
    chart(&meta, HOST, 2, "y", &[0x21, 0x22]);
    chart(&meta, HOST, 3, "w", &[0x31]);
    chart(&meta, OTHER, 4, "x", &[0x41]);
    for context in ["x", "y", "z"] {
        queue(&meta, HOST, context);
    }
    queue(&meta, OTHER, "x");
    let ((), records) = netdata_agent_log::capture(|| {
        // 0x21 still has data
        meta.cleanup_host_contexts(&HOST, "h", |id| id[0] != 0x21, || false);
    });
    let records: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        state(&meta),
        (
            vec![0x21, 0x31, 0x41],
            vec![2, 3, 4],
            vec![2, 3, 4],
            vec![(0xaa, "z".to_string()), (0xbb, "x".to_string())]
        )
    );
    assert_eq!(
        records,
        [
            "Verifying the retention of 3 contexts for host h",
            "Verified the contexts of host h (Checked 4 metrics and removed 3)"
        ]
    );
}

/// Nothing queued: no record; a shutdown stops after the dimension it met, and the context's cleanup row still goes.
#[test]
fn a_shutdown_stops_after_one_dimension() {
    let (_dir, meta) = db(&SqliteSettings::default());
    chart(&meta, HOST, 1, "x", &[0x11, 0x12]);
    chart(&meta, HOST, 2, "y", &[0x21]);
    let ((), records) =
        netdata_agent_log::capture(|| meta.cleanup_host_contexts(&HOST, "h", |_| true, || false));
    assert!(records.is_empty());
    queue(&meta, HOST, "x");
    queue(&meta, HOST, "y");
    meta.cleanup_host_contexts(&HOST, "h", |_| true, || true);
    let (dims, _, _, queued) = state(&meta);
    assert_eq!(
        (dims, queued),
        (vec![0x12, 0x21], vec![(0xaa, "y".to_string())])
    );
}

/// `sql_metadata_wal_size_acceptable()`: ten journal size limits of WAL; a missing WAL is acceptable.
#[test]
fn the_wal_limit_is_ten_journal_size_limits() {
    let (dir, meta) = db(&SqliteSettings {
        journal_size_limit: Some(100),
        ..Default::default()
    });
    let wal = dir.path().join("netdata-meta.db-wal");
    let _ = std::fs::remove_file(&wal);
    assert!(meta.wal_size_acceptable());
    std::fs::write(&wal, vec![0; 1000]).unwrap();
    assert!(meta.wal_size_acceptable());
    std::fs::write(&wal, vec![0; 1001]).unwrap();
    assert!(!meta.wal_size_acceptable());
}

/// The records of a call, and its answer.
fn cycle_call(meta: &MetaDb, cycle: &mut CleanupCycle, env: &CycleEnv<'_>) -> (bool, Vec<String>) {
    let (next, records) = netdata_agent_log::capture(|| meta.run_cleanup_cycle(cycle, env));
    (
        next,
        records.into_iter().filter_map(|r| r.message).collect(),
    )
}

/// `run_cleanup_cycle()` of the dimension cycle: armed 1800 s after the first call; a pass snapshots the largest
/// rowid, runs slices 60 s apart (each ending at the first row checked more than 5 monotonic seconds in), completes
/// once the slices reached the snapshot, and runs again a week later.
#[test]
fn the_dimension_cycle_runs_in_slices_then_weekly() {
    use std::cell::Cell;
    let (_dir, meta) = db(&SqliteSettings::default());
    chart(&meta, HOST, 1, "x", &[0x11, 0x12, 0x13, 0x14]);
    let now = Cell::new(1000);
    let mono = Cell::new(0);
    let clock = || now.get();
    // each row checked takes 3 monotonic seconds: a slice ends after its second row (6 > 5)
    let monotonic = || {
        let t = mono.get();
        mono.set(t + 3);
        t
    };
    let env = CycleEnv {
        now: &clock,
        monotonic: &monotonic,
        shutting_down: &|| false,
        dimension_can_be_deleted: &|id: &[u8; 16]| id[0].is_multiple_of(2),
    };
    let mut cycle = CleanupCycle::new(CycleKind::Dimension);
    assert_eq!(cycle_call(&meta, &mut cycle, &env), (true, vec![]));
    now.set(2799);
    assert_eq!(cycle_call(&meta, &mut cycle, &env), (true, vec![]));
    now.set(2800);
    assert_eq!(
        cycle_call(&meta, &mut cycle, &env),
        (
            false,
            vec![
                "Dimension metadata check has been scheduled to run (max id = 4)".to_string(),
                "Checking dimensions starting after row 0".to_string(),
                "Dimensions checked 2, deleted 1. Checks will resume in 60 seconds".to_string()
            ]
        )
    );
    now.set(2859);
    assert!(cycle_call(&meta, &mut cycle, &env).0);
    now.set(2860);
    assert_eq!(
        cycle_call(&meta, &mut cycle, &env).1,
        [
            "Checking dimensions starting after row 2",
            "Dimensions checked 2, deleted 1. Checks will resume in 60 seconds"
        ]
    );
    now.set(2920);
    assert_eq!(
        cycle_call(&meta, &mut cycle, &env),
        (true, vec!["Dimension metadata check completed".to_string()])
    );
    assert_eq!(state(&meta).0, [0x11, 0x13]);
    // a week later, a fresh snapshot: the deleted row 4 was the largest
    now.set(2920 + 604_799);
    assert!(cycle_call(&meta, &mut cycle, &env).1.is_empty());
    now.set(2920 + 604_800);
    assert_eq!(
        cycle_call(&meta, &mut cycle, &env).1[0],
        "Dimension metadata check has been scheduled to run (max id = 3)"
    );
}

/// The chart cycle deletes the charts no dimension names (the trigger takes their labels), the label cycle the
/// labels no chart names; both run one pass only. An empty table is scheduled and completed in one call.
#[test]
fn the_chart_and_label_cycles_run_once() {
    let (_dir, meta) = db(&SqliteSettings::default());
    chart(&meta, HOST, 1, "x", &[0x11]);
    chart(&meta, HOST, 2, "y", &[]);
    meta.lock()
        .execute(
            "INSERT INTO chart_label (chart_id, label_key, label_value) VALUES (x'09090909090909090909090909090909', 'k', 'v')",
            [],
        )
        .unwrap();
    let now = std::cell::Cell::new(0);
    let clock = || now.get();
    let env = CycleEnv {
        now: &clock,
        monotonic: &|| 0,
        shutting_down: &|| false,
        dimension_can_be_deleted: &|_: &[u8; 16]| false,
    };
    for (kind, singular, plural, checked, deleted, repeat) in [
        (CycleKind::Chart, "Chart", "Charts", 2, 1, 60),
        (
            CycleKind::ChartLabel,
            "Chart label",
            "Chart labels",
            2,
            1,
            3600,
        ),
    ] {
        now.set(1);
        let mut cycle = CleanupCycle::new(kind);
        assert!(cycle_call(&meta, &mut cycle, &env).0);
        now.set(1801);
        let (next, records) = cycle_call(&meta, &mut cycle, &env);
        assert!(!next);
        assert_eq!(
            records[2],
            format!(
                "{plural} checked {checked}, deleted {deleted}. Checks will resume in {repeat} seconds"
            )
        );
        now.set(1801 + repeat);
        assert_eq!(
            cycle_call(&meta, &mut cycle, &env),
            (true, vec![format!("{singular} metadata check completed")])
        );
        now.set(1_000_000);
        assert_eq!(cycle_call(&meta, &mut cycle, &env), (true, vec![]));
    }
    let (dims, charts, labels, _) = state(&meta);
    assert_eq!((dims, charts, labels), (vec![0x11], vec![1], vec![1]));

    let (_dir, empty) = db(&SqliteSettings::default());
    let mut cycle = CleanupCycle::new(CycleKind::Chart);
    now.set(1);
    cycle_call(&empty, &mut cycle, &env);
    now.set(1801);
    assert_eq!(
        cycle_call(&empty, &mut cycle, &env),
        (
            true,
            vec![
                "Chart metadata check has been scheduled to run (max id = 0)".to_string(),
                "Chart metadata check completed".to_string()
            ]
        )
    );
}

/// A shutdown ends a slice before its first row, and the slice still reports and arms the next one.
#[test]
fn a_shutdown_ends_a_slice_before_its_rows() {
    let (_dir, meta) = db(&SqliteSettings::default());
    chart(&meta, HOST, 1, "x", &[0x12]);
    let now = std::cell::Cell::new(0);
    let clock = || now.get();
    let env = CycleEnv {
        now: &clock,
        monotonic: &|| 0,
        shutting_down: &|| true,
        dimension_can_be_deleted: &|_: &[u8; 16]| true,
    };
    let mut cycle = CleanupCycle::new(CycleKind::Dimension);
    cycle_call(&meta, &mut cycle, &env);
    now.set(1800);
    let (next, records) = cycle_call(&meta, &mut cycle, &env);
    assert!(!next);
    assert_eq!(
        records[2],
        "Dimensions checked 0, deleted 0. Checks will resume in 60 seconds"
    );
    assert_eq!(state(&meta).0, [0x12]);
}
