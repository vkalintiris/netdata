use super::*;
use crate::open::SqliteSettings;
use crate::read::NodeId;

fn db() -> (tempfile::TempDir, MetaDb) {
    let dir = tempfile::tempdir().unwrap();
    let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
    (dir, meta)
}

const HOST: [u8; 16] = [0xaa; 16];
const OTHER: [u8; 16] = [0xbb; 16];
const CHART: [u8; 16] = [0xcc; 16];
const DIM: [u8; 16] = [0xdd; 16];

fn host(hostname: &str) -> HostRecord<'_> {
    HostRecord {
        host_id: HOST,
        hostname,
        registry_hostname: hostname,
        update_every: 1,
        os: "linux",
        timezone: "Etc/UTC",
        hops: 1,
        memory_mode: 4,
        abbrev_timezone: "UTC",
        utc_offset: 0,
        program_name: "netdata",
        program_version: "v0",
        entries: 3600,
        health_enabled: true,
        last_connected: 1_700_000_000,
    }
}

fn chart<'a>(name: Option<&'a str>, title: &'static str) -> ChartRecord<'a> {
    ChartRecord {
        chart_id: CHART,
        host_id: HOST,
        type_: "t",
        id: "c",
        name,
        family: "f",
        context: "t.c",
        title,
        units: "u",
        plugin: "p",
        module: "",
        priority: 1000,
        update_every: 1,
        chart_type: 2,
        memory_mode: 4,
        history_entries: 3600,
    }
}

