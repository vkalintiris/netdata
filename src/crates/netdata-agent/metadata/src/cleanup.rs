//! The metadata writer's maintenance of `netdata-meta.db` (`run_metadata_cleanup()`, `sqlite_metadata.c`): the
//! context cleanup scan, which drops the dimension rows of the contexts a host queued for cleanup once no tier holds
//! their data, and the WAL size it waits under; the dimension, chart and chart-label cleanup cycles, which drop the
//! rows without data, dimensions or chart in slices of at most 5 s. C's SQL verbatim, with C's records.

use std::collections::HashSet;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error, netdata_log_info};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, Statement};

use crate::conn;
use crate::open::MetaDb;
use crate::read::{prepare_failed, uuid};
use crate::write::text;

const CTX_GET_CONTEXT_META_CLEANUP_LIST: &str =
    "SELECT context FROM ctx_metadata_cleanup WHERE host_id = @host_id";

const SQL_SELECT_HOST_CTX_CHART_LIST: &str =
    "SELECT rowid, context FROM chart WHERE host_id = @host";

const SQL_SELECT_HOST_CTX_CHART_DIM_LIST: &str = "SELECT d.dim_id, d.rowid FROM chart c, dimension d WHERE \
     c.chart_id = d.chart_id AND c.rowid = @rowid";

const SQL_DELETE_DIMENSION_BY_ID: &str =
    "DELETE FROM dimension WHERE rowid = @dimension_row AND dim_id = @uuid";

const CTX_DELETE_CONTEXT_META_CLEANUP_ITEM: &str =
    "DELETE FROM ctx_metadata_cleanup WHERE host_id = @host_id AND context = @context";

/// `SQLITE_METADATA_WAL_LIMIT_X`.
const WAL_LIMIT_X: i64 = 10;

const SELECT_DIMENSION_LIST: &str = "SELECT dim_id, rowid FROM dimension WHERE rowid > @row_id";
const SELECT_CHART_LIST: &str = "SELECT chart_id, rowid FROM chart WHERE rowid > @row_id";
const SELECT_CHART_LABEL_LIST: &str =
    "SELECT chart_id, rowid FROM chart_label WHERE rowid > @row_id";

const SQL_CHECK_CHART_EXISTENCE_IN_DIMENSION: &str =
    "SELECT count(1) FROM dimension WHERE chart_id = @chart_id";
const SQL_CHECK_CHART_EXISTENCE_IN_CHART: &str =
    "SELECT count(1) FROM chart WHERE chart_id = @chart_id";

const SQL_DELETE_CHART_BY_UUID: &str = "DELETE FROM chart WHERE chart_id = @chart_id";
const SQL_DELETE_CHART_LABEL_BY_UUID: &str = "DELETE FROM chart_label WHERE chart_id = @chart_id";

/// `METADATA_MAINTENANCE_FIRST_CHECK`: seconds from a cycle's first call to its first pass.
const MAINTENANCE_FIRST_CHECK_S: i64 = 1800;
/// `METADATA_MAINTENANCE_REPEAT`, `METADATA_LABEL_CHECK_INTERVAL`: seconds from a slice to the next.
const MAINTENANCE_REPEAT_S: i64 = 60;
const LABEL_CHECK_INTERVAL_S: i64 = 3600;
/// `METADATA_RUNTIME_THRESHOLD`: a slice ends at the first row checked more than this many monotonic seconds in.
const RUNTIME_THRESHOLD_S: i64 = 5;
/// `dim_cleanup_cycle.complete_repeat_after`: a completed dimension pass runs again a week later.
const DIMENSION_PASS_REPEAT_S: i64 = 604_800;

/// The table a cleanup cycle walks (`dim_cleanup_cycle`, `chart_cleanup_cycle`, `label_cleanup_cycle`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleKind {
    /// Dimension rows no dbengine tier holds.
    Dimension,
    /// Chart rows no dimension row names.
    Chart,
    /// Chart-label rows no chart row names.
    ChartLabel,
}

