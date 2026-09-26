//! The migrations of `src/database/sqlite/sqlite_db_migration.c`: `netdata-meta.db` from any version to 18, the
//! health log tables of version 8 included (`health_migrate_old_health_log_table()` of `sqlite_health.c`), and
//! `context-meta.db` to 1. The version a migration returns is the one the configure step writes.

use netdata_agent_log::{netdata_log_error, netdata_log_info};
use netdata_agent_text::parse::uuid_parse_flexi;
use rusqlite::Connection;
use rusqlite::types::ValueRef;

use crate::conn::{self, Markers, SQLITE_CORRUPT, init_database_batch};
use crate::schema;

/// `DB_METADATA_VERSION` and `DB_CONTEXT_METADATA_VERSION`.
pub const META_VERSION: i32 = 18;
pub const CONTEXT_VERSION: i32 = 1;

/// `%Q` of `sqlite3_mprintf()`.
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// `%w`: the text of a double-quoted identifier.
fn ident(s: &str) -> String {
    s.replace('"', "\"\"")
}

/// `sqlite3_exec()` with `return_int_cb`: the first column of the last row, read as `str2uint32_t()` reads it.
fn exec_int(c: &Connection, sql: &str) -> Result<i32, rusqlite::Error> {
    let mut stmt = c.prepare(sql)?;
    let mut rows = stmt.query([])?;
    let mut value = 0;
    while let Some(row) = rows.next()? {
        let text = match row.get_ref(0)? {
            ValueRef::Integer(i) => i.to_string(),
            ValueRef::Real(r) => r.to_string(),
            ValueRef::Text(t) | ValueRef::Blob(t) => String::from_utf8_lossy(t).into_owned(),
            ValueRef::Null => String::new(),
        };
        let digits: String = text
            .trim_start()
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        value = digits.parse::<u64>().map_or(0, |v| v as u32 as i32);
    }
    Ok(value)
}

/// An int query whose failure is recorded as `<what>; <message>` and reads as 0.
fn int_or_record(c: &Connection, sql: &str, what: &str) -> i32 {
    exec_int(c, sql).unwrap_or_else(|err| {
        netdata_log_info!("{what}; {}", conn::message(&err));
        0
    })
}

/// `db_table_count()`.
fn table_count(c: &Connection) -> i32 {
    int_or_record(
        c,
        "select count(1) from sqlite_schema where type = 'table'",
        "Error checking database table count",
    )
}

/// `table_exists_in_database()`.
pub fn table_exists(c: &Connection, table: &str) -> bool {
    let sql = format!(
        "select 1 from sqlite_schema where type = 'table' and name = {}",
        quote(table)
    );
    int_or_record(c, &sql, "Error checking table existence") != 0
}

/// `column_exists_in_table()`.
fn column_exists(c: &Connection, table: &str, column: &str) -> bool {
    let sql = format!(
        "SELECT 1 FROM pragma_table_info({}) where name = {}",
        quote(table),
        quote(column)
    );
    int_or_record(c, &sql, "Error checking column existence") != 0
}

/// `get_database_user_version()`.
fn user_version(c: &Connection) -> i32 {
    exec_int(c, "PRAGMA user_version").unwrap_or_else(|_| {
        netdata_log_error!("Failed to get user version for database");
        0
    })
}

/// A migration step: 0 on success.
type Step = fn(&Connection, &Markers) -> i32;

fn batch(c: &Connection, statements: &[&str], markers: &Markers) -> i32 {
    match init_database_batch(c, statements, markers) {
        Ok(()) => 0,
        Err(conn::BatchError::Corrupt) => SQLITE_CORRUPT,
        Err(conn::BatchError::Failed) => 1,
    }
}

/// A batch run when `table` exists without `column`.
fn batch_if_missing(
    c: &Connection,
    table: &str,
    column: &str,
    statements: &[&str],
    markers: &Markers,
) -> i32 {
    if table_exists(c, table) && !column_exists(c, table, column) {
        return batch(c, statements, markers);
    }
    0
}

