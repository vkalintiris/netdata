//! The metadata writer's statements (`sqlite_metadata.c`, `sqlite_aclk.c`): C's SQL verbatim, bound as C binds it,
//! with C's records. Each function is one C function; the daemon walks hosts, charts and dimensions, maps them to the
//! records here, and keeps the flags. A statement that cannot be prepared is reported with the C function's name.

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, Statement, ToSql};
use std::cell::RefCell;

use crate::conn;
use crate::open::MetaDb;

const SQL_STORE_HOST_INFO: &str = "INSERT OR REPLACE INTO host (host_id, hostname, registry_hostname, update_every, \
     os, timezone, tags, hops, memory_mode, abbrev_timezone, utc_offset, program_name, program_version, entries, \
     health_enabled, last_connected) VALUES (@host_id, @hostname, @registry_hostname, @update_every, @os, @timezone, \
     @tags, @hops, @memory_mode, @abbrev_tz, @utc_offset, @prog_name, @prog_version, @entries, @health_enabled, \
     @last_connected)";

const SQL_STORE_HOST_SYSTEM_INFO_VALUES: &str = "INSERT OR REPLACE INTO host_info (host_id, system_key, \
     system_value, date_created) VALUES (@uuid, @name, @value, UNIXEPOCH())";

const SQL_DELETE_HOST_LABELS: &str = "DELETE FROM host_label WHERE host_id = @uuid";

const SQL_STORE_HOST_LABEL: &str =
    "INSERT INTO host_label (host_id, source_type, label_key, label_value, date_created) VALUES ";
const SQL_STORE_HOST_LABEL_CONFLICT: &str = " ON CONFLICT (host_id, label_key) DO UPDATE SET source_type = \
     excluded.source_type, label_value = excluded.label_value, date_created = UNIXEPOCH()";
const SQL_STORE_CHART_LABEL: &str =
    "INSERT INTO chart_label (chart_id, source_type, label_key, label_value, date_created) VALUES ";
const SQL_STORE_CHART_LABEL_CONFLICT: &str = " ON CONFLICT (chart_id, label_key) DO UPDATE SET source_type = \
     excluded.source_type, label_value = excluded.label_value, date_created = UNIXEPOCH()";

const SQL_STORE_CLAIM_ID: &str = "INSERT INTO node_instance (host_id, claim_id, date_created) VALUES (@host_id, \
     @claim_id, UNIXEPOCH()) ON CONFLICT(host_id) DO UPDATE SET claim_id = excluded.claim_id";

const SQL_STORE_CHART: &str = "INSERT INTO chart (chart_id, host_id, type, id, name, family, context, title, unit, \
     plugin, module, priority, update_every, chart_type, memory_mode, history_entries) values (@chart_id, @host_id, \
     @type, @id, @name, @family, @context, @title, @unit, @plugin, @module, @priority, @update_every, @chart_type, \
     @memory_mode, @history_entries) ON CONFLICT(chart_id) DO UPDATE SET type=excluded.type, id=excluded.id, \
     name=excluded.name, family=excluded.family, context=excluded.context, title=excluded.title, \
     unit=excluded.unit, plugin=excluded.plugin, module=excluded.module, priority=excluded.priority, \
     update_every=excluded.update_every, chart_type=excluded.chart_type, memory_mode = excluded.memory_mode, \
     history_entries = excluded.history_entries";

const SQL_STORE_DIMENSION: &str = "INSERT INTO dimension (dim_id, chart_id, id, name, multiplier, divisor , \
     algorithm, options) VALUES (@dim_id, @chart_id, @id, @name, @multiplier, @divisor, @algorithm, @options) ON \
     CONFLICT(dim_id) DO UPDATE SET id=excluded.id, name=excluded.name, multiplier=excluded.multiplier, \
     divisor=excluded.divisor, algorithm=excluded.algorithm, options=excluded.options";

const DELETE_DIMENSION_UUID: &str = "DELETE FROM dimension WHERE dim_id = @uuid";

const SCHEDULE_HOST_CTX_CLEANUP: &str = "INSERT INTO ctx_metadata_cleanup (host_id, context, date_created) \
                                         VALUES (@host_id, @context, UNIXEPOCH()) ON CONFLICT DO UPDATE SET \
                                         date_created = excluded.date_created";