/// A cleanup cycle (`struct cleanup_cycle`): its table and its state between the store jobs.
#[derive(Debug, Clone)]
pub struct CleanupCycle {
    kind: CycleKind,
    next_execution_t: i64,
    last_row_id: i64,
    max_row_id: i64,
    snapshot_pending: bool,
    completed: bool,
}

impl CleanupCycle {
    pub fn new(kind: CycleKind) -> CleanupCycle {
        CleanupCycle {
            kind,
            next_execution_t: 0,
            last_row_id: 0,
            max_row_id: 0,
            snapshot_pending: false,
            completed: false,
        }
    }

    /// `select_sql`, `max_rowid_sql`, `repeat_after`, `complete_repeat_after` (0: one pass only) and the three labels.
    fn descriptor(&self) -> (&'static str, &'static str, i64, i64, [&'static str; 3]) {
        match self.kind {
            CycleKind::Dimension => (
                SELECT_DIMENSION_LIST,
                "SELECT MAX(rowid) FROM dimension",
                MAINTENANCE_REPEAT_S,
                DIMENSION_PASS_REPEAT_S,
                ["Dimension", "Dimensions", "dimensions"],
            ),
            CycleKind::Chart => (
                SELECT_CHART_LIST,
                "SELECT MAX(rowid) FROM chart",
                MAINTENANCE_REPEAT_S,
                0,
                ["Chart", "Charts", "charts"],
            ),
            CycleKind::ChartLabel => (
                SELECT_CHART_LABEL_LIST,
                "SELECT MAX(rowid) FROM chart_label",
                LABEL_CHECK_INTERVAL_S,
                0,
                ["Chart label", "Chart labels", "chart labels"],
            ),
        }
    }
}

/// The clocks and checks a cleanup cycle reads: the wall clock and the monotonic seconds (`now_realtime_sec()`,
/// `now_monotonic_sec()`), whether a shutdown began, and the dimension cycle's `dimension_can_be_deleted()`.
pub struct CycleEnv<'a> {
    pub now: &'a dyn Fn() -> i64,
    pub monotonic: &'a dyn Fn() -> i64,
    pub shutting_down: &'a dyn Fn() -> bool,
    pub dimension_can_be_deleted: &'a dyn Fn(&[u8; 16]) -> bool,
}

/// A statement prepared at its first use, as C's `if (!*res) PREPARE_STATEMENT(...)`; `None` when that failed
/// (reported with `function`).
fn prepared<'c, 's>(
    c: &'c Connection,
    stmt: &'s mut Option<Statement<'c>>,
    sql: &str,
    function: &str,
) -> Option<&'s mut Statement<'c>> {
    if stmt.is_none() {
        match c.prepare(sql) {
            Ok(s) => *stmt = Some(s),
            Err(err) => {
                prepare_failed(&err, function);
                return None;
            }
        }
    }
    stmt.as_mut()
}

impl MetaDb {
    /// `sql_metadata_wal_size_acceptable()`: `netdata-meta.db-wal` is at most ten journal size limits (a missing file
    /// counts as -1 bytes).
    pub fn wal_size_acceptable(&self) -> bool {
        let wal = self.cache_dir().join("netdata-meta.db-wal");
        let size = std::fs::metadata(wal).map_or(-1, |m| m.len() as i64);
        size <= WAL_LIMIT_X.saturating_mul(self.journal_size_limit)
    }