/// `add_column_to_matching_tables()`: each statement for every table matching `like` that lacks `column`, run while
/// the table list is being read, as C does; their failures are ignored.
fn add_column_to_matching_tables(
    c: &Connection,
    like: &str,
    column: &str,
    statements: &[&str],
    what: &str,
) -> i32 {
    let sql = format!(
        "SELECT name FROM sqlite_schema WHERE type ='table' AND name LIKE {}",
        quote(like)
    );
    let Ok(mut stmt) = c.prepare(&sql) else {
        netdata_log_error!("Failed to prepare statement to alter {what} tables");
        return 1;
    };
    let Ok(mut rows) = stmt.query([]) else {
        return 0;
    };
    while let Ok(Some(row)) = rows.next() {
        let Ok(Some(table)) = row.get::<_, Option<String>>(0) else {
            continue;
        };
        if !column_exists(c, &table, column) {
            for fmt in statements {
                let _ = c.execute_batch(&fmt.replace("%w", &ident(&table)));
            }
        }
    }
    0
}

/// `execute_on_matching_names()`: the statement for every name `select` returns, run together through `db_execute()`.
fn execute_on_matching_names(
    c: &Connection,
    select: &str,
    fmt: &str,
    what: &str,
    markers: &Markers,
) -> i32 {
    let Ok(mut stmt) = c.prepare(select) else {
        netdata_log_error!("Failed to prepare statement to {what}");
        return 1;
    };
    let mut sql = String::new();
    if let Ok(mut rows) = stmt.query([]) {
        while let Ok(Some(row)) = rows.next() {
            if let Ok(Some(name)) = row.get::<_, Option<String>>(0) {
                sql.push_str(&fmt.replace("%w", &ident(&name)));
            }
        }
    }
    drop(stmt);
    if !sql.is_empty() {
        let _ = conn::db_execute(c, &sql, markers);
    }
    0
}

fn noop(_: &Connection, _: &Markers) -> i32 {
    0
}

fn v1_v2(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(c, "host", "hops", schema::MIGRATE_V1_V2, m)
}

fn v2_v3(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(c, "host", "memory_mode", schema::MIGRATE_V2_V3, m)
}

fn v3_v4(c: &Connection, _: &Markers) -> i32 {
    add_column_to_matching_tables(
        c,
        "health_log_%",
        "chart_context",
        &["ALTER TABLE \"%w\" ADD chart_context text"],
        "health_log",
    )
}

fn v4_v5(c: &Connection, m: &Markers) -> i32 {
    batch(c, schema::MIGRATE_V4_V5, m)
}

fn v5_v6(c: &Connection, m: &Markers) -> i32 {
    batch(c, schema::MIGRATE_V5_V6, m)
}

fn v6_v7(c: &Connection, _: &Markers) -> i32 {
    add_column_to_matching_tables(
        c,
        "aclk_alert_%",
        "filtered_alert_unique_id",
        &[
            "ALTER TABLE \"%w\" ADD filtered_alert_unique_id",
            "UPDATE \"%w\" SET filtered_alert_unique_id = alert_unique_id",
        ],
        "aclk_alert",
    )
}

fn v7_v8(c: &Connection, _: &Markers) -> i32 {
    add_column_to_matching_tables(
        c,
        "health_log_%",
        "transition_id",
        &["ALTER TABLE \"%w\" ADD transition_id blob"],
        "health_log",
    )
}

