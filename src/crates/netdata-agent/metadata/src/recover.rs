//! `recover_database()` (`src/database/sqlite/sqlite_metadata.c`): what SQLite's recover extension can read of a
//! corrupt `netdata-meta.db` becomes the database, with C's records.

use std::path::Path;

use netdata_agent_log::{netdata_log_error, netdata_log_info};
use rusqlite::{Connection, OpenFlags};

use crate::conn::{self, Markers};

const SQLITE_OK: i32 = 0;

/// Recovers `src` into `dst`, then renames `dst` over `src` when recover could free its resources. A database that
/// cannot be opened is left alone, silently.
pub fn recover_database(src: &Path, dst: &Path) {
    let Ok(c) = Connection::open_with_flags(
        src,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    ) else {
        return;
    };
    let _ = c.busy_timeout(std::time::Duration::ZERO);
    netdata_log_info!("Recover {}", src.display());
    netdata_log_info!("     to {}", dst.display());
    // closing the database afterwards removes its -wal and -shm files
    let _ = conn::db_execute(
        &c,
        "select count(*) from sqlite_master limit 0",
        &Markers::default(),
    );
    let Some((run, finish)) = netdata_agent_sys::sqlite_recover(&c, &dst.to_string_lossy()) else {
        return;
    };
    if run == SQLITE_OK {
        netdata_log_info!("Recover complete");
    } else {
        netdata_log_error!("Recover encountered an error but the database may be usable");
    }
    drop(c);
    if finish == SQLITE_OK {
        if std::fs::rename(dst, src).is_ok() {
            netdata_log_info!("Renamed {}", dst.display());
            netdata_log_info!("     to {}", src.display());
        }
    } else {
        netdata_log_error!("Recover failed to free resources");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_corrupt_database_is_recovered_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (
            dir.path().join("netdata-meta.db"),
            dir.path().join("netdata-meta-recover.db"),
        );
        {
            let c = Connection::open(&src).unwrap();
            c.execute_batch(
                "CREATE TABLE a(x INTEGER, y TEXT); CREATE TABLE b(x INTEGER, y TEXT);
                 WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
                 INSERT INTO a SELECT i, printf('%0200d', i) FROM n;
                 INSERT INTO b VALUES (1, 'kept')",
            )
            .unwrap();
        }
        // scribble over a page in the middle of table a
        let mut bytes = std::fs::read(&src).unwrap();
        let page = 4096 * (bytes.len() / 4096 / 2);
        bytes[page..page + 4096].fill(0xa5);
        std::fs::write(&src, bytes).unwrap();
        let (_, records) = netdata_agent_log::capture(|| recover_database(&src, &dst));
        let d = dir.path().display();
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(
            messages,
            [
                format!("Recover {d}/netdata-meta.db"),
                format!("     to {d}/netdata-meta-recover.db"),
                "Recover complete".to_string(),
                format!("Renamed {d}/netdata-meta-recover.db"),
                format!("     to {d}/netdata-meta.db"),
            ]
        );
        assert!(!dst.exists());
        let c = Connection::open(&src).unwrap();
        let a: i64 = c
            .query_row("SELECT count(*) FROM a", [], |r| r.get(0))
            .unwrap();
        assert!(a > 1500 && a < 2000, "{a}");
        let b: String = c.query_row("SELECT y FROM b", [], |r| r.get(0)).unwrap();
        assert_eq!(b, "kept");
    }
}