// The double space before DO is C's.
const SQL_SET_HOST_LABEL: &str = "INSERT INTO host_label (host_id, source_type, label_key, label_value, \
     date_created) VALUES (@host_id, @source_type, @label_key, @label_value, UNIXEPOCH()) ON CONFLICT (host_id, \
     label_key)  DO UPDATE SET source_type = excluded.source_type, label_value=excluded.label_value, \
     date_created=UNIXEPOCH()";

const SQL_INVALIDATE_NODE_INSTANCES: &str = "UPDATE node_instance SET node_id = NULL WHERE EXISTS (SELECT host_id \
     FROM node_instance WHERE host_id = @host_id AND (@claim_id IS NULL OR claim_id <> @claim_id))";

const SQL_UNREGISTER_NODE: &str =
    "UPDATE node_instance SET node_id = NULL WHERE host_id = @host_id";

const SQL_INVALIDATE_HOST_LAST_CONNECTED: &str =
    "UPDATE host SET last_connected = 1 WHERE host_id = @host_id";

/// `LABEL_BATCH_SIZE`: labels per multi-row insert.
const LABEL_BATCH_SIZE: usize = 1024;

/// `RRDLABEL_FLAG_INTERNAL`: the marks a stored source leaves out.
const LABEL_FLAG_INTERNAL: u32 = (1 << 29) | (1 << 30) | (1 << 31);

/// `RRDLABEL_SRC_AUTO`.
const LABEL_SRC_AUTO: i32 = 1;

/// `DATABASE_VACUUM_FREQUENCY_SECONDS`, `DATABASE_FREE_PAGES_THRESHOLD_PC`, `DATABASE_FREE_PAGES_VACUUM_PC`.
const VACUUM_FREQUENCY_S: i64 = 60;
const FREE_PAGES_THRESHOLD_PC: i32 = 5;
const FREE_PAGES_VACUUM_PC: i32 = 10;

/// A host row (`store_host_metadata()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRecord<'a> {
    pub host_id: [u8; 16],
    pub hostname: &'a str,
    pub registry_hostname: &'a str,
    pub update_every: i32,
    pub os: &'a str,
    pub timezone: &'a str,
    /// `rrdhost_ingestion_hops()`.
    pub hops: i32,
    /// `RRD_DB_MODE`.
    pub memory_mode: i32,
    pub abbrev_timezone: &'a str,
    pub utc_offset: i32,
    pub program_name: &'a str,
    pub program_version: &'a str,
    pub entries: i64,
    pub health_enabled: bool,
    pub last_connected: i64,
}

/// A label as `rrdlabels_walkthrough_read()` hands it over: name, value and its `RRDLABEL_SRC` with marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LabelRecord<'a> {
    pub name: &'a [u8],
    pub value: &'a [u8],
    pub source: u32,
}

/// Where labels go (`label_store_type_t`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelTable {
    Host,
    Chart,
}

/// A chart row (`store_chart_metadata()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartRecord<'a> {
    pub chart_id: [u8; 16],
    pub host_id: [u8; 16],
    /// `st->parts.type`, `st->parts.id`, `st->parts.name` (`None` or empty: NULL).
    pub type_: &'a str,
    pub id: &'a str,
    pub name: Option<&'a str>,
    pub family: &'a str,
    pub context: &'a str,
    pub title: &'a str,
    pub units: &'a str,
    pub plugin: &'a str,
    pub module: &'a str,
    pub priority: i32,
    pub update_every: i32,
    /// `RRDSET_TYPE`.
    pub chart_type: i32,
    /// `RRD_DB_MODE`.
    pub memory_mode: i32,
    pub history_entries: i32,
}

/// A dimension row (`store_dimension_metadata()`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimRecord<'a> {
    pub dim_id: [u8; 16],
    pub chart_id: [u8; 16],
    pub id: &'a str,
    pub name: &'a str,
    pub multiplier: i32,
    pub divisor: i32,
    /// `RRD_ALGORITHM`.
    pub algorithm: i32,
    /// `RRDDIM_OPTION_HIDDEN`: stored as `'hidden'`, else NULL.
    pub hidden: bool,
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

