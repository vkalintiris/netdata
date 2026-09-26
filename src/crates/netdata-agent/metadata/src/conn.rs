//! The connection helpers of `src/database/sqlite/sqlite_functions.c`: result codes and their texts, the corruption
//! markers of `netdata-meta.db`, `init_database_batch()`, `db_execute()` and the retries of
//! `sqlite3_step_monitored()`.

use std::fs::OpenOptions;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use rusqlite::Connection;
use rusqlite::fallible_iterator::FallibleIterator;

pub const SQLITE_ERROR: i32 = 1;
pub const SQLITE_BUSY: i32 = 5;
pub const SQLITE_LOCKED: i32 = 6;
pub const SQLITE_CORRUPT: i32 = 11;
pub const SQLITE_NOTADB: i32 = 26;

/// `SQL_MAX_RETRY` and `SQLITE_INSERT_DELAY`.
pub const MAX_RETRY: usize = 100;
pub const RETRY_DELAY: Duration = Duration::from_millis(10);

/// The primary result code of an error, as C sees it: C does not turn on extended result codes (rusqlite does), and
/// errors that are not SQLite's read as `SQLITE_ERROR`.
pub fn result_code(err: &rusqlite::Error) -> i32 {
    match err {
        rusqlite::Error::SqliteFailure(e, _) | rusqlite::Error::SqlInputError { error: e, .. } => {
            e.extended_code & 0xff
        }
        _ => SQLITE_ERROR,
    }
}

/// The connection's message for an error (`sqlite3_errmsg()`, what `sqlite3_exec()` reports; rusqlite gives a failed
/// prepare its own variant), else the code's text.
pub fn message(err: &rusqlite::Error) -> String {
    match err {
        rusqlite::Error::SqliteFailure(_, Some(msg))
        | rusqlite::Error::SqlInputError { msg, .. } => msg.clone(),
        err => errstr(result_code(err)).to_string(),
    }
}

/// `sqlite3_errstr()`.
pub fn errstr(rc: i32) -> &'static str {
    const MESSAGES: [Option<&str>; 29] = [
        Some("not an error"),
        Some("SQL logic error"),
        None,
        Some("access permission denied"),
        Some("query aborted"),
        Some("database is locked"),
        Some("database table is locked"),
        Some("out of memory"),
        Some("attempt to write a readonly database"),
        Some("interrupted"),
        Some("disk I/O error"),
        Some("database disk image is malformed"),
        Some("unknown operation"),
        Some("database or disk is full"),
        Some("unable to open database file"),
        Some("locking protocol"),
        None,
        Some("database schema has changed"),
        Some("string or blob too big"),
        Some("constraint failed"),
        Some("datatype mismatch"),
        Some("bad parameter or other API misuse"),
        None,
        Some("authorization denied"),
        None,
        Some("column index out of range"),
        Some("file is not a database"),
        Some("notification message"),
        Some("warning message"),
    ];
    match rc {
        // SQLITE_ABORT_ROLLBACK, SQLITE_ROW, SQLITE_DONE
        516 => "abort due to ROLLBACK",
        100 => "another row available",
        101 => "no more rows available",
        rc => usize::try_from(rc & 0xff)
            .ok()
            .and_then(|i| MESSAGES.get(i).copied().flatten())
            .unwrap_or("unknown error"),
    }
}

/// Where a database's corruption markers go: only `netdata-meta.db` (`db_meta`) has them, in the cache directory.
#[derive(Debug, Clone, Default)]
pub struct Markers(Option<PathBuf>);

impl Markers {
    pub fn meta(cache_dir: &Path) -> Markers {
        Markers(Some(cache_dir.to_path_buf()))
    }

    /// `mark_database_to_recover()`: `.netdata-meta.db.recover` for a corrupt database, `.delete` otherwise, for the
    /// next start to act on. False when this database has no markers or the file cannot be created.
    pub fn mark(&self, rc: i32) -> bool {
        let Some(dir) = &self.0 else {
            return false;
        };
        let kind = if rc == SQLITE_CORRUPT {
            "recover"
        } else {
            "delete"
        };
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(dir.join(format!(".netdata-meta.db.{kind}")))
            .is_ok()
    }
}

/// How `init_database_batch()` failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchError {
    /// Corrupt or not a database: marked for the next start.
    Corrupt,
    Failed,
}

/// `sqlite3_exec()` without a callback: every statement of `sql`, each stepped until it is done. rusqlite's
/// `execute_batch()` steps a statement once, which is not enough for one that works a step at a time:
/// `PRAGMA incremental_vacuum(N)` frees one page per step.
pub fn exec(conn: &Connection, sql: &str) -> rusqlite::Result<()> {
    let mut batch = rusqlite::Batch::new(conn, sql);
    while let Some(mut stmt) = batch.next()? {
        let mut rows = stmt.raw_query();
        while rows.next()?.is_some() {}
    }
    Ok(())
}

/// `init_database_batch()`: each statement in turn; the first failure is recorded with its statement.
pub fn init_database_batch(
    conn: &Connection,
    batch: &[&str],
    markers: &Markers,
) -> Result<(), BatchError> {
    for sql in batch {
        let Err(err) = exec(conn, sql) else {
            continue;
        };
        let rc = result_code(&err);
        netdata_log_error!(
            "SQLite error during database initialization, rc = {rc} ({})",
            message(&err)
        );
        netdata_log_error!("SQLite failed statement {sql}");
        if rc == SQLITE_CORRUPT || rc == SQLITE_NOTADB {
            if markers.mark(rc) {
                netdata_log_error!("Database is corrupted will attempt to fix");
            }
            return Err(BatchError::Corrupt);
        }
        return Err(BatchError::Failed);
    }
    Ok(())
}

