//! Opening and closing the two databases: `sql_init_meta_database()` (`sqlite_metadata.c`),
//! `sql_init_context_database()` (`sqlite_context.c`), `configure_sqlite_database()` and `sql_close_database()`
//! (`sqlite_functions.c`), with C's records. `netdata-meta.db` is shared by every thread behind one lock, as C shares
//! its serialized handle.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error, netdata_log_info};
use rusqlite::{Connection, OpenFlags};

use crate::conn::{self, Markers, init_database_batch};
use crate::{functions, migrate, recover, schema};

/// `def_journal_size_limit`.
pub const DEFAULT_JOURNAL_SIZE_LIMIT: i64 = 16_777_216;

/// `SQLITE_BUSY_DELAY_MS`: `netdata-meta.db`'s busy timeout once it is initialized.
const BUSY_TIMEOUT: Duration = Duration::from_millis(30);

/// The `[sqlite]` keys of netdata.conf that exist: C reads each only when it is there, so the others never appear in
/// the configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqliteSettings {
    pub auto_vacuum: Option<String>,
    pub synchronous: Option<String>,
    pub journal_mode: Option<String>,
    pub temp_store: Option<String>,
    pub journal_size_limit: Option<i64>,
    pub cache_size: Option<i64>,
}

/// `sqlite3_open()`: read-write, created when missing, serialized; no busy timeout (rusqlite sets one).
fn open_rw(path: &Path) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
    )?;
    conn.busy_timeout(Duration::ZERO)?;
    Ok(conn)
}

fn open_failed(path: &Path, err: &rusqlite::Error) {
    netdata_log_error!(
        "Failed to initialize database at {}, due to \"{}\"",
        path.display(),
        conn::errstr(conn::result_code(err))
    );
}

/// `configure_sqlite_database()`: the pragmas (each overridable by its `[sqlite]` key), the version, then `optimize`.
fn configure(
    c: &Connection,
    target: i32,
    settings: &SqliteSettings,
    markers: &Markers,
) -> Result<(), ()> {
    let text = |value: &Option<String>, default: &str| {
        value.clone().unwrap_or_else(|| default.to_string())
    };
    let statements = [
        format!(
            "PRAGMA auto_vacuum={}",
            text(&settings.auto_vacuum, "INCREMENTAL")
        ),
        format!(
            "PRAGMA synchronous={}",
            text(&settings.synchronous, "NORMAL")
        ),
        format!(
            "PRAGMA journal_mode={}",
            text(&settings.journal_mode, "WAL")
        ),
        format!("PRAGMA temp_store={}", text(&settings.temp_store, "MEMORY")),
        format!(
            "PRAGMA journal_size_limit={}",
            settings
                .journal_size_limit
                .unwrap_or(DEFAULT_JOURNAL_SIZE_LIMIT)
        ),
        format!("PRAGMA cache_size={}", settings.cache_size.unwrap_or(-2000)),
        format!("PRAGMA user_version={target}"),
        "PRAGMA optimize=0x10002".to_string(),
    ];
    for sql in &statements {
        init_database_batch(c, &[sql], markers).map_err(|_| ())?;
    }
    Ok(())
}

/// `sql_close_database()`.
fn close(c: Connection, name: &str) {
    let _ = conn::db_execute(&c, "PRAGMA optimize", &Markers::default());
    netdata_log_info!("{name}: Closing sqlite database");
    if let Err((_, err)) = c.close() {
        let rc = conn::result_code(&err);
        netdata_log_error!(
            "{name}: Error while closing the sqlite database: rc {rc}, error \"{}\"",
            conn::errstr(rc)
        );
    }
}

/// The command-line maintenance of `netdata-meta.db` (`db_check_action_type_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    Recover,
    ReclaimSpace,
    Analyze,
}

/// `netdata-meta.db` (`db_meta`).
pub struct MetaDb {
    conn: Mutex<Connection>,
    cache_dir: PathBuf,
}

impl MetaDb {
    pub fn path(cache_dir: &Path) -> PathBuf {
        cache_dir.join("netdata-meta.db")
    }