/// Bytes bound as text, as `sqlite3_bind_text()` binds a C string.
fn text(bytes: &[u8]) -> ToSqlOutput<'_> {
    ToSqlOutput::Borrowed(ValueRef::Text(bytes))
}

/// A UUID bound as a 16-byte blob, or NULL.
fn uuid_or_null(id: Option<&[u8; 16]>) -> ToSqlOutput<'_> {
    match id {
        Some(id) => ToSqlOutput::Borrowed(ValueRef::Blob(id)),
        None => ToSqlOutput::Owned(Value::Null),
    }
}

/// Prepares `sql` and runs it with `params`, retried while busy: the step's error code, or the prepare failure
/// already reported.
fn execute(c: &Connection, sql: &str, function: &str, params: &[&dyn ToSql]) -> Result<(), Step> {
    let mut stmt = c.prepare(sql).map_err(|err| {
        prepare_failed(&err, function);
        Step::Prepare
    })?;
    conn::retry(|| stmt.execute(params))
        .map(drop)
        .map_err(|err| Step::Failed(conn::result_code(&err)))
}

/// Why a statement did not run.
enum Step {
    /// Its prepare failed (reported).
    Prepare,
    /// Its step failed with this code.
    Failed(i32),
}

/// `store_label_batch()`: one multi-row upsert; true when it failed.
fn store_label_batch(
    c: &Connection,
    id: &[u8; 16],
    labels: &[LabelRecord<'_>],
    table: LabelTable,
) -> bool {
    let (head, conflict) = match table {
        LabelTable::Host => (SQL_STORE_HOST_LABEL, SQL_STORE_HOST_LABEL_CONFLICT),
        LabelTable::Chart => (SQL_STORE_CHART_LABEL, SQL_STORE_CHART_LABEL_CONFLICT),
    };
    let rows = vec!["(?, ?, ?, ?, UNIXEPOCH())"; labels.len()].join(", ");
    let sql = format!("{head}{rows}{conflict}");
    let mut params: Vec<ToSqlOutput<'_>> = Vec::with_capacity(labels.len() * 4);
    for label in labels {
        params.push(uuid_or_null(Some(id)));
        params.push(ToSqlOutput::Owned(Value::Integer(i64::from(
            (label.source & !LABEL_FLAG_INTERNAL) as i32,
        ))));
        params.push(text(label.name));
        params.push(text(label.value));
    }
    let mut stmt = match c.prepare(&sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            prepare_failed(&err, "store_label_batch");
            return true;
        }
    };
    match conn::retry(|| stmt.execute(rusqlite::params_from_iter(params.iter()))) {
        Ok(_) => false,
        Err(err) => {
            netdata_log_error!(
                "Failed to store label batch, rc = {}",
                conn::result_code(&err)
            );
            true
        }
    }
}

/// `store_labels()`: the labels in batches of `LABEL_BATCH_SIZE`; true when all were stored.
fn store_labels(
    c: &Connection,
    id: &[u8; 16],
    labels: &[LabelRecord<'_>],
    table: LabelTable,
) -> bool {
    let mut errors = 0;
    for batch in labels.chunks(LABEL_BATCH_SIZE) {
        errors += usize::from(store_label_batch(c, id, batch, table));
    }
    errors == 0
}

/// A statement prepared at its first use in a scan and kept for the rest of it, as C keeps `store_chart` and
/// `store_dimension`; a failed prepare is reported and tried again at the next use.
fn scan_statement<'c, 's>(
    conn: &'c Connection,
    slot: &'s RefCell<Option<Statement<'c>>>,
    sql: &str,
    function: &str,
) -> Option<std::cell::RefMut<'s, Statement<'c>>> {
    let mut slot = slot.borrow_mut();
    if slot.is_none() {
        match conn.prepare(sql) {
            Ok(stmt) => *slot = Some(stmt),
            Err(err) => {
                prepare_failed(&err, function);
                return None;
            }
        }
    }
    std::cell::RefMut::filter_map(slot, Option::as_mut).ok()
}

/// `get_pragma_value()`: the pragma's integer, -1 when it cannot be read.
fn pragma_value(c: &Connection, sql: &str) -> i64 {
    let mut stmt = match c.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            prepare_failed(&err, "get_pragma_value");
            return -1;
        }
    };
    conn::retry(|| stmt.query_row([], |r| r.get::<_, i64>(0))).unwrap_or(-1)
}