/// `do_migration_v8_v9()`: the shared health log tables, then every per-host `health_log_<guid>` table copied into
/// them and dropped.
fn v8_v9(c: &Connection, m: &Markers) -> i32 {
    for sql in [
        "CREATE TABLE IF NOT EXISTS health_log (health_log_id INTEGER PRIMARY KEY, host_id blob, alarm_id int, \
         config_hash_id blob, name text, chart text, family text, recipient text, units text, exec text, \
         chart_context text, last_transition_id blob, UNIQUE (host_id, alarm_id))",
        "CREATE INDEX IF NOT EXISTS health_log_ind_1 ON health_log (host_id)",
        "CREATE TABLE IF NOT EXISTS health_log_detail (health_log_id int, unique_id int, alarm_id int, \
         alarm_event_id int, updated_by_id int, updates_id int, when_key int, duration int, non_clear_duration int, \
         flags int, exec_run_timestamp int, delay_up_to_timestamp int, info text, exec_code int, new_status real, \
         old_status real, delay int, new_value double, old_value double, last_repeat int, transition_id blob, \
         global_id int, summary text, host_id blob)",
        "CREATE INDEX IF NOT EXISTS health_log_d_ind_1 ON health_log_detail (unique_id)",
        "CREATE INDEX IF NOT EXISTS health_log_d_ind_2 ON health_log_detail (global_id)",
        "CREATE INDEX IF NOT EXISTS health_log_d_ind_3 ON health_log_detail (transition_id)",
        "CREATE INDEX IF NOT EXISTS health_log_d_ind_4 ON health_log_detail (health_log_id)",
        "ALTER TABLE alert_hash ADD source text",
        "CREATE INDEX IF NOT EXISTS alert_hash_index ON alert_hash (hash_id)",
    ] {
        let _ = c.execute_batch(sql);
    }
    let Ok(mut stmt) = c.prepare(
        "SELECT name FROM sqlite_schema WHERE type ='table' AND name LIKE 'health_log_%' AND name <> \
         'health_log_detail'",
    ) else {
        netdata_log_error!("Failed to prepare statement to alter health_log tables");
        return 1;
    };
    let mut migrated: Vec<String> = Vec::new();
    if let Ok(mut rows) = stmt.query([]) {
        while let Ok(Some(row)) = rows.next() {
            if let Ok(Some(table)) = row.get::<_, Option<String>>(0)
                && migrate_old_health_log_table(c, &table, m)
                && !migrated.contains(&table)
            {
                migrated.push(table);
            }
        }
    }
    drop(stmt);
    for table in &migrated {
        // sql_drop_table()
        if let Err(err) = c.execute_batch(&format!("DROP table {table}")) {
            netdata_log_error!(
                "DES SQLite error during drop table operation for {table}, rc = {}",
                conn::result_code(&err)
            );
        }
    }
    let _ = c.execute_batch("ALTER TABLE health_log_detail DROP COLUMN host_id");
    0
}

/// `execute_insert()`: one step; a corrupt database is marked and recorded.
fn execute_insert(
    c: &Connection,
    sql: &str,
    host: Option<&[u8; 16]>,
    m: &Markers,
) -> Result<(), i32> {
    let mut stmt = c.prepare(sql).map_err(|e| -conn::result_code(&e))?;
    let result = conn::retry(|| match host {
        Some(host) => stmt.execute(rusqlite::params![&host[..]]),
        None => stmt.execute([]),
    });
    match result {
        Ok(_) => Ok(()),
        Err(err) => {
            let rc = conn::result_code(&err);
            if rc == SQLITE_CORRUPT {
                m.mark(rc);
                netdata_log_error!("SQLite error {rc}");
            }
            Err(rc)
        }
    }
}

