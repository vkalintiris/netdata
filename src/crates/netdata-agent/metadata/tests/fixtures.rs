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
