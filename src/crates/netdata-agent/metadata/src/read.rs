//! The start-up readers of `netdata-meta.db` and `context-meta.db`, with C's SQL (`sqlite_metadata.c`,
//! `sqlite_aclk.c`, `sqlite_context.c`), the machine-GUID change and the agent event log. Rows come out typed; UUID
//! columns count only as 16-byte blobs, as `sqlite3_column_uuid_ptr()` reads them.

use std::path::Path;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};

use crate::conn::{self, Markers};
use crate::open::MetaDb;

/// A UUID column: a blob of exactly 16 bytes.
fn uuid(row: &Row<'_>, i: usize) -> Option<[u8; 16]> {
    match row.get_ref(i) {
        Ok(ValueRef::Blob(b)) => b.try_into().ok(),
        _ => None,
    }
}

/// `sqlite3_column_text()`: any value as text, NULL as `None`.
fn text(row: &Row<'_>, i: usize) -> Option<String> {
    match row.get_ref(i).ok()? {
        ValueRef::Null => None,
        ValueRef::Integer(v) => Some(v.to_string()),
        ValueRef::Real(v) => Some(v.to_string()),
        ValueRef::Text(t) | ValueRef::Blob(t) => Some(String::from_utf8_lossy(t).into_owned()),
    }
}

/// `sqlite3_column_int64()`: text read as its leading number, a real truncated, NULL as 0.
fn int(row: &Row<'_>, i: usize) -> i64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v,
        Ok(ValueRef::Real(v)) => v as i64,
        Ok(ValueRef::Text(t)) => {
            let t = String::from_utf8_lossy(t);
            let t = t.trim_start();
            let end = t
                .char_indices()
                .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
                .map_or(t.len(), |(i, _)| i);
            t[..end].parse().unwrap_or(0)
        }
        _ => 0,
    }
}

/// `PREPARE_STATEMENT()`'s record, with the C function it names.
fn prepare_failed(err: &rusqlite::Error, function: &str) {
    nd_log!(
        Source::Daemon,
        Priority::Err,
        "Failed to prepare statement, rc={} in {function}",
        conn::result_code(err)
    );
}

/// Runs `sql` with `params` bound, calling `f` for each row. While the database is busy the query runs again, as
/// `sqlite3_step_monitored()` retries, but only before a row came out: rusqlite restarts a statement whose step
/// failed, where C would continue it.
fn for_each_row(
    c: &Connection,
    sql: &str,
    params: impl rusqlite::Params + Copy,
    function: &str,
    mut f: impl FnMut(&Row<'_>),
) {
    let mut stmt = match c.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            prepare_failed(&err, function);
            return;
        }
    };
    let mut delivered = false;
    let mut attempt = 1;
    loop {
        let mut run = || -> rusqlite::Result<()> {
            let mut rows = stmt.query(params)?;
            while let Some(row) = rows.next()? {
                delivered = true;
                f(row);
            }
            Ok(())
        };
        match run() {
            Err(err)
                if !delivered
                    && matches!(
                        conn::result_code(&err),
                        conn::SQLITE_BUSY | conn::SQLITE_LOCKED
                    )
                    && attempt < conn::MAX_RETRY =>
            {
                attempt += 1;
                std::thread::sleep(conn::RETRY_DELAY);
            }
            _ => return,
        }
    }
}

/// A row of `SQL_FETCH_ALL_HOSTS`: a host with `hops > 0`, the ones C loads as archived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRow {
    pub host_id: [u8; 16],
    pub hostname: Option<String>,
    pub registry_hostname: Option<String>,
    /// SQL NULL reads as 1; a stored 0 stays 0.
    pub update_every: i32,
    pub os: Option<String>,
    pub timezone: Option<String>,
    pub hops: i32,
    pub abbrev_timezone: Option<String>,
    pub utc_offset: i32,
    pub program_name: Option<String>,
    pub program_version: Option<String>,
    pub entries: i32,
    pub last_connected: i64,
    /// Its `_is_ephemeral` label is `true`.
    pub is_ephemeral: bool,
    /// It has a node id row.
    pub is_registered: bool,
}