/// The chart and dimension stores of one host (`metadata_scan_host()`), inside its transaction.
pub struct HostScan<'c> {
    conn: &'c Connection,
    chart: RefCell<Option<Statement<'c>>>,
    dimension: RefCell<Option<Statement<'c>>>,
}

impl HostScan<'_> {
    /// `check_and_update_chart_labels()`'s store: true when the labels were stored.
    pub fn chart_labels(&self, chart_id: &[u8; 16], labels: &[LabelRecord<'_>]) -> bool {
        store_labels(self.conn, chart_id, labels, LabelTable::Chart)
    }

    /// `store_chart_metadata()`: true when stored.
    pub fn chart(&self, chart: &ChartRecord<'_>) -> bool {
        let Some(mut stmt) = scan_statement(
            self.conn,
            &self.chart,
            SQL_STORE_CHART,
            "store_chart_metadata",
        ) else {
            return false;
        };
        let name = chart.name.filter(|n| !n.is_empty());
        let params: [&dyn ToSql; 16] = [
            &&chart.chart_id[..],
            &&chart.host_id[..],
            &chart.type_,
            &chart.id,
            &name,
            &chart.family,
            &chart.context,
            &chart.title,
            &chart.units,
            &chart.plugin,
            &chart.module,
            &chart.priority,
            &chart.update_every,
            &chart.chart_type,
            &chart.memory_mode,
            &chart.history_entries,
        ];
        match conn::retry(|| stmt.execute(&params[..])) {
            Ok(_) => true,
            Err(err) => {
                netdata_log_error!("Failed to store chart, rc = {}", conn::result_code(&err));
                false
            }
        }
    }

    /// `store_dimension_metadata()`: true when stored.
    pub fn dimension(&self, dim: &DimRecord<'_>) -> bool {
        let Some(mut stmt) = scan_statement(
            self.conn,
            &self.dimension,
            SQL_STORE_DIMENSION,
            "store_dimension_metadata",
        ) else {
            return false;
        };
        let options = dim.hidden.then_some("hidden");
        let params: [&dyn ToSql; 8] = [
            &&dim.dim_id[..],
            &&dim.chart_id[..],
            &dim.id,
            &dim.name,
            &dim.multiplier,
            &dim.divisor,
            &dim.algorithm,
            &options,
        ];
        match conn::retry(|| stmt.execute(&params[..])) {
            Ok(_) => true,
            Err(err) => {
                netdata_log_error!(
                    "Failed to store dimension, rc = {}",
                    conn::result_code(&err)
                );
                false
            }
        }
    }
}

impl MetaDb {
    /// `store_host_metadata()`: the host row; true when stored.
    pub fn store_host(&self, host: &HostRecord<'_>) -> bool {
        let c = self.lock();
        let params: [&dyn ToSql; 16] = [
            &&host.host_id[..],
            &host.hostname,
            &host.registry_hostname,
            &host.update_every,
            &host.os,
            &host.timezone,
            &"",
            &host.hops,
            &host.memory_mode,
            &host.abbrev_timezone,
            &host.utc_offset,
            &host.program_name,
            &host.program_version,
            &host.entries,
            &i32::from(host.health_enabled),
            &host.last_connected,
        ];
        match execute(&c, SQL_STORE_HOST_INFO, "store_host_metadata", &params) {
            Ok(()) => true,
            Err(Step::Prepare) => false,
            Err(Step::Failed(rc)) => {
                netdata_log_error!("Failed to store host {}, rc = {rc}", host.hostname);
                false
            }
        }
    }