    /// `ctx_get_context_list_to_cleanup()` with `cleanup_host_context_metadata()`: when the host queued contexts for
    /// cleanup, each of its charts of such a context has its dimension rows without retention deleted
    /// (`can_be_deleted`), then the context's cleanup row, even when the dimensions stopped short. The scan stops
    /// once a shutdown began (`shutting_down`) or the WAL grew too large; a queued context no chart row has stays.
    pub fn cleanup_host_contexts(
        &self,
        host_id: &[u8; 16],
        hostname: &str,
        can_be_deleted: impl Fn(&[u8; 16]) -> bool,
        shutting_down: impl Fn() -> bool,
    ) {
        let c = self.lock();
        let Some(contexts) = cleanup_list(&c, host_id) else {
            return;
        };
        if contexts.is_empty() {
            return;
        }
        let mut charts = match c.prepare(SQL_SELECT_HOST_CTX_CHART_LIST) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "cleanup_host_context_metadata");
                return;
            }
        };
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Verifying the retention of {} contexts for host {hostname}",
            contexts.len()
        );
        let mut dims: Option<Statement<'_>> = None;
        let mut cleanup_row: Option<Statement<'_>> = None;
        let (mut checked, mut deleted) = (0usize, 0usize);
        let keep_going = || !shutting_down() && self.wal_size_acceptable();
        if let Ok(mut rows) = charts.query([&host_id[..]]) {
            let mut can_continue = true;
            while can_continue {
                let Ok(Some(row)) = rows.next() else {
                    break;
                };
                let chart_row_id = row.get::<_, i64>(0).unwrap_or(0);
                let context = match row.get_ref(1) {
                    Ok(ValueRef::Text(t) | ValueRef::Blob(t)) => Some(t.to_vec()),
                    _ => None,
                };
                if let Some(context) = context.filter(|ctx| contexts.contains(ctx)) {
                    can_continue = clean_chart_dimensions(
                        &c,
                        &mut dims,
                        chart_row_id,
                        &can_be_deleted,
                        &keep_going,
                        (&mut checked, &mut deleted),
                    );
                    delete_cleanup_row(&c, &mut cleanup_row, host_id, &context);
                }
                can_continue = can_continue && keep_going();
            }
        }
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Verified the contexts of host {hostname} (Checked {checked} metrics and removed {deleted})"
        );
    }
}

impl MetaDb {
    /// `run_cleanup_cycle()`: the first call arms the cycle 1800 s later; each call past that time snapshots the
    /// table's largest rowid when a pass begins, completes the pass once the slices reached it (the dimension cycle
    /// runs again a week later, the others never), or runs one slice from the last row seen and arms the next one.
    /// True when the next cycle may run: this one waits, completed or could not start.
    pub fn run_cleanup_cycle(&self, cycle: &mut CleanupCycle, env: &CycleEnv<'_>) -> bool {
        let (select_sql, max_rowid_sql, repeat_after, complete_repeat_after, labels) =
            cycle.descriptor();
        let [singular, plural, lower] = labels;
        if complete_repeat_after == 0 && cycle.completed {
            return true;
        }
        let now = (env.now)();
        if cycle.next_execution_t == 0 {
            cycle.next_execution_t = now.saturating_add(MAINTENANCE_FIRST_CHECK_S);
            cycle.snapshot_pending = true;
        }
        if cycle.next_execution_t > now {
            return true;
        }
        let c = self.lock();
        if cycle.snapshot_pending {
            cycle.max_row_id = max_rowid(&c, max_rowid_sql);
            cycle.snapshot_pending = false;
            netdata_log_info!(
                "{singular} metadata check has been scheduled to run (max id = {})",
                cycle.max_row_id
            );
        }
        if cycle.last_row_id >= cycle.max_row_id {
            netdata_log_info!("{singular} metadata check completed");
            if complete_repeat_after != 0 {
                cycle.next_execution_t = now.saturating_add(complete_repeat_after);
                cycle.last_row_id = 0;
                cycle.snapshot_pending = true;
            } else {
                cycle.completed = true;
            }
            return true;
        }
        let mut select = match c.prepare(select_sql) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "run_cleanup_cycle");
                return true;
            }
        };
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Checking {lower} starting after row {}",
            cycle.last_row_id
        );
        let (checked, deleted) =
            cleanup_slice(&c, &mut select, cycle.kind, &mut cycle.last_row_id, env);
        cycle.next_execution_t = (env.now)().saturating_add(repeat_after);
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "{plural} checked {checked}, deleted {deleted}. Checks will resume in {repeat_after} seconds"
        );
        false
    }
}