    /// The start-up markers: `.recover` recovers the database in place, then `.delete` renames it to
    /// `netdata-meta.bad` for a fresh start.
    fn apply_markers(cache_dir: &Path) {
        if std::fs::remove_file(cache_dir.join(".netdata-meta.db.recover")).is_ok() {
            recover::recover_database(
                &Self::path(cache_dir),
                &cache_dir.join("netdata-meta-recover.db"),
            );
        }
        if std::fs::remove_file(cache_dir.join(".netdata-meta.db.delete")).is_ok() {
            let (from, to) = (Self::path(cache_dir), cache_dir.join("netdata-meta.bad"));
            if std::fs::rename(&from, &to).is_err() {
                netdata_log_error!("Failed to rename {} to {}", from.display(), to.display());
            }
        }
    }

    /// `sql_init_meta_database(DB_CHECK_NONE)`: `None` after a failure, which C's records already describe.
    pub fn open(cache_dir: &Path, settings: &SqliteSettings) -> Option<MetaDb> {
        Self::apply_markers(cache_dir);
        let path = Self::path(cache_dir);
        let c = match open_rw(&path) {
            Ok(c) => c,
            Err(err) => {
                open_failed(&path, &err);
                return None;
            }
        };
        netdata_log_info!("SQLite database {} initialization", path.display());
        functions::register(&c);
        let markers = Markers::meta(cache_dir);
        let target = migrate::meta(&c, &markers);
        configure(&c, target, settings, &markers).ok()?;
        init_database_batch(&c, schema::META_CONFIG, &markers).ok()?;
        init_database_batch(&c, schema::META_CLEANUP, &markers).ok()?;
        netdata_log_info!("SQLite database initialization completed");
        if c.busy_timeout(BUSY_TIMEOUT).is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "SQLITE: Failed to set busy timeout to {} ms",
                BUSY_TIMEOUT.as_millis()
            );
        }
        Some(MetaDb {
            conn: Mutex::new(c),
            cache_dir: cache_dir.to_path_buf(),
        })
    }

    /// `-W sqlite-meta-recover`, `sqlite-compact` and `sqlite-analyze`: `sql_init_meta_database()` with its check.
    /// Recover runs whether or not a marker asked for it; the other two run after the markers and end with the
    /// database closed.
    pub fn check(cache_dir: &Path, check: Check) {
        let path = Self::path(cache_dir);
        if check == Check::Recover {
            let _ = std::fs::remove_file(cache_dir.join(".netdata-meta.db.recover"));
            recover::recover_database(&path, &cache_dir.join("netdata-meta-recover.db"));
            return;
        }
        Self::apply_markers(cache_dir);
        let c = match open_rw(&path) {
            Ok(c) => c,
            Err(err) => {
                open_failed(&path, &err);
                return;
            }
        };
        let sql = if check == Check::ReclaimSpace {
            netdata_log_info!("Reclaiming space of {}", path.display());
            "VACUUM"
        } else {
            netdata_log_info!("Running ANALYZE on {}", path.display());
            "ANALYZE"
        };
        match c.execute_batch(sql) {
            Err(err) => netdata_log_error!(
                "Failed to execute {sql} rc = {} ({})",
                conn::result_code(&err),
                conn::message(&err)
            ),
            Ok(()) => {
                let _ = conn::db_execute(
                    &c,
                    "select count(*) from sqlite_master limit 0",
                    &Markers::default(),
                );
            }
        }
    }

    /// The connection, under the lock every user of `db_meta` takes.
    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The corruption markers of this database.
    pub fn markers(&self) -> Markers {
        Markers::meta(&self.cache_dir)
    }

    /// `sql_delete_aclk_table_list()`: the `aclk_alert_*` tables, triggers and indexes of old agents, dropped on
    /// every start.
    pub fn drop_legacy_aclk_tables(&self) {
        let c = self.lock();
        let sql = conn::retry(|| {
            let mut stmt = c.prepare(
                "SELECT 'DROP '||type||' IF EXISTS '||name||';' FROM sqlite_schema WHERE name LIKE 'aclk_alert_%' AND \
                 type IN ('table', 'trigger', 'index')",
            )?;
            let mut sql = String::new();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                sql.push_str(&row.get::<_, Option<String>>(0)?.unwrap_or_default());
            }
            Ok(sql)
        });
        let Ok(sql) = sql else { return };
        if conn::db_execute(&c, &sql, &self.markers()).is_err() {
            netdata_log_error!("Failed to drop unused ACLK tables");
        }
    }

    /// Closes it as `sqlite_close_databases()` does, after the context database.
    pub fn close(self) {
        let c = self
            .conn
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        close(c, "METADATA");
    }
}

