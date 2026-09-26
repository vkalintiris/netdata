//! Checks over the C-written snapshots of the private fixtures (`<fx>/<run>/cache/`): `NETDATA_DBENGINE_FIXTURES`
//! points at them, and without it each test says it skipped. Brief `knowledge/brief-dbengine-s1.md` §5.2 in the
//! status repository.

use std::path::PathBuf;

use netdata_agent_metadata::conn::{Markers, init_database_batch};
use netdata_agent_metadata::schema;
use rusqlite::Connection;

fn fixtures() -> Option<PathBuf> {
    let dir = std::env::var_os("NETDATA_DBENGINE_FIXTURES").map(PathBuf::from);
    if dir.is_none() {
        eprintln!("skipped: NETDATA_DBENGINE_FIXTURES unset");
    }
    dir
}

/// Every schema entry but the statistics table: type, name, table and the stored `CREATE` text.
fn schema_entries(c: &Connection) -> Vec<(String, String, String, Option<String>)> {
    let mut stmt = c
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name <> 'sqlite_stat1' ORDER BY rowid")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// A database created from C's statements holds C's schema text entry for entry (brief F8).
#[test]
fn a_fresh_schema_equals_cs() {
    let Some(fx) = fixtures() else { return };
    let dir = tempfile::tempdir().unwrap();
    for run in ["run1", "run2", "runR"] {
        let copy = dir.path().join(format!("{run}.db"));
        std::fs::copy(fx.join(run).join("cache/netdata-meta.db"), &copy).unwrap();
        let c_db = Connection::open(&copy).unwrap();
        let fresh = Connection::open_in_memory().unwrap();
        init_database_batch(&fresh, schema::META_CONFIG, &Markers::default()).unwrap();
        init_database_batch(&fresh, schema::META_CLEANUP, &Markers::default()).unwrap();
        assert_eq!(schema_entries(&fresh), schema_entries(&c_db), "{run}");
    }
}

/// Opening a copy of a C-written cache changes neither schema nor version (brief §1.5 item 3).
#[test]
fn opening_a_c_cache_keeps_its_schema() {
    use netdata_agent_metadata::open::{ContextDb, MetaDb, SqliteSettings};
    let Some(fx) = fixtures() else { return };
    let version = |c: &Connection| {
        c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap()
    };
    for run in ["run1", "run2", "runR"] {
        let dir = tempfile::tempdir().unwrap();
        for file in ["netdata-meta.db", "context-meta.db"] {
            let copy = dir.path().join(file);
            std::fs::copy(fx.join(run).join("cache").join(file), &copy).unwrap();
            // the fixtures are read-only, and a copy keeps the mode
            std::fs::set_permissions(&copy, std::os::unix::fs::PermissionsExt::from_mode(0o644))
                .unwrap();
        }
        let before: Vec<_> = ["netdata-meta.db", "context-meta.db"]
            .iter()
            .map(|f| {
                let c = Connection::open(dir.path().join(f)).unwrap();
                (schema_entries(&c), version(&c))
            })
            .collect();
        let (meta, records) =
            netdata_agent_log::capture(|| MetaDb::open(dir.path(), &SqliteSettings::default()));
        let meta = meta.unwrap_or_else(|| panic!("{run}: {records:?}"));
        let context = ContextDb::open(dir.path(), &SqliteSettings::default()).expect(run);
        context.close();
        meta.close();
        let after: Vec<_> = ["netdata-meta.db", "context-meta.db"]
            .iter()
            .map(|f| {
                let c = Connection::open(dir.path().join(f)).unwrap();
                (schema_entries(&c), version(&c))
            })
            .collect();
        assert_eq!(before, after, "{run}");
    }
}

/// A writable copy of a snapshot's two databases.
fn copy_cache(fx: &std::path::Path, run: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for file in ["netdata-meta.db", "context-meta.db"] {
        let copy = dir.path().join(file);
        std::fs::copy(fx.join(run).join("cache").join(file), &copy).unwrap();
        std::fs::set_permissions(&copy, std::os::unix::fs::PermissionsExt::from_mode(0o644))
            .unwrap();
    }
    dir
}

fn hex(id: &[u8; 16]) -> String {
    id.iter().map(|b| format!("{b:02x}")).collect()
}

/// The chart and dimension readers give every host's metrics as the checkers' metric map lists them, the archived
/// hosts are the stored children, and the dimension UUIDs are C's count (brief §5.2 items 2-4).
#[test]
fn the_readers_list_cs_metrics_and_hosts() {
    use netdata_agent_metadata::open::{MetaDb, SqliteSettings};
    use netdata_agent_metadata::read::{chart_list, dimension_list};
    let Some(fx) = fixtures() else { return };
    for (run, uuids) in [
        ("run1", 116),
        ("run2", 116),
        ("runR", 116),
        ("orig-20260924/run1", 122),
        ("orig-20260924/run2", 116),
        ("orig-20260924/runR", 122),
    ] {
        let dir = copy_cache(&fx, run);
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).expect(run);
        let hosts: Vec<([u8; 16], String)> = {
            let c = meta.lock();
            let mut stmt = c.prepare("SELECT host_id, hostname FROM host").unwrap();
            stmt.query_map([], |r| {
                Ok((r.get::<_, Vec<u8>>(0)?.try_into().unwrap(), r.get(1)?))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect()
        };
        let mut rows = Vec::new();
        for (host, hostname) in &hosts {
            let c = meta.lock();
            let mut update_every = std::collections::HashMap::new();
            chart_list(&c, host, |chart| {
                update_every.insert(chart.id.unwrap(), chart.update_every);
            });
            dimension_list(&c, host, |dim| {
                let chart = dim.chart_id.unwrap();
                rows.push(format!(
                    "{}\t{chart}\t{}\t{}\t{}\t{hostname}",
                    hex(&dim.dim_id),
                    dim.id.unwrap(),
                    update_every[&chart],
                    hex(host)
                ));
            });
        }
        rows.sort();
        let result = run.replace('/', "-");
        let mut want: Vec<String> =
            std::fs::read_to_string(fx.join("results").join(&result).join("metric-map.tsv"))
                .unwrap()
                .lines()
                .filter(|l| !l.starts_with('#'))
                .map(String::from)
                .collect();
        want.sort();
        assert_eq!(rows, want, "{run}");
        let archived: Vec<String> = meta
            .archived_hosts()
            .into_iter()
            .map(|h| h.hostname.unwrap())
            .collect();
        assert_eq!(archived, ["b6child"], "{run}");
        assert_eq!(meta.dimension_uuids(|_| {}), uuids, "{run}");
        meta.close();
    }
}