/// `SQL_FETCH_ALL_HOSTS`.
const FETCH_ALL_HOSTS: &str = "SELECT h.host_id, h.hostname, h.registry_hostname, h.update_every, h.os, h.timezone, \
                               h.hops, h.abbrev_timezone, h.utc_offset, h.program_name, h.program_version, \
                               h.entries, h.last_connected, CASE WHEN hl.label_value = 'true' THEN 1 ELSE 0 END, \
                               CASE WHEN ni.node_id IS NULL THEN 0 ELSE 1 END FROM host h LEFT JOIN host_label hl \
                               ON hl.host_id = h.host_id AND hl.label_key = '_is_ephemeral' LEFT JOIN \
                               node_instance ni ON ni.host_id = h.host_id WHERE h.hops > 0";

/// What `node_instance` says about a host's node id.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeId {
    /// No row: the node id stays as it is.
    Absent,
    /// A row without a valid id: the node id is cleared.
    Cleared,
    Set([u8; 16]),
}

/// `event_log_type_t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    StartTime = 1,
    ShutdownTime = 2,
}

/// `SQL_GET_AGENT_EVENT_TYPE_MEDIAN`.
const AGENT_EVENT_MEDIAN: &str = "SELECT AVG(value) AS median FROM (SELECT value FROM agent_event_log WHERE \
                                  event_type = @event ORDER BY value  LIMIT 2 - (SELECT COUNT(*) FROM \
                                  agent_event_log WHERE event_type = @event) % 2 OFFSET(SELECT(COUNT(*) - 1) / 2 \
                                  FROM agent_event_log WHERE event_type = @event)) ";