fn rows(meta: &MetaDb, sql: &str) -> Vec<String> {
    let c = meta.lock();
    let mut stmt = c.prepare(sql).unwrap();
    let n = stmt.column_count();
    stmt.query_map([], |r| {
        Ok((0..n)
            .map(|i| match r.get_ref(i).unwrap() {
                ValueRef::Null => "NULL".to_string(),
                ValueRef::Integer(v) => v.to_string(),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                ValueRef::Blob(b) => format!("x{:02x}", b[0]),
                ValueRef::Real(v) => v.to_string(),
            })
            .collect::<Vec<_>>()
            .join("|"))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
    records.into_iter().filter_map(|r| r.message).collect()
}

/// A host's rows read back as the archived-host readers read them; a second store replaces the host row (a new
/// rowid, as C's `INSERT OR REPLACE`) and adds its node instance only once.
#[test]
fn host_rows_read_back() {
    let (_dir, meta) = db();
    assert!(meta.store_host(&host("child")));
    let keys = [
        ("NETDATA_HOST_OS_NAME", Some("Debian")),
        ("NETDATA_HOST_OS_ID", None),
    ];
    assert!(meta.store_host_system_info(&HOST, &keys));
    let labels = [
        LabelRecord {
            name: b"role",
            value: b"db",
            source: 2,
        },
        LabelRecord {
            name: b"_os",
            value: b"linux",
            source: 1 | (1 << 29),
        },
    ];
    assert!(meta.delete_host_labels(&HOST) && meta.store_host_labels(&HOST, &labels));
    assert!(meta.store_claim_id(&HOST, None));
    let archived = meta.archived_hosts();
    assert_eq!(archived.len(), 1);
    let row = &archived[0];
    assert_eq!(
        (
            row.hostname.as_deref(),
            row.entries,
            row.last_connected,
            row.is_registered
        ),
        (Some("child"), 3600, 1_700_000_000, false)
    );
    assert_eq!(
        meta.host_info(&HOST),
        [
            ("NETDATA_HOST_OS_ID".into(), "unknown".into()),
            ("NETDATA_HOST_OS_NAME".into(), "Debian".into())
        ]
    );
    assert_eq!(
        meta.host_labels(&HOST),
        [
            ("_os".into(), "linux".into(), 1),
            ("role".into(), "db".into(), 2)
        ]
    );
    assert_eq!(
        rows(&meta, "SELECT rowid, tags, health_enabled FROM host"),
        ["1||1"]
    );
    assert!(meta.store_host(&host("renamed")));
    assert_eq!(
        rows(&meta, "SELECT rowid, hostname FROM host"),
        ["2|renamed"]
    );
    assert_eq!(
        rows(&meta, "SELECT count(*), claim_id FROM node_instance"),
        ["1|NULL"]
    );
}

/// Labels go in batches of 1024 with the internal marks left out; host labels are replaced (deleted first), chart
/// labels only upserted.
#[test]
fn labels_batch_and_replace_as_c() {
    let (_dir, meta) = db();
    let names: Vec<String> = (0..1025).map(|i| format!("l{i}")).collect();
    let many: Vec<LabelRecord<'_>> = names
        .iter()
        .map(|n| LabelRecord {
            name: n.as_bytes(),
            value: b"v",
            source: 1 | (1 << 30) | (1 << 31),
        })
        .collect();
    assert!(meta.store_host_labels(&HOST, &many));
    assert_eq!(
        rows(&meta, "SELECT count(*), max(source_type) FROM host_label"),
        ["1025|1"]
    );
    let one = [LabelRecord {
        name: b"l0",
        value: b"w",
        source: 2,
    }];
    assert!(meta.delete_host_labels(&HOST) && meta.store_host_labels(&HOST, &one));
    assert_eq!(
        rows(
            &meta,
            "SELECT label_key, label_value, source_type FROM host_label"
        ),
        ["l0|w|2"]
    );
    meta.scan_host(|scan| {
        assert!(scan.chart_labels(&CHART, &many[..2]));
        assert!(scan.chart_labels(&CHART, &one));
    });
    assert_eq!(
        rows(
            &meta,
            "SELECT label_key, label_value, source_type FROM chart_label ORDER BY label_key"
        ),
        ["l0|w|2", "l1|v|1"]
    );
}

/// Charts and dimensions upsert on their ids inside the scan's transaction: an absent or empty name is NULL, a
/// hidden dimension stores `'hidden'`, and a second store updates the row in place.
#[test]
fn charts_and_dimensions_upsert() {
    let (_dir, meta) = db();
    let mut dim = DimRecord {
        dim_id: DIM,
        chart_id: CHART,
        id: "d",
        name: "d",
        multiplier: 3,
        divisor: 7,
        algorithm: 1,
        hidden: true,
    };
    meta.scan_host(|scan| {
        assert!(scan.chart(&chart(Some(""), "first")) && scan.dimension(&dim));
        assert!(!scan.conn.is_autocommit(), "BEGIN TRANSACTION");
    });
    assert!(meta.lock().is_autocommit(), "COMMIT TRANSACTION");
    dim.hidden = false;
    meta.scan_host(|scan| {
        assert!(scan.chart(&chart(Some("named"), "second")) && scan.dimension(&dim));
    });
    assert_eq!(
        rows(
            &meta,
            "SELECT rowid, chart_id, host_id, type, id, name, family, context, title, unit, plugin, module, priority, update_every, chart_type, memory_mode, history_entries FROM chart"
        ),
        ["1|xcc|xaa|t|c|named|f|t.c|second|u|p||1000|1|2|4|3600"]
    );
    assert_eq!(
        rows(&meta, "SELECT name FROM chart WHERE name IS NULL"),
        Vec::<String>::new()
    );
    assert_eq!(
        rows(
            &meta,
            "SELECT rowid, dim_id, chart_id, id, name, multiplier, divisor, algorithm, options FROM dimension"
        ),
        ["1|xdd|xcc|d|d|3|7|1|NULL"]
    );
    meta.delete_dimension(&DIM);
    assert_eq!(rows(&meta, "SELECT count(*) FROM dimension"), ["0"]);
}

/// The unclaimed start's invalidation nulls every node id (C's `EXISTS` is uncorrelated), a matching claim id none;
/// the unregistration nulls one and invalidates its last connection; a netdatacli label is stored with `AUTO`.
#[test]
fn node_instances_as_c() {
    let (_dir, meta) = db();
    assert!(meta.store_host(&host("child")));
    meta.lock()
        .execute_batch(
            "INSERT OR REPLACE INTO node_instance (host_id, claim_id, node_id) VALUES (x'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', \
             x'11111111111111111111111111111111', x'01010101010101010101010101010101'), \
             (x'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', NULL, x'02020202020202020202020202020202')",
        )
        .unwrap();
    meta.invalidate_node_instances(&HOST, Some(&[0x11; 16]));
    assert_eq!(
        rows(
            &meta,
            "SELECT count(*) FROM node_instance WHERE node_id IS NULL"
        ),
        ["0"]
    );
    meta.invalidate_node_instances(&HOST, None);
    assert_eq!(
        rows(
            &meta,
            "SELECT count(*) FROM node_instance WHERE node_id IS NULL"
        ),
        ["2"]
    );
    meta.lock()
        .execute_batch("UPDATE node_instance SET node_id = x'03030303030303030303030303030303'")
        .unwrap();
    meta.unregister_node(&HOST);
    assert_eq!(meta.node_id(&HOST), NodeId::Cleared);
    assert_ne!(meta.node_id(&OTHER), NodeId::Cleared);
    assert_eq!(rows(&meta, "SELECT last_connected FROM host"), ["1"]);
    assert!(meta.set_host_label(&HOST, "_is_ephemeral", "true"));
    assert_eq!(
        rows(
            &meta,
            "SELECT label_key, label_value, source_type FROM host_label"
        ),
        ["_is_ephemeral|true|1"]
    );
}

/// At most once a minute, and only past 5% free pages, 10% of the free pages go, with C's record.
#[test]
fn vacuum_as_c() {
    let (_dir, meta) = db();
    let mut next_run = 0;
    let ((), records) = netdata_agent_log::capture(|| meta.vacuum(&mut next_run, 1000));
    assert_eq!((messages(records), next_run), (Vec::<String>::new(), 1060));
    meta.lock()
        .execute_batch(
            "CREATE TABLE filler(x); WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000) \
             INSERT INTO filler SELECT randomblob(1000) FROM n; DROP TABLE filler;",
        )
        .unwrap();
    let free = |meta: &MetaDb| pragma_value(&meta.lock(), "PRAGMA freelist_count");
    let before = free(&meta);
    assert!(before > 100, "{before}");
    let ((), records) = netdata_agent_log::capture(|| meta.vacuum(&mut next_run, 1059));
    assert!(
        records.is_empty() && free(&meta) == before,
        "not before the next run"
    );
    let ((), records) = netdata_agent_log::capture(|| meta.vacuum(&mut next_run, 1060));
    let freed = before * 10 / 100;
    assert_eq!(
        messages(records),
        [format!("METADATA: Freeing {freed} database pages")]
    );
    assert_eq!(free(&meta), before - freed);
    meta.wal_checkpoint();
}

/// A statement that cannot be prepared names the C function; a failed step logs C's line.
#[test]
fn failures_log_cs_records() {
    let (_dir, meta) = db();
    meta.lock()
        .execute_batch(
            "DROP TABLE host; CREATE TRIGGER no_dims BEFORE INSERT ON dimension BEGIN SELECT RAISE(ABORT, 'no'); END",
        )
        .unwrap();
    let (stored, records) = netdata_agent_log::capture(|| meta.store_host(&host("child")));
    assert!(!stored);
    assert_eq!(
        messages(records),
        ["Failed to prepare statement, rc=1 in store_host_metadata"]
    );
    let dim = DimRecord {
        dim_id: DIM,
        chart_id: CHART,
        id: "d",
        name: "d",
        multiplier: 1,
        divisor: 1,
        algorithm: 0,
        hidden: false,
    };
    let (stored, records) =
        netdata_agent_log::capture(|| meta.scan_host(|scan| scan.dimension(&dim)));
    assert!(!stored);
    assert_eq!(messages(records), ["Failed to store dimension, rc = 19"]);
}
