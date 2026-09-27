//! The metadata writer's maintenance of `netdata-meta.db` (`run_metadata_cleanup()`, `sqlite_metadata.c`): the
//! context cleanup scan, which drops the dimension rows of the contexts a host queued for cleanup once no tier holds
//! their data, and the WAL size it waits under. C's SQL verbatim, with C's records.

use std::collections::HashSet;

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
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