/// `health_migrate_old_health_log_table()`: a `health_log_<guid>` table (underscores for dashes) copied into the
/// shared tables. True when it may be dropped.
fn migrate_old_health_log_table(c: &Connection, table: &str, m: &Markers) -> bool {
    if table.len() < 46 {
        return false;
    }
    let mut guid = table.as_bytes()[11..].to_vec();
    for i in [8, 13, 18, 23] {
        if let Some(b) = guid.get_mut(i) {
            *b = b'-';
        }
    }
    let Some(host) = uuid_parse_flexi(&guid) else {
        return false;
    };
    let copy = format!(
        "INSERT OR IGNORE INTO health_log (host_id, alarm_id, config_hash_id, name, chart, family, exec, recipient, \
         units, chart_context) SELECT ?1, alarm_id, config_hash_id, name, chart, family, exec, recipient, units, \
         chart_context from {table}"
    );
    match execute_insert(c, &copy, Some(&host), m) {
        Err(rc) if rc < 0 => {
            netdata_log_error!(
                "Failed to prepare statement to copy health log, rc = {}",
                -rc
            );
            return false;
        }
        Err(rc) => netdata_log_error!("Failed to execute SQL_COPY_HEALTH_LOG, rc = {rc}"),
        Ok(()) => {}
    }
    let detail = format!(
        "INSERT INTO health_log_detail (unique_id, alarm_id, alarm_event_id, updated_by_id, updates_id, when_key, \
         duration, non_clear_duration, flags, exec_run_timestamp, delay_up_to_timestamp, info, exec_code, new_status, \
         old_status, delay, new_value, old_value, last_repeat, transition_id, global_id, host_id) SELECT unique_id, \
         alarm_id, alarm_event_id, updated_by_id, updates_id, when_key, duration, non_clear_duration, flags, \
         exec_run_timestamp, delay_up_to_timestamp, info, exec_code, new_status, old_status, delay, new_value, \
         old_value, last_repeat, transition_id, now_usec(1), ?1 from {table}"
    );
    match execute_insert(c, &detail, Some(&host), m) {
        Err(rc) if rc < 0 => {
            netdata_log_error!(
                "Failed to prepare statement to copy health log detail, rc = {}",
                -rc
            );
            return false;
        }
        Err(rc) => {
            netdata_log_error!("Failed to execute SQL_COPY_HEALTH_LOG_DETAIL, rc = {rc}");
            return false;
        }
        Ok(()) => {}
    }
    match execute_insert(
        c,
        "update health_log_detail set transition_id = uuid_random() where transition_id is null",
        None,
        m,
    ) {
        Err(rc) if rc < 0 => {
            netdata_log_error!(
                "Failed to prepare statement to update health log detail with transition ids, rc = {}",
                -rc
            );
            return false;
        }
        Err(rc) => {
            netdata_log_error!(
                "Failed to execute SQL_UPDATE_HEALTH_LOG_DETAIL_TRANSITION_ID, rc = {rc}"
            );
            return false;
        }
        Ok(()) => {}
    }
    match execute_insert(
        c,
        "update health_log_detail set health_log_id = (select health_log_id from health_log where host_id = ?1 and \
         alarm_id = health_log_detail.alarm_id) where health_log_id is null and host_id = ?1",
        Some(&host),
        m,
    ) {
        Err(rc) if rc < 0 => {
            netdata_log_error!(
                "Failed to prepare statement to update health log detail with health log ids, rc = {}",
                -rc
            );
            return false;
        }
        Err(rc) => netdata_log_error!(
            "Failed to execute SQL_UPDATE_HEALTH_LOG_DETAIL_HEALTH_LOG_ID, rc = {rc}"
        ),
        Ok(()) => {}
    }
    match execute_insert(
        c,
        "update health_log set last_transition_id = (select transition_id from health_log_detail where \
         health_log_id = health_log.health_log_id and alarm_id = health_log.alarm_id group by (alarm_id) having \
         max(alarm_event_id)) where host_id = ?1",
        Some(&host),
        m,
    ) {
        Err(rc) if rc < 0 => {
            netdata_log_error!(
                "Failed to prepare statement to update health log  with last transition id, rc = {}",
                -rc
            );
            return false;
        }
        Err(rc) => netdata_log_error!(
            "Failed to execute SQL_UPDATE_HEALTH_LOG_LAST_TRANSITION_ID, rc = {rc}"
        ),
        Ok(()) => {}
    }
    true
}

fn v9_v10(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(c, "alert_hash", "chart_labels", schema::MIGRATE_V9_V10, m)
}