    /// `store_host_systeminfo()`: one row per key, each on a statement of its own (`add_host_sysinfo_key_value()`),
    /// `None` stored as `unknown`; true when every key was stored.
    pub fn store_host_system_info(
        &self,
        host_id: &[u8; 16],
        keys: &[(&str, Option<&str>)],
    ) -> bool {
        let c = self.lock();
        let mut stored = 0;
        for &(name, value) in keys {
            let value = value.unwrap_or("unknown");
            let params: [&dyn ToSql; 3] = [&&host_id[..], &name, &value];
            match execute(
                &c,
                SQL_STORE_HOST_SYSTEM_INFO_VALUES,
                "add_host_sysinfo_key_value",
                &params,
            ) {
                Ok(()) => stored += 1,
                Err(Step::Prepare) => {}
                Err(Step::Failed(rc)) => {
                    netdata_log_error!("Failed to store host info value {name}, rc = {rc}");
                }
            }
        }
        stored == keys.len()
    }

    /// `exec_statement_with_uuid(SQL_DELETE_HOST_LABELS)`: true when deleted.
    pub fn delete_host_labels(&self, host_id: &[u8; 16]) -> bool {
        let c = self.lock();
        match execute(
            &c,
            SQL_DELETE_HOST_LABELS,
            "exec_statement_with_uuid",
            &[&&host_id[..]],
        ) {
            Ok(()) => true,
            Err(Step::Prepare) => {
                netdata_log_error!("Failed to prepare statement {SQL_DELETE_HOST_LABELS}");
                false
            }
            Err(Step::Failed(rc)) => {
                netdata_log_error!("Failed to execute {SQL_DELETE_HOST_LABELS}, rc = {rc}");
                false
            }
        }
    }