/// `context-meta.db` (`db_context_meta`), with `netdata-meta.db` attached as `meta`.
pub struct ContextDb {
    conn: Mutex<Connection>,
}

impl ContextDb {
    /// `sql_init_context_database()`: `None` after a failure; it has no markers and no busy timeout.
    pub fn open(cache_dir: &Path, settings: &SqliteSettings) -> Option<ContextDb> {
        let path = cache_dir.join("context-meta.db");
        let c = match open_rw(&path) {
            Ok(c) => c,
            Err(err) => {
                open_failed(&path, &err);
                return None;
            }
        };
        netdata_log_info!("SQLite database {} initialization", path.display());
        let none = Markers::default();
        let target = migrate::context(&c);
        configure(&c, target, settings, &none).ok()?;
        let attach = format!(
            "ATTACH DATABASE \"{}\" as meta",
            MetaDb::path(cache_dir).display()
        );
        init_database_batch(&c, &[&attach], &none).ok()?;
        init_database_batch(&c, schema::CONTEXT_CONFIG, &none).ok()?;
        init_database_batch(&c, schema::CONTEXT_CLEANUP, &none).ok()?;
        Some(ContextDb {
            conn: Mutex::new(c),
        })
    }

    pub fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn close(self) {
        let c = self
            .conn
            .into_inner()
            .unwrap_or_else(PoisonError::into_inner);
        close(c, "CONTEXT");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
        records.into_iter().filter_map(|r| r.message).collect()
    }

    fn pragma(c: &Connection, name: &str) -> String {
        c.query_row(&format!("PRAGMA {name}"), [], |r| {
            Ok(match r.get_ref(0)? {
                rusqlite::types::ValueRef::Integer(i) => i.to_string(),
                v => String::from_utf8_lossy(v.as_bytes().unwrap_or_default()).into_owned(),
            })
        })
        .unwrap()
    }