impl MetaDb {
    /// The rows of `aclk_synchronization_init()`, in the table's order. A row whose `host_id` is not a UUID is
    /// skipped with C's record; a failed query ends the list with C's record (the load may be partial).
    pub fn archived_hosts(&self) -> Vec<HostRow> {
        let c = self.lock();
        let mut stmt = match c.prepare(FETCH_ALL_HOSTS) {
            Ok(stmt) => stmt,
            Err(err) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "SQLite error when preparing statement to load archived hosts: {}",
                    conn::message(&err)
                );
                return Vec::new();
            }
        };
        let mut hosts = Vec::new();
        let result = conn::retry(|| {
            hosts.clear();
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let Some(host_id) = uuid(row, 0) else {
                    let (kind, bytes) = match row.get_ref(0)? {
                        ValueRef::Integer(v) => (1, v.to_string().len()),
                        ValueRef::Real(v) => (2, v.to_string().len()),
                        ValueRef::Text(t) => (3, t.len()),
                        ValueRef::Blob(b) => (4, b.len()),
                        ValueRef::Null => (5, 0),
                    };
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "Skipping archived host: host_id column is not a valid 16-byte UUID blob (type={kind}, \
                         bytes={bytes}). Possible DB corruption."
                    );
                    continue;
                };
                hosts.push(HostRow {
                    host_id,
                    hostname: text(row, 1),
                    registry_hostname: text(row, 2),
                    update_every: if matches!(row.get_ref(3)?, ValueRef::Null) {
                        1
                    } else {
                        int(row, 3) as i32
                    },
                    os: text(row, 4),
                    timezone: text(row, 5),
                    hops: int(row, 6) as i32,
                    abbrev_timezone: text(row, 7),
                    utc_offset: int(row, 8) as i32,
                    program_name: text(row, 9),
                    program_version: text(row, 10),
                    entries: int(row, 11) as i32,
                    last_connected: int(row, 12),
                    is_ephemeral: int(row, 13) != 0,
                    is_registered: int(row, 14) != 0,
                });
            }
            Ok(())
        });
        if let Err(err) = result {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "SQLite error while loading archived hosts, rc = {} ({}); load may be partial",
                conn::result_code(&err),
                conn::message(&err)
            );
        }
        hosts
    }

    /// `sql_build_host_system_info()`: the host's system info keys and values.
    pub fn host_info(&self, host_id: &[u8; 16]) -> Vec<(String, String)> {
        let mut info = Vec::new();
        for_each_row(
            &self.lock(),
            "SELECT system_key, system_value FROM host_info WHERE host_id = @host_id AND system_key IS NOT NULL AND \
             system_value IS NOT NULL",
            [&host_id[..]],
            "sql_build_host_system_info",
            |row| {
                info.push((
                    text(row, 0).unwrap_or_default(),
                    text(row, 1).unwrap_or_default(),
                ))
            },
        );
        info
    }

    /// `sql_load_host_labels()`: name, value and source of each stored host label.
    pub fn host_labels(&self, host_id: &[u8; 16]) -> Vec<(String, String, u32)> {
        let mut labels = Vec::new();
        for_each_row(
            &self.lock(),
            "SELECT label_key, label_value, source_type FROM host_label WHERE host_id = @host_id AND label_key IS \
             NOT NULL AND label_value IS NOT NULL",
            [&host_id[..]],
            "sql_load_host_labels",
            |row| {
                labels.push((
                    text(row, 0).unwrap_or_default(),
                    text(row, 1).unwrap_or_default(),
                    int(row, 2) as u32,
                ));
            },
        );
        labels
    }

    /// `sql_load_node_id()`.
    pub fn node_id(&self, host_id: &[u8; 16]) -> NodeId {
        let mut node_id = NodeId::Absent;
        let c = self.lock();
        let mut stmt = match c.prepare("SELECT node_id FROM node_instance WHERE host_id = @host_id")
        {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "sql_load_node_id");
                return node_id;
            }
        };
        let _ = conn::retry(|| {
            let mut rows = stmt.query([&host_id[..]])?;
            if let Some(row) = rows.next()? {
                node_id = uuid(row, 0).map_or(NodeId::Cleared, NodeId::Set);
            }
            Ok(())
        });
        node_id
    }

    /// `detect_machine_guid_change()`: an older localhost (hops 0, another GUID) becomes a child, and node instances
    /// of hosts no longer in the table go.
    pub fn detect_machine_guid_change(&self, host_id: &[u8; 16]) {
        const CONVERT: &str = "UPDATE host SET hops = 1 WHERE hops = 0 AND host_id <> @host_id";
        let c = self.lock();
        let converted = match c.prepare(CONVERT) {
            Err(err) => {
                prepare_failed(&err, "exec_statement_with_uuid");
                netdata_log_error!("Failed to prepare statement {CONVERT}");
                false
            }
            Ok(mut stmt) => match conn::retry(|| stmt.execute([&host_id[..]])) {
                Ok(_) => true,
                Err(err) => {
                    netdata_log_error!(
                        "Failed to execute {CONVERT}, rc = {}",
                        conn::result_code(&err)
                    );
                    false
                }
            },
        };
        if converted
            && conn::db_execute(
                &c,
                "DELETE FROM node_instance WHERE host_id NOT IN (SELECT host_id FROM host)",
                &self.markers(),
            )
            .is_err()
        {
            netdata_log_error!("Failed to remove deleted hosts from node instances");
        }
    }

    /// `add_agent_event()`: an event of this start, with the agent's version.
    pub fn add_agent_event(&self, kind: EventKind, version: &str, value: i64) {
        let c = self.lock();
        let mut stmt = match c.prepare(
            "INSERT INTO agent_event_log (event_type, version, value, date_created) VALUES  (@event_type, \
             @version, @value, UNIXEPOCH())",
        ) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "add_agent_event");
                return;
            }
        };
        if let Err(err) =
            conn::retry(|| stmt.execute(rusqlite::params![kind as i32, version, value]))
        {
            let rc = conn::result_code(&err);
            if rc == conn::SQLITE_CORRUPT {
                self.markers().mark(rc);
                netdata_log_error!("SQLite error {rc}");
            }
            netdata_log_error!("Failed to store agent event information, rc = {rc}");
        }
    }

    /// `get_agent_event_time_median()` without its cache (the caller keeps it): the median value of an event, 0
    /// without events.
    pub fn agent_event_median(&self, kind: EventKind) -> u64 {
        let mut median = 0;
        for_each_row(
            &self.lock(),
            AGENT_EVENT_MEDIAN,
            [kind as i32],
            "get_agent_event_time_median",
            |row| {
                median = int(row, 0) as u64;
            },
        );
        median
    }

    /// `cleanup_agent_event_log()`: events older than 30 days.
    pub fn cleanup_agent_event_log(&self) {
        let _ = conn::db_execute(
            &self.lock(),
            "DELETE FROM agent_event_log WHERE date_created < UNIXEPOCH() - 30 * 86400",
            &self.markers(),
        );
    }

    /// `SQL_HOSTNAME_TO_REMOVE` of `cmd_remove_stale_node_internal()`: the stored hosts with this hostname, or all
    /// of them for `ALL_NODES`, in the table's order, as machine GUIDs; `None` when the statement cannot be prepared.
    /// A row whose `host_id` is not a UUID is skipped.
    pub fn hosts_named(&self, hostname: &str) -> Option<Vec<String>> {
        const HOSTNAME_TO_REMOVE: &str =
            "SELECT host_id FROM host WHERE (hostname = @hostname OR @hostname = 'ALL_NODES')";
        let c = self.lock();
        let mut stmt = match c.prepare(HOSTNAME_TO_REMOVE) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "cmd_remove_stale_node_internal");
                return None;
            }
        };
        let mut guids = Vec::new();
        if let Ok(mut rows) = stmt.query([hostname]) {
            while let Ok(Some(row)) = rows.next() {
                if let Some(id) = uuid(row, 0) {
                    guids.push(guid(&id));
                }
            }
        }
        Some(guids)
    }

    /// The UUID of every stored dimension, as `populate_metrics_from_database()` reads them (on a read-only handle
    /// of its own, falling back to the shared one); the count of valid ones.
    pub fn dimension_uuids(&self, mut f: impl FnMut(&[u8; 16])) -> usize {
        let path = MetaDb::path(self.cache_dir());
        let own = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .ok();
        if let Some(c) = &own {
            let _ = c.busy_timeout(std::time::Duration::ZERO);
            let _ = conn::db_execute(c, "PRAGMA cache_size=10000", &Markers::default());
        }
        let shared;
        let c: &Connection = match &own {
            Some(c) => c,
            None => {
                shared = self.lock();
                &shared
            }
        };
        let mut count = 0;
        for_each_row(
            c,
            "SELECT dim_id FROM dimension",
            [],
            "populate_metrics_from_database",
            |row| {
                if let Some(id) = uuid(row, 0) {
                    f(&id);
                    count += 1;
                }
            },
        );
        count
    }
}