/// `get_rowid_from_statement()`: the first column of the first row, 0 without one (or NULL).
fn max_rowid(c: &Connection, sql: &str) -> i64 {
    let mut stmt = match c.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            prepare_failed(&err, "get_rowid_from_statement");
            return 0;
        }
    };
    conn::retry(|| stmt.query_row([], |r| r.get::<_, Option<i64>>(0)))
        .ok()
        .flatten()
        .unwrap_or(0)
}

/// `run_cleanup_loop()`: the rows after `row_id`, until the first one checked more than 5 monotonic seconds in or a
/// shutdown; each row whose check holds is deleted (C's `action_cb`), `row_id` following the rows read. The rows
/// checked and deleted.
fn cleanup_slice(
    c: &Connection,
    select: &mut Statement<'_>,
    kind: CycleKind,
    row_id: &mut i64,
    env: &CycleEnv<'_>,
) -> (u32, u32) {
    let (mut checked, mut deleted) = (0u32, 0u32);
    if (env.shutting_down)() {
        return (checked, deleted);
    }
    let Ok(mut rows) = select.query([*row_id]) else {
        return (checked, deleted);
    };
    let (mut check, mut action): (Option<Statement<'_>>, Option<Statement<'_>>) = (None, None);
    let started = (env.monotonic)();
    let mut time_expired = false;
    while !time_expired {
        let Ok(Some(row)) = rows.next() else {
            break;
        };
        if (env.shutting_down)() {
            break;
        }
        *row_id = row.get::<_, i64>(1).unwrap_or(0);
        let Some(id) = uuid(row, 0) else {
            continue;
        };
        let delete = match kind {
            CycleKind::Dimension => (env.dimension_can_be_deleted)(&id),
            CycleKind::Chart => chart_can_be_deleted(c, &mut check, &id, true),
            CycleKind::ChartLabel => chart_can_be_deleted(c, &mut check, &id, false),
        };
        if delete {
            match kind {
                CycleKind::Dimension => crate::write::delete_dimension_uuid(c, &id),
                CycleKind::Chart => delete_chart_uuid(c, &mut action, &id, false),
                CycleKind::ChartLabel => delete_chart_uuid(c, &mut action, &id, true),
            }
            deleted += 1;
        }
        checked += 1;
        time_expired = (env.monotonic)() - started > RUNTIME_THRESHOLD_S;
    }
    (checked, deleted)
}

/// `chart_can_be_deleted()`: no dimension row (`in_dimension`) or no chart row names the chart; false when the check
/// could not be prepared, and when its step gave no row.
fn chart_can_be_deleted<'c>(
    c: &'c Connection,
    stmt: &mut Option<Statement<'c>>,
    chart_id: &[u8; 16],
    in_dimension: bool,
) -> bool {
    let sql = if in_dimension {
        SQL_CHECK_CHART_EXISTENCE_IN_DIMENSION
    } else {
        SQL_CHECK_CHART_EXISTENCE_IN_CHART
    };
    let Some(stmt) = prepared(c, stmt, sql, "chart_can_be_deleted") else {
        return false;
    };
    conn::retry(|| stmt.query_row([&chart_id[..]], |r| r.get::<_, i64>(0))).unwrap_or(1) == 0
}

/// `delete_chart_uuid()`: the chart's row, or its label rows.
fn delete_chart_uuid<'c>(
    c: &'c Connection,
    stmt: &mut Option<Statement<'c>>,
    chart_id: &[u8; 16],
    label_only: bool,
) {
    let sql = if label_only {
        SQL_DELETE_CHART_LABEL_BY_UUID
    } else {
        SQL_DELETE_CHART_BY_UUID
    };
    let Some(stmt) = prepared(c, stmt, sql, "delete_chart_uuid") else {
        return;
    };
    if let Err(err) = conn::retry(|| stmt.execute([&chart_id[..]])) {
        netdata_log_error!(
            "Failed to delete a chart uuid from the {} table, rc = {}",
            if label_only { "labels" } else { "chart" },
            conn::result_code(&err)
        );
    }
}