    /// `store_labels()` for a host: true when all were stored.
    pub fn store_host_labels(&self, host_id: &[u8; 16], labels: &[LabelRecord<'_>]) -> bool {
        store_labels(&self.lock(), host_id, labels, LabelTable::Host)
    }

    /// `store_claim_id()`: the host's node instance with this claim id (NULL when unclaimed); true when stored.
    pub fn store_claim_id(&self, host_id: &[u8; 16], claim_id: Option<&[u8; 16]>) -> bool {
        let c = self.lock();
        let claim = uuid_or_null(claim_id);
        match execute(
            &c,
            SQL_STORE_CLAIM_ID,
            "store_claim_id",
            &[&&host_id[..], &claim],
        ) {
            Ok(()) => true,
            Err(Step::Prepare) => false,
            Err(Step::Failed(rc)) => {
                netdata_log_error!("Failed to store host claim id rc = {rc}");
                false
            }
        }
    }

    /// `metadata_scan_host()`'s transaction around `f`: `BEGIN TRANSACTION`, the stores, `COMMIT TRANSACTION`,
    /// whatever the stores did.
    pub fn scan_host<R>(&self, f: impl FnOnce(&HostScan<'_>) -> R) -> R {
        let conn = self.lock();
        let markers = self.markers();
        let _ = conn::db_execute(&conn, "BEGIN TRANSACTION", &markers);
        let scan = HostScan {
            conn: &conn,
            chart: RefCell::new(None),
            dimension: RefCell::new(None),
        };
        let result = f(&scan);
        drop(scan);
        let _ = conn::db_execute(&conn, "COMMIT TRANSACTION", &markers);
        result
    }

    /// `store_ctx_cleanup_list()`'s writes: `sql_schedule_host_ctx_cleanup()` of each (host, context), on one
    /// statement prepared at the first item written; an item met once `shutting_down` holds is skipped.
    pub fn schedule_host_ctx_cleanup(
        &self,
        items: &[([u8; 16], String)],
        shutting_down: impl Fn() -> bool,
    ) {
        let c = self.lock();
        let mut stmt: Option<Statement<'_>> = None;
        for (host_id, context) in items {
            if shutting_down() {
                continue;
            }
            if stmt.is_none() {
                match c.prepare(SCHEDULE_HOST_CTX_CLEANUP) {
                    Ok(prepared) => stmt = Some(prepared),
                    Err(err) => {
                        prepare_failed(&err, "sql_schedule_host_ctx_cleanup");
                        continue;
                    }
                }
            }
            let Some(stmt) = stmt.as_mut() else {
                continue;
            };
            if let Err(err) = conn::retry(|| stmt.execute(rusqlite::params![&host_id[..], context]))
            {
                netdata_log_error!(
                    "Failed to host context check data, rc = {}",
                    conn::result_code(&err)
                );
            }
        }
    }

    /// `delete_dimension_uuid()`.
    pub fn delete_dimension(&self, dim_id: &[u8; 16]) {
        let c = self.lock();
        if let Err(Step::Failed(rc)) = execute(
            &c,
            DELETE_DIMENSION_UUID,
            "delete_dimension_uuid",
            &[&&dim_id[..]],
        ) {
            netdata_log_error!("Failed to delete dimension uuid, rc = {rc}");
        }
    }

    /// `sql_set_host_label()`: one label, stored at once with the `AUTO` source; true when stored.
    pub fn set_host_label(&self, host_id: &[u8; 16], key: &str, value: &str) -> bool {
        let c = self.lock();
        let params: [&dyn ToSql; 4] = [&&host_id[..], &LABEL_SRC_AUTO, &key, &value];
        match execute(&c, SQL_SET_HOST_LABEL, "sql_set_host_label", &params) {
            Ok(()) => true,
            Err(Step::Prepare) => false,
            Err(Step::Failed(rc)) => {
                netdata_log_error!("Failed to store node instance information, rc = {rc}");
                false
            }
        }
    }

    /// `invalidate_node_instances()`: when the host's node instance has another claim id (or none is given), every
    /// node id goes: the `EXISTS` does not refer to the updated row.
    pub fn invalidate_node_instances(&self, host_id: &[u8; 16], claim_id: Option<&[u8; 16]>) {
        let c = self.lock();
        let claim = uuid_or_null(claim_id);
        let params: [&dyn ToSql; 2] = [&&host_id[..], &claim];
        if let Err(Step::Failed(rc)) = execute(
            &c,
            SQL_INVALIDATE_NODE_INSTANCES,
            "invalidate_node_instances",
            &params,
        ) {
            netdata_log_error!("Failed to invalidate node instance information, rc = {rc}");
        }
    }

    /// `sql_unregister_node()`: the host's node id goes, then its last connection is invalidated
    /// (`invalidate_host_last_connected()`).
    pub fn unregister_node(&self, host_id: &[u8; 16]) {
        let c = self.lock();
        match execute(
            &c,
            SQL_UNREGISTER_NODE,
            "sql_unregister_node",
            &[&&host_id[..]],
        ) {
            Err(Step::Prepare) => {}
            Err(Step::Failed(_)) => {
                netdata_log_error!("Failed to execute command to remove host node id")
            }
            Ok(()) => {
                if let Err(Step::Failed(rc)) = execute(
                    &c,
                    SQL_INVALIDATE_HOST_LAST_CONNECTED,
                    "invalidate_host_last_connected",
                    &[&&host_id[..]],
                ) {
                    netdata_log_error!(
                        "Failed invalidate last_connected time for host with GUID {}, rc = {rc}",
                        crate::read::guid(host_id)
                    );
                }
            }
        }
    }

    /// `vacuum_database(db_meta, "METADATA", …)`: at most once a minute, when more than 5% of the pages are free, 10%
    /// of the free pages are given back (`incremental_vacuum(0)`, when that rounds to 0, frees them all).
    pub fn vacuum(&self, next_run: &mut i64, now: i64) {
        if *next_run > now {
            return;
        }
        *next_run = now.saturating_add(VACUUM_FREQUENCY_S);
        let c = self.lock();
        let free_pages = pragma_value(&c, "PRAGMA freelist_count") as i32;
        let total_pages = pragma_value(&c, "PRAGMA page_count") as i32;
        if free_pages > total_pages * FREE_PAGES_THRESHOLD_PC / 100 {
            let do_free_pages = free_pages * FREE_PAGES_VACUUM_PC / 100;
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "METADATA: Freeing {do_free_pages} database pages"
            );
            let _ = conn::db_execute(
                &c,
                &format!("PRAGMA incremental_vacuum({do_free_pages})"),
                &self.markers(),
            );
        }
    }

    /// `sqlite3_wal_checkpoint(db_meta, NULL)`: a passive checkpoint of every attached database, its result unused.
    pub fn wal_checkpoint(&self) {
        let _ = self
            .lock()
            .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()));
    }
}

#[cfg(test)]
mod tests;
