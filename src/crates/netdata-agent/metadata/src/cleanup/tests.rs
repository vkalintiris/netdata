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