    #[test]
    fn fresh_databases_open_as_cs() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display().to_string();
        let (meta, records) =
            netdata_agent_log::capture(|| MetaDb::open(dir.path(), &SqliteSettings::default()));
        let meta = meta.unwrap();
        assert_eq!(
            messages(records),
            [
                format!("SQLite database {d}/netdata-meta.db initialization"),
                "SQLite database initialization completed".to_string(),
            ]
        );
        {
            let c = meta.lock();
            let got: Vec<String> = [
                "auto_vacuum",
                "synchronous",
                "journal_mode",
                "journal_size_limit",
                "user_version",
            ]
            .iter()
            .map(|p| pragma(&c, p))
            .collect();
            assert_eq!(got, ["2", "1", "wal", "16777216", "18"]);
        }
        let (context, records) =
            netdata_agent_log::capture(|| ContextDb::open(dir.path(), &SqliteSettings::default()));
        let context = context.unwrap();
        assert_eq!(
            messages(records),
            [format!(
                "SQLite database {d}/context-meta.db initialization"
            )]
        );
        {
            let c = context.lock();
            assert_eq!(pragma(&c, "user_version"), "1");
            let attached: i64 = c
                .query_row("SELECT count(*) FROM meta.host", [], |r| r.get(0))
                .unwrap();
            assert_eq!(attached, 0);
        }
        let (_, records) = netdata_agent_log::capture(|| {
            context.close();
            meta.close();
        });
        assert_eq!(
            messages(records),
            [
                "CONTEXT: Closing sqlite database",
                "METADATA: Closing sqlite database"
            ]
        );
        // a second start finds the current versions
        let (meta, records) =
            netdata_agent_log::capture(|| MetaDb::open(dir.path(), &SqliteSettings::default()));
        assert_eq!(
            messages(records)[1],
            "metadata database version is 18 (no migration needed)"
        );
        meta.unwrap().close();
    }

    #[test]
    fn sqlite_settings_override_the_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let settings = SqliteSettings {
            synchronous: Some("FULL".into()),
            journal_mode: Some("DELETE".into()),
            journal_size_limit: Some(1000),
            cache_size: Some(-500),
            ..SqliteSettings::default()
        };
        let meta = MetaDb::open(dir.path(), &settings).unwrap();
        let c = meta.lock();
        let got: Vec<String> = [
            "synchronous",
            "journal_mode",
            "journal_size_limit",
            "cache_size",
        ]
        .iter()
        .map(|p| pragma(&c, p))
        .collect();
        assert_eq!(got, ["2", "delete", "1000", "-500"]);
    }

    #[test]
    fn a_file_that_is_not_a_database_is_renamed_at_the_next_start() {
        let dir = tempfile::tempdir().unwrap();
        let db = MetaDb::path(dir.path());
        std::fs::write(&db, vec![0x5au8; 8192]).unwrap();
        let (meta, records) =
            netdata_agent_log::capture(|| MetaDb::open(dir.path(), &SqliteSettings::default()));
        assert!(meta.is_none());
        assert_eq!(
            messages(records)[1..],
            [
                "Failed to get user version for database",
                "Error checking database table count; file is not a database",
                "SQLite error during database initialization, rc = 26 (file is not a database)",
                "SQLite failed statement PRAGMA auto_vacuum=INCREMENTAL",
                "Database is corrupted will attempt to fix",
            ]
        );
        assert!(dir.path().join(".netdata-meta.db.delete").exists());
        let (meta, records) =
            netdata_agent_log::capture(|| MetaDb::open(dir.path(), &SqliteSettings::default()));
        assert!(meta.is_some());
        assert_eq!(messages(records).len(), 2);
        assert!(dir.path().join("netdata-meta.bad").exists());
        assert!(!dir.path().join(".netdata-meta.db.delete").exists());
    }

    #[test]
    fn a_recover_marker_recovers_before_the_open() {
        let dir = tempfile::tempdir().unwrap();
        MetaDb::open(dir.path(), &SqliteSettings::default())
            .unwrap()
            .close();
        std::fs::write(dir.path().join(".netdata-meta.db.recover"), b"").unwrap();
        let (meta, records) =
            netdata_agent_log::capture(|| MetaDb::open(dir.path(), &SqliteSettings::default()));
        assert!(meta.is_some());
        let messages = messages(records);
        assert_eq!(
            messages[0],
            format!("Recover {}", MetaDb::path(dir.path()).display())
        );
        assert_eq!(messages[2], "Recover complete");
        assert!(!dir.path().join(".netdata-meta.db.recover").exists());
    }

    #[test]
    fn the_command_line_checks() {
        let dir = tempfile::tempdir().unwrap();
        MetaDb::open(dir.path(), &SqliteSettings::default())
            .unwrap()
            .close();
        let db = MetaDb::path(dir.path()).display().to_string();
        let run =
            |check| messages(netdata_agent_log::capture(|| MetaDb::check(dir.path(), check)).1);
        assert_eq!(
            run(Check::ReclaimSpace),
            [format!("Reclaiming space of {db}")]
        );
        assert_eq!(run(Check::Analyze), [format!("Running ANALYZE on {db}")]);
        let recovered = run(Check::Recover);
        assert_eq!(
            recovered[..3],
            [
                format!("Recover {db}"),
                format!("     to {}/netdata-meta-recover.db", dir.path().display()),
                "Recover complete".to_string()
            ]
        );
    }

    #[test]
    fn legacy_aclk_tables_are_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        meta.lock()
            .execute_batch(
                "CREATE TABLE aclk_alert_x(a); CREATE INDEX aclk_alert_x_i ON aclk_alert_x(a);
                 CREATE TABLE aclk_queue_keep(a)",
            )
            .unwrap();
        meta.drop_legacy_aclk_tables();
        let left: Vec<String> = {
            let c = meta.lock();
            let mut stmt = c
                .prepare("SELECT name FROM sqlite_schema WHERE name LIKE 'aclk%' ORDER BY name")
                .unwrap();
            stmt.query_map([], |r| r.get(0))
                .unwrap()
                .map(Result::unwrap)
                .collect()
        };
        assert_eq!(left, ["aclk_queue", "aclk_queue_keep"]);
    }
}