/// A row of `CTX_GET_CHART_LIST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRow {
    pub chart_id: [u8; 16],
    /// `type.id`.
    pub id: Option<String>,
    pub name: Option<String>,
    pub context: Option<String>,
    pub title: Option<String>,
    pub units: Option<String>,
    pub priority: i32,
    pub update_every: i32,
    pub chart_type: i32,
    pub family: Option<String>,
}

/// A row of `CTX_GET_DIMENSION_LIST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimRow {
    pub dim_id: [u8; 16],
    pub id: Option<String>,
    pub name: Option<String>,
    pub hidden: bool,
    /// The chart's `type.id`.
    pub chart_id: Option<String>,
    pub context: Option<String>,
    pub algorithm: i32,
}

/// A row of `CTX_GET_CONTEXT_LIST`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRow {
    pub id: Option<String>,
    pub version: i64,
    pub title: Option<String>,
    pub chart_type: Option<String>,
    pub units: Option<String>,
    pub priority: i64,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub deleted: bool,
    pub family: Option<String>,
}

pub(crate) fn guid(host: &[u8; 16]) -> String {
    uuid::Uuid::from_bytes(*host).hyphenated().to_string()
}

/// `ctx_get_chart_list()`: a host's charts, on `netdata-meta.db` (a context-load thread's own handle or the shared
/// one). A row without a valid chart id is skipped with C's record.
pub fn chart_list(c: &Connection, host: &[u8; 16], mut f: impl FnMut(ChartRow)) {
    for_each_row(
        c,
        "SELECT c.chart_id, c.type||'.'||c.id, c.name, c.context, c.title, c.unit, c.priority, c.update_every, \
         c.chart_type, c.family FROM chart c WHERE c.host_id = @host_id AND c.chart_id IS NOT NULL",
        [&host[..]],
        "ctx_get_chart_list",
        |row| {
            let Some(chart_id) = uuid(row, 0) else {
                netdata_log_error!(
                    "CTX [{}]: Got invalid chart id in column 0. Ignoring it.",
                    guid(host)
                );
                return;
            };
            f(ChartRow {
                chart_id,
                id: text(row, 1),
                name: text(row, 2),
                context: text(row, 3),
                title: text(row, 4),
                units: text(row, 5),
                priority: int(row, 6) as i32,
                update_every: int(row, 7) as i32,
                chart_type: int(row, 8) as i32,
                family: text(row, 9),
            });
        },
    );
}