/// `ctx_get_context_list_to_cleanup()`'s rows: the host's queued contexts; `None` when the statement could not run.
fn cleanup_list(c: &Connection, host_id: &[u8; 16]) -> Option<HashSet<Vec<u8>>> {
    let mut stmt = match c.prepare(CTX_GET_CONTEXT_META_CLEANUP_LIST) {
        Ok(stmt) => stmt,
        Err(err) => {
            prepare_failed(&err, "ctx_get_context_list_to_cleanup");
            return None;
        }
    };
    let mut contexts = HashSet::new();
    let mut rows = stmt.query([&host_id[..]]).ok()?;
    while let Ok(Some(row)) = rows.next() {
        if let Ok(ValueRef::Text(t) | ValueRef::Blob(t)) = row.get_ref(0) {
            contexts.insert(t.to_vec());
        }
    }
    Some(contexts)
}

/// `clean_host_chart_dimensions()`: the chart's dimension rows, each deleted by rowid when `can_be_deleted` holds;
/// whether the scan may go on (`keep_going` after each dimension; false when the statement could not run).
fn clean_chart_dimensions<'c>(
    c: &'c Connection,
    dims: &mut Option<Statement<'c>>,
    chart_row_id: i64,
    can_be_deleted: &impl Fn(&[u8; 16]) -> bool,
    keep_going: &impl Fn() -> bool,
    (checked, deleted): (&mut usize, &mut usize),
) -> bool {
    let Some(stmt) = prepared(
        c,
        dims,
        SQL_SELECT_HOST_CTX_CHART_DIM_LIST,
        "clean_host_chart_dimensions",
    ) else {
        return false;
    };
    let Ok(mut rows) = stmt.query([chart_row_id]) else {
        return false;
    };
    // prepared at the first delete, finalized with the chart, as C's `dim_del_stmt`
    let mut delete: Option<Statement<'_>> = None;
    let mut can_continue = true;
    while can_continue {
        let Ok(Some(row)) = rows.next() else {
            break;
        };
        let Some(dim_id) = uuid(row, 0) else {
            continue;
        };
        let dimension_row = row.get::<_, i64>(1).unwrap_or(0);
        if can_be_deleted(&dim_id) {
            delete_dimension_by_rowid(c, &mut delete, dimension_row, &dim_id);
            *deleted += 1;
        }
        *checked += 1;
        can_continue = keep_going();
    }
    can_continue
}

/// `delete_dimension_by_rowid()`.
fn delete_dimension_by_rowid<'c>(
    c: &'c Connection,
    stmt: &mut Option<Statement<'c>>,
    dimension_row: i64,
    dim_id: &[u8; 16],
) {
    let Some(stmt) = prepared(
        c,
        stmt,
        SQL_DELETE_DIMENSION_BY_ID,
        "delete_dimension_by_rowid",
    ) else {
        return;
    };
    if let Err(err) = conn::retry(|| stmt.execute(rusqlite::params![dimension_row, &dim_id[..]])) {
        netdata_log_error!(
            "Failed to delete dimension id, rc = {}",
            conn::result_code(&err)
        );
    }
}

/// `ctx_delete_metadata_cleanup_context()`.
fn delete_cleanup_row<'c>(
    c: &'c Connection,
    stmt: &mut Option<Statement<'c>>,
    host_id: &[u8; 16],
    context: &[u8],
) {
    let Some(stmt) = prepared(
        c,
        stmt,
        CTX_DELETE_CONTEXT_META_CLEANUP_ITEM,
        "ctx_delete_metadata_cleanup_context",
    ) else {
        return;
    };
    if let Err(err) = conn::retry(|| stmt.execute(rusqlite::params![&host_id[..], text(context)])) {
        netdata_log_error!(
            "Failed to delete context check entry, rc = {}",
            conn::result_code(&err)
        );
    }
}

#[cfg(test)]
mod tests;