fn v10_v11(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(c, "health_log", "chart_name", schema::MIGRATE_V10_V11, m)
}

/// `MIGR_11_12_UPD_HEALTH_LOG_DETAIL`.
const UPDATE_DETAIL_SUMMARY: &str = "UPDATE health_log_detail SET summary = (select name from health_log where \
                                     health_log_id = health_log_detail.health_log_id)";

fn v11_v12(c: &Connection, m: &Markers) -> i32 {
    let mut rc = 0;
    if table_exists(c, "health_log_detail")
        && !column_exists(c, "health_log_detail", "summary")
        && table_exists(c, "alert_hash")
        && !column_exists(c, "alert_hash", "summary")
    {
        rc = batch(c, schema::MIGRATE_V11_V12, m);
    }
    if rc == 0 {
        let _ = c.execute_batch(UPDATE_DETAIL_SUMMARY);
    }
    rc
}

fn v12_v13(c: &Connection, m: &Markers) -> i32 {
    let mut rc = 0;
    if table_exists(c, "health_log_detail") && !column_exists(c, "health_log_detail", "summary") {
        rc = batch(c, schema::MIGRATE_V12_V13_DETAIL, m);
        let _ = c.execute_batch(UPDATE_DETAIL_SUMMARY);
    }
    if table_exists(c, "alert_hash") && !column_exists(c, "alert_hash", "summary") {
        rc = batch(c, schema::MIGRATE_V12_V13_HASH, m);
    }
    rc
}

fn v13_v14(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(c, "host", "last_connected", schema::MIGRATE_V13_V14, m)
}

fn v14_v15(c: &Connection, m: &Markers) -> i32 {
    execute_on_matching_names(
        c,
        "SELECT name FROM sqlite_schema WHERE type = \"index\" AND name LIKE \"aclk_alert_index@_%\" ESCAPE \"@\"",
        "DROP INDEX IF EXISTS \"%w\"; ",
        "drop unused indices",
        m,
    )
}

fn v15_v16(c: &Connection, m: &Markers) -> i32 {
    execute_on_matching_names(
        c,
        "SELECT name FROM sqlite_schema WHERE type = \"table\" AND name LIKE \"aclk_alert_%\"",
        "ANALYZE \"%w\"; ",
        "analyze aclk_alert tables",
        m,
    )
}

fn v16_v17(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(
        c,
        "alert_hash",
        "time_group_condition",
        schema::MIGRATE_V16_V17,
        m,
    )
}

fn v17_v18(c: &Connection, m: &Markers) -> i32 {
    batch_if_missing(
        c,
        "alert_hash",
        "time_group_condition",
        schema::MIGRATE_V17_V18,
        m,
    )
}

/// `migration_action`.
const META_STEPS: [(&str, Step); 18] = [
    ("v0 to v1", noop),
    ("v1 to v2", v1_v2),
    ("v2 to v3", v2_v3),
    ("v3 to v4", v3_v4),
    ("v4 to v5", v4_v5),
    ("v5 to v6", v5_v6),
    ("v6 to v7", v6_v7),
    ("v7 to v8", v7_v8),
    ("v8 to v9", v8_v9),
    ("v9 to v10", v9_v10),
    ("v10 to v11", v10_v11),
    ("v11 to v12", v11_v12),
    ("v12 to v13", v12_v13),
    ("v13 to v14", v13_v14),
    ("v14 to v15", v14_v15),
    ("v15 to v16", v15_v16),
    ("v16 to v17", v16_v17),
    ("v17 to v18", v17_v18),
];

/// `context_migration_action`.
const CONTEXT_STEPS: [(&str, Step); 1] = [("v0 to v1", noop)];