/// `ctx_get_dimension_list()`: a host's dimensions in the order they were stored. A row without a valid dimension
/// id is skipped with C's record.
pub fn dimension_list(c: &Connection, host: &[u8; 16], mut f: impl FnMut(DimRow)) {
    for_each_row(
        c,
        "SELECT d.dim_id, d.id, d.name, CASE WHEN INSTR(d.options,\"hidden\") > 0 THEN 1 ELSE 0 END, \
         c.type||'.'||c.id, c.context, d.algorithm FROM dimension d, chart c WHERE c.host_id = @host_id AND \
         d.chart_id = c.chart_id AND d.dim_id IS NOT NULL ORDER BY d.rowid ASC",
        [&host[..]],
        "ctx_get_dimension_list",
        |row| {
            let Some(dim_id) = uuid(row, 0) else {
                netdata_log_error!(
                    "CTX [{}]: Got invalid dimension id in column 0. Ignoring it.",
                    guid(host)
                );
                return;
            };
            f(DimRow {
                dim_id,
                id: text(row, 1),
                name: text(row, 2),
                hidden: int(row, 3) != 0,
                chart_id: text(row, 4),
                context: text(row, 5),
                algorithm: int(row, 6) as i32,
            });
        },
    );
}

/// `ctx_get_context_list()`: a host's contexts, on `context-meta.db`.
pub fn context_list(c: &Connection, host: &[u8; 16], mut f: impl FnMut(ContextRow)) {
    for_each_row(
        c,
        "SELECT id, version, title, chart_type, unit, priority, first_time_t, last_time_t, deleted, family FROM \
         context c WHERE c.host_id = @host_id",
        [&host[..]],
        "ctx_get_context_list",
        |row| {
            f(ContextRow {
                id: text(row, 0),
                version: int(row, 1),
                title: text(row, 2),
                chart_type: text(row, 3),
                units: text(row, 4),
                priority: int(row, 5),
                first_time_s: int(row, 6),
                last_time_s: int(row, 7),
                deleted: int(row, 8) != 0,
                family: text(row, 9),
            });
        },
    );
}

/// `ctx_get_label_list()`: a chart's labels, through `context-meta.db`'s attached `meta`.
pub fn chart_labels(
    c: &Connection,
    chart: &[u8; 16],
) -> Vec<(Option<String>, Option<String>, u32)> {
    let mut labels = Vec::new();
    for_each_row(
        c,
        "SELECT l.label_key, l.label_value, l.source_type FROM meta.chart_label l WHERE l.chart_id = @id",
        [&chart[..]],
        "ctx_get_label_list",
        |row| labels.push((text(row, 0), text(row, 1), int(row, 2) as u32)),
    );
    labels
}