/// `db_execute()`: retried while the database is busy or locked, each failure recorded; a corrupt database is
/// marked. The error is the last result code.
pub fn db_execute(conn: &Connection, sql: &str, markers: &Markers) -> Result<(), i32> {
    let mut attempt = 0;
    loop {
        let Err(err) = exec(conn, sql) else {
            return Ok(());
        };
        attempt += 1;
        let rc = result_code(&err);
        let msg = match &err {
            rusqlite::Error::SqliteFailure(_, Some(msg))
            | rusqlite::Error::SqlInputError { msg, .. } => msg.as_str(),
            _ => "unknown",
        };
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "Failed to execute '{sql}', rc = {rc} ({msg}) -- attempt {attempt}"
        );
        if (rc == SQLITE_BUSY || rc == SQLITE_LOCKED) && attempt < MAX_RETRY {
            std::thread::sleep(RETRY_DELAY);
            continue;
        }
        if rc == SQLITE_CORRUPT {
            markers.mark(rc);
        }
        return Err(rc);
    }
}

/// `sqlite3_step_monitored()`: `f` again while the database is busy or locked, up to `MAX_RETRY` times. rusqlite
/// resets a statement whose step failed, so `f` runs the statement from its start.
pub fn retry<T>(mut f: impl FnMut() -> rusqlite::Result<T>) -> rusqlite::Result<T> {
    let mut attempt = 1;
    loop {
        match f() {
            Err(err)
                if matches!(result_code(&err), SQLITE_BUSY | SQLITE_LOCKED)
                    && attempt < MAX_RETRY =>
            {
                attempt += 1;
                std::thread::sleep(RETRY_DELAY);
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn result_codes_read_as_sqlites_texts() {
        let cases = HashMap::from([
            (0, "not an error"),
            (5, "database is locked"),
            (11, "database disk image is malformed"),
            (26, "file is not a database"),
            (2, "unknown error"),
            (22, "unknown error"),
            (100, "another row available"),
            (101, "no more rows available"),
            (516, "abort due to ROLLBACK"),
            // an extended code reads as its primary one
            (266, "disk I/O error"),
            (99, "unknown error"),
        ]);
        for (rc, text) in cases {
            assert_eq!(errstr(rc), text, "{rc}");
        }
    }

    fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
        records.into_iter().filter_map(|r| r.message).collect()
    }

    #[test]
    fn a_file_that_is_not_a_database_is_marked_for_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("netdata-meta.db");
        std::fs::write(&db, vec![0x5au8; 8192]).unwrap();
        let conn = Connection::open(&db).unwrap();
        let (result, records) = netdata_agent_log::capture(|| {
            init_database_batch(
                &conn,
                &["PRAGMA auto_vacuum=INCREMENTAL"],
                &Markers::meta(dir.path()),
            )
        });
        assert_eq!(result, Err(BatchError::Corrupt));
        assert_eq!(
            messages(records),
            [
                "SQLite error during database initialization, rc = 26 (file is not a database)",
                "SQLite failed statement PRAGMA auto_vacuum=INCREMENTAL",
                "Database is corrupted will attempt to fix",
            ]
        );
        let marker = std::fs::metadata(dir.path().join(".netdata-meta.db.delete")).unwrap();
        assert_eq!(marker.len(), 0);
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&marker.permissions()) & 0o777,
            0o600
        );
        // a database without markers only records the failure
        let (_, records) = netdata_agent_log::capture(|| {
            init_database_batch(&conn, &["SELECT 1"], &Markers::default())
        });
        assert_eq!(messages(records).len(), 2);
    }

    #[test]
    fn a_failing_statement_stops_the_batch() {
        let conn = Connection::open_in_memory().unwrap();
        let (result, records) = netdata_agent_log::capture(|| {
            init_database_batch(
                &conn,
                &[
                    "CREATE TABLE t(a)",
                    "INSERT INTO nope VALUES (1)",
                    "DROP TABLE t",
                ],
                &Markers::default(),
            )
        });
        assert_eq!(result, Err(BatchError::Failed));
        assert_eq!(
            messages(records),
            [
                "SQLite error during database initialization, rc = 1 (no such table: nope)",
                "SQLite failed statement INSERT INTO nope VALUES (1)",
            ]
        );
        conn.execute_batch("SELECT a FROM t").unwrap();
    }

    #[test]
    fn a_busy_database_is_retried_and_each_attempt_recorded() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("x.db");
        let holder = Connection::open(&db).unwrap();
        holder
            .execute_batch("CREATE TABLE t(a); BEGIN EXCLUSIVE")
            .unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.busy_timeout(Duration::ZERO).unwrap();
        let (result, records) = netdata_agent_log::capture(|| {
            db_execute(&conn, "INSERT INTO t VALUES (1)", &Markers::default())
        });
        assert_eq!(result, Err(SQLITE_BUSY));
        let records = messages(records);
        assert_eq!(records.len(), MAX_RETRY);
        assert_eq!(
            records.last().unwrap(),
            "Failed to execute 'INSERT INTO t VALUES (1)', rc = 5 (database is locked) -- attempt 100"
        );
        let mut calls = 0;
        let result = retry(|| {
            calls += 1;
            if calls == 3 {
                holder.execute_batch("COMMIT").unwrap();
            }
            conn.execute("INSERT INTO t VALUES (2)", [])
        });
        assert_eq!(result.unwrap(), 1);
        assert_eq!(calls, 3);
    }
}