/// `migrate_database()`: the steps from the stored version to `target`; a failed step's version is returned. A
/// newer version runs none and returns `target`.
fn migrate(c: &Connection, target: i32, name: &str, steps: &[(&str, Step)], m: &Markers) -> i32 {
    let version = exec_int(c, "PRAGMA user_version").unwrap_or_else(|err| {
        netdata_log_info!(
            "Error checking the {name} database version; {}",
            conn::message(&err)
        );
        0
    });
    if version == target {
        netdata_log_info!("{name} database version is {target} (no migration needed)");
        return target;
    }
    netdata_log_info!(
        "Database version is {version}, current version is {target}. Running migration for {name} ..."
    );
    for i in version.max(0)..target {
        let Some((step_name, step)) = steps.get(i as usize) else {
            break;
        };
        netdata_log_info!("Running database \"{name}\" migration {step_name}");
        if step(c, m) != 0 {
            netdata_log_error!(
                "Database {name} migration from version {i} to version {} failed",
                i + 1
            );
            return i;
        }
    }
    target
}

/// `perform_database_migration()`: a fresh database (version 0, no tables) needs none.
pub fn meta(c: &Connection, markers: &Markers) -> i32 {
    if user_version(c) == 0 && table_count(c) == 0 {
        return META_VERSION;
    }
    migrate(c, META_VERSION, "metadata", &META_STEPS, markers)
}