/// A read-only handle to one of the databases, as a context-load thread opens them; `None` when it cannot be opened
/// (the caller falls back to the shared handle).
pub fn read_only(path: &Path) -> Option<Connection> {
    let c = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    c.busy_timeout(std::time::Duration::ZERO).ok()?;
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open::SqliteSettings;

    fn db() -> (tempfile::TempDir, MetaDb) {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        (dir, meta)
    }

    const A: [u8; 16] = [0xaa; 16];
    const B: [u8; 16] = [0xbb; 16];
    const C: [u8; 16] = [0xcc; 16];

    #[test]
    fn archived_hosts_read_as_c_reads_them() {
        let (_dir, meta) = db();
        {
            let c = meta.lock();
            // rows from older schemas can hold NULLs the current one forbids
            c.execute_batch(
                "DROP TABLE host; CREATE TABLE host(host_id BLOB PRIMARY KEY, hostname TEXT, registry_hostname TEXT,
                 update_every INT, os TEXT, timezone TEXT, tags TEXT, hops INT, memory_mode INT, abbrev_timezone TEXT,
                 utc_offset INT, program_name TEXT, program_version TEXT, entries INT, health_enabled INT,
                 last_connected INT)",
            )
            .unwrap();
            let insert = "INSERT INTO host (host_id, hostname, registry_hostname, update_every, os, timezone, hops, \
                          abbrev_timezone, utc_offset, program_name, program_version, entries, last_connected) \
                          VALUES (?1, ?2, 'reg', ?3, 'linux', 'UTC', ?4, 'UTC', 0, ?5, 'v1', 3600, 17)";
            c.execute(insert, rusqlite::params![&A[..], "self", 1, 0, "netdata"])
                .unwrap();
            c.execute(
                insert,
                rusqlite::params![
                    &B[..],
                    "child",
                    Option::<i32>::None,
                    1,
                    Option::<&str>::None
                ],
            )
            .unwrap();
            c.execute(insert, rusqlite::params![&C[..], "zero", 0, 2, "netdata"])
                .unwrap();
            c.execute(
                insert,
                rusqlite::params!["not-a-uuid", "bad", 1, 1, "netdata"],
            )
            .unwrap();
            c.execute_batch(&format!(
                "INSERT INTO host_label VALUES (x'{}', 1, '_is_ephemeral', 'true', 0);
                 INSERT INTO node_instance (host_id) VALUES (x'{}');
                 INSERT INTO node_instance (host_id, node_id) VALUES (x'{}', x'{}');",
                "bb".repeat(16),
                "bb".repeat(16),
                "cc".repeat(16),
                "11".repeat(16)
            ))
            .unwrap();
        }
        let (hosts, records) = netdata_agent_log::capture(|| meta.archived_hosts());
        let summary: Vec<_> = hosts
            .iter()
            .map(|h| {
                (
                    h.hostname.clone().unwrap(),
                    h.update_every,
                    h.hops,
                    h.program_name.clone(),
                    h.is_ephemeral,
                    h.is_registered,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                ("child".to_string(), 1, 1, None, true, false),
                (
                    "zero".to_string(),
                    0,
                    2,
                    Some("netdata".to_string()),
                    false,
                    true
                ),
            ]
        );
        let messages: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
        assert_eq!(
            messages,
            [
                "Skipping archived host: host_id column is not a valid 16-byte UUID blob (type=3, bytes=10). Possible \
              DB corruption."
            ]
        );
        assert_eq!(meta.node_id(&C), NodeId::Set([0x11; 16]));
        assert_eq!(meta.node_id(&B), NodeId::Cleared);
        assert_eq!(meta.node_id(&[0xdd; 16]), NodeId::Absent);
    }

    #[test]
    fn host_info_and_labels() {
        let (_dir, meta) = db();
        meta.lock()
            .execute_batch(&format!(
                "INSERT INTO host_info VALUES (x'{a}', 'NETDATA_SYSTEM_OS_NAME', 'Debian', 0);
                 INSERT INTO host_info VALUES (x'{b}', 'NETDATA_SYSTEM_OS_NAME', 'other', 0);
                 INSERT INTO host_label VALUES (x'{a}', 2, 'k', 'v', 0);",
                a = "aa".repeat(16),
                b = "bb".repeat(16)
            ))
            .unwrap();
        assert_eq!(
            meta.host_info(&A),
            [("NETDATA_SYSTEM_OS_NAME".to_string(), "Debian".to_string())]
        );
        assert_eq!(
            meta.host_labels(&A),
            [("k".to_string(), "v".to_string(), 2)]
        );
    }

    #[test]
    fn a_changed_machine_guid_makes_the_old_localhost_a_child() {
        let (_dir, meta) = db();
        meta.lock()
            .execute_batch(&format!(
                "INSERT INTO host (host_id, hostname, hops) VALUES (x'{}', 'old', 0);
                 INSERT INTO host (host_id, hostname, hops) VALUES (x'{}', 'new', 0);
                 INSERT INTO node_instance (host_id) VALUES (x'{}');",
                "aa".repeat(16),
                "bb".repeat(16),
                "cc".repeat(16)
            ))
            .unwrap();
        meta.detect_machine_guid_change(&B);
        let c = meta.lock();
        let hops: Vec<(String, i64)> = c
            .prepare("SELECT hostname, hops FROM host ORDER BY hostname")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(hops, [("new".to_string(), 0), ("old".to_string(), 1)]);
        let orphans: i64 = c
            .query_row(
                &format!(
                    "SELECT count(*) FROM node_instance WHERE host_id = x'{}'",
                    "cc".repeat(16)
                ),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn agent_event_medians_and_cleanup() {
        let (_dir, meta) = db();
        let cases = [
            (vec![], 0),
            (vec![5], 5),
            (vec![9, 2], 5),
            (vec![7, 1, 4], 4),
            (vec![10, 1, 4, 7], 5),
        ];
        for (values, median) in cases {
            meta.lock()
                .execute_batch("DELETE FROM agent_event_log")
                .unwrap();
            for v in &values {
                meta.add_agent_event(EventKind::StartTime, "v0", *v);
            }
            meta.add_agent_event(EventKind::ShutdownTime, "v0", 1000);
            assert_eq!(
                meta.agent_event_median(EventKind::StartTime),
                median,
                "{values:?}"
            );
        }
        meta.lock()
            .execute_batch("UPDATE agent_event_log SET date_created = UNIXEPOCH() - 31 * 86400 WHERE value = 1000")
            .unwrap();
        meta.cleanup_agent_event_log();
        assert_eq!(meta.agent_event_median(EventKind::ShutdownTime), 0);
        assert_eq!(meta.agent_event_median(EventKind::StartTime), 5);
    }

    #[test]
    fn chart_and_dimension_lists() {
        let (_dir, meta) = db();
        let c = meta.lock();
        c.execute_batch(&format!(
            "INSERT INTO chart (chart_id, host_id, type, id, name, context, update_every, chart_type, priority)
               VALUES (x'{c1}', x'{h}', 'system', 'cpu', NULL, 'system.cpu', 2, 1, 100);
             INSERT INTO chart (chart_id, host_id, type, id) VALUES ('bad', x'{h}', 'x', 'y');
             INSERT INTO dimension (dim_id, chart_id, id, name, options, algorithm) VALUES (x'{d1}', x'{c1}', 'user', 'user', 'hidden', 1);
             INSERT INTO dimension (dim_id, chart_id, id, name) VALUES (x'{d2}', x'{c1}', 'system', 'system');",
            c1 = "01".repeat(16),
            h = "aa".repeat(16),
            d1 = "02".repeat(16),
            d2 = "03".repeat(16)
        ))
        .unwrap();
        let mut charts = Vec::new();
        let (_, records) = netdata_agent_log::capture(|| chart_list(&c, &A, |r| charts.push(r)));
        assert_eq!(charts.len(), 1);
        assert_eq!(
            (
                charts[0].id.as_deref(),
                charts[0].name.as_deref(),
                charts[0].update_every
            ),
            (Some("system.cpu"), None, 2)
        );
        assert_eq!(
            records
                .into_iter()
                .filter_map(|r| r.message)
                .collect::<Vec<_>>(),
            [
                "CTX [aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa]: Got invalid chart id in column 0. Ignoring it."
            ]
        );
        let mut dims = Vec::new();
        dimension_list(&c, &A, |r| {
            dims.push((r.id.unwrap(), r.hidden, r.chart_id.unwrap(), r.algorithm))
        });
        assert_eq!(
            dims,
            [
                ("user".to_string(), true, "system.cpu".to_string(), 1),
                ("system".to_string(), false, "system.cpu".to_string(), 0)
            ]
        );
    }

    /// The netdatacli lookup: a hostname's stored hosts, or all of them, in the table's order; a row that is not a
    /// UUID is skipped.
    #[test]
    fn hosts_named_as_c() {
        let (_dir, meta) = db();
        meta.lock()
            .execute_batch(
                "INSERT INTO host (host_id, hostname) VALUES (x'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 'b'), \
                 (x'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 'a'), ('not-a-uuid', 'a')",
            )
            .unwrap();
        assert_eq!(
            meta.hosts_named("a"),
            Some(vec!["aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".to_string()])
        );
        assert_eq!(meta.hosts_named("ALL_NODES").map(|g| g.len()), Some(2));
        assert_eq!(meta.hosts_named("none"), Some(vec![]));
    }
}