/// `perform_context_database_migration()`: a fresh database (version 0, no `context` table) needs none.
pub fn context(c: &Connection) -> i32 {
    if user_version(c) == 0 && !table_exists(c, "context") {
        return CONTEXT_VERSION;
    }
    migrate(
        c,
        CONTEXT_VERSION,
        "context",
        &CONTEXT_STEPS,
        &Markers::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions;

    fn records(f: impl FnOnce() -> i32) -> (i32, Vec<String>) {
        let (version, records) = netdata_agent_log::capture(f);
        (
            version,
            records.into_iter().filter_map(|r| r.message).collect(),
        )
    }

    fn columns(c: &Connection, table: &str) -> Vec<String> {
        let mut stmt = c
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap();
        stmt.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn fresh_current_and_newer_databases() {
        let c = Connection::open_in_memory().unwrap();
        assert_eq!(records(|| meta(&c, &Markers::default())), (18, vec![]));
        c.execute_batch("CREATE TABLE host(host_id BLOB); PRAGMA user_version=18")
            .unwrap();
        assert_eq!(
            records(|| meta(&c, &Markers::default())),
            (
                18,
                vec!["metadata database version is 18 (no migration needed)".to_string()]
            )
        );
        c.execute_batch("PRAGMA user_version=19").unwrap();
        assert_eq!(
            records(|| meta(&c, &Markers::default())),
            (18, vec!["Database version is 19, current version is 18. Running migration for metadata ...".to_string()])
        );
    }

    #[test]
    fn a_version_1_database_is_brought_to_18() {
        let c = Connection::open_in_memory().unwrap();
        functions::register(&c);
        c.execute_batch(
            "CREATE TABLE host(host_id BLOB PRIMARY KEY, hostname TEXT NOT NULL);
             CREATE TABLE alert_hash(hash_id blob PRIMARY KEY);
             CREATE TABLE aclk_alert_x(alert_unique_id INT);
             CREATE INDEX aclk_alert_index_x ON aclk_alert_x(alert_unique_id);
             INSERT INTO aclk_alert_x VALUES (7);
             PRAGMA user_version=1",
        )
        .unwrap();
        let (version, log) = records(|| meta(&c, &Markers::default()));
        assert_eq!(version, 18);
        let mut want = vec![
            "Database version is 1, current version is 18. Running migration for metadata ..."
                .to_string(),
        ];
        want.extend(
            (1..18).map(|i| format!("Running database \"metadata\" migration v{i} to v{}", i + 1)),
        );
        assert_eq!(log, want);
        assert_eq!(
            columns(&c, "host"),
            [
                "host_id",
                "hostname",
                "hops",
                "memory_mode",
                "abbrev_timezone",
                "utc_offset",
                "program_name",
                "program_version",
                "entries",
                "health_enabled",
                "last_connected"
            ]
        );
        assert_eq!(
            columns(&c, "alert_hash"),
            [
                "hash_id",
                "source",
                "chart_labels",
                "summary",
                "time_group_condition",
                "time_group_value",
                "dims_group",
                "data_source"
            ]
        );
        let copied: i64 = c
            .query_row(
                "SELECT filtered_alert_unique_id FROM aclk_alert_x",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(copied, 7);
        let old_index: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name = 'aclk_alert_index_x'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(old_index, 0);
        assert_eq!(columns(&c, "health_log_detail").last().unwrap(), "summary");
    }

    #[test]
    fn version_8_health_logs_move_into_the_shared_tables() {
        let c = Connection::open_in_memory().unwrap();
        functions::register(&c);
        let table = "health_log_5a1e0000_0000_4000_8000_0000000000aa";
        c.execute_batch(&format!(
            "CREATE TABLE alert_hash(hash_id blob PRIMARY KEY);
             CREATE TABLE {table}(alarm_id int, config_hash_id blob, name text, chart text, family text, exec text,
               recipient text, units text, chart_context text, unique_id int, alarm_event_id int, updated_by_id int,
               updates_id int, when_key int, duration int, non_clear_duration int, flags int, exec_run_timestamp int,
               delay_up_to_timestamp int, info text, exec_code int, new_status real, old_status real, delay int,
               new_value double, old_value double, last_repeat int, transition_id blob);
             INSERT INTO {table} (alarm_id, name, unique_id, alarm_event_id) VALUES (3, 'cpu', 11, 1);
             PRAGMA user_version=8"
        ))
        .unwrap();
        let (version, log) = records(|| meta(&c, &Markers::default()));
        assert_eq!(version, 18, "{log:?}");
        let host: Vec<u8> = c
            .query_row(
                "SELECT host_id FROM health_log WHERE alarm_id = 3",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            host,
            [
                0x5a, 0x1e, 0, 0, 0, 0, 0x40, 0, 0x80, 0, 0, 0, 0, 0, 0, 0xaa
            ]
        );
        let (unique_id, transition, summary): (i64, Vec<u8>, String) = c
            .query_row(
                "SELECT unique_id, transition_id, summary FROM health_log_detail d JOIN health_log l USING (health_log_id)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            (unique_id, transition.len(), summary.as_str()),
            (11, 16, "cpu")
        );
        assert!(!table_exists(&c, table));
        assert!(!columns(&c, "health_log_detail").contains(&"host_id".to_string()));
    }

    #[test]
    fn a_failed_step_returns_its_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("m.db");
        let rw = Connection::open(&path).unwrap();
        rw.execute_batch("CREATE TABLE host(host_id BLOB); PRAGMA user_version=1")
            .unwrap();
        drop(rw);
        let ro =
            Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        let (version, log) = records(|| meta(&ro, &Markers::default()));
        assert_eq!(version, 1);
        assert_eq!(
            log,
            [
                "Database version is 1, current version is 18. Running migration for metadata ...",
                "Running database \"metadata\" migration v1 to v2",
                "SQLite error during database initialization, rc = 8 (attempt to write a readonly database)",
                "SQLite failed statement ALTER TABLE host ADD hops INTEGER NOT NULL DEFAULT 0",
                "Database metadata migration from version 1 to version 2 failed",
            ]
        );
    }

    #[test]
    fn the_context_database() {
        let c = Connection::open_in_memory().unwrap();
        assert_eq!(records(|| context(&c)), (1, vec![]));
        c.execute_batch("CREATE TABLE context(host_id BLOB)")
            .unwrap();
        assert_eq!(
            records(|| context(&c)),
            (
                1,
                vec![
                    "Database version is 0, current version is 1. Running migration for context ...".to_string(),
                    "Running database \"context\" migration v0 to v1".to_string(),
                ]
            )
        );
    }
}
