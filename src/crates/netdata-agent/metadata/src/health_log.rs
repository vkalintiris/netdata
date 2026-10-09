//! The alert log's rows in `netdata-meta.db` (`sqlite_health.c`): the save of an entry (`health_log`,
//! `health_log_detail`, `alert_queue`), the REMOVED rows injected at a host's first health pass, the load, an
//! alarm's id, the retention cleanup. Each method is one C function, with C's statements and C's records.
//!
//! Where C's SQL reads the wall clock itself (`UNIXEPOCH()`, `NOW_USEC(0)`) the caller's clock is bound instead, so
//! that a caller with a scripted clock gets rows that follow it. The values are the same in a running agent.

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};

use netdata_agent_log::netdata_log_error;
use rusqlite::types::{ToSqlOutput, Value, ValueRef};
use rusqlite::{Connection, Row, ToSql};

use crate::conn;
use crate::open::MetaDb;
use crate::read::{End, prepare_failed};
use crate::write::{Step, execute, text};

const SQL_UPDATE_HEALTH_LOG: &str = "UPDATE health_log_detail SET updated_by_id = @updated_by, flags = @flags, \
     exec_run_timestamp = @exec_time, exec_code = @exec_code WHERE unique_id = @unique_id AND alarm_id = @alarm_id \
     AND transition_id = @transaction";

const SQL_INSERT_ALERT_PENDING_QUEUE: &str = "INSERT INTO alert_queue (host_id, health_log_id, unique_id, alarm_id, \
     status, date_scheduled)  VALUES (@host_id, @health_log_id, @unique_id, @alarm_id, @new_status, @delay) ON \
     CONFLICT (host_id, health_log_id, alarm_id) DO UPDATE SET status = excluded.status, unique_id = \
     excluded.unique_id,  date_scheduled = MIN(date_scheduled, excluded.date_scheduled)";

const SQL_INSERT_HEALTH_LOG_DETAIL: &str = "INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, \
     alarm_event_id, updated_by_id, updates_id, when_key, duration, non_clear_duration, flags, exec_run_timestamp, \
     delay_up_to_timestamp, info, exec_code, new_status, old_status, delay, new_value, old_value, last_repeat, \
     transition_id, global_id, summary)  VALUES (@health_log_id,@unique_id,@alarm_id,@alarm_event_id,\
     @updated_by_id,@updates_id,@when_key,@duration,@non_clear_duration,@flags,@exec_run_timestamp,\
     @delay_up_to_timestamp, @info,@exec_code,@new_status,@old_status,@delay,@new_value,@old_value,@last_repeat,\
     @transition_id,@global_id,@summary)";

const SQL_INSERT_HEALTH_LOG: &str = "INSERT INTO health_log (host_id, alarm_id, config_hash_id, name, chart, exec, \
     recipient, units, chart_context, last_transition_id, chart_name) VALUES (@host_id,@alarm_id, @config_hash_id,\
     @name,@chart,@exec,@recipient,@units,@chart_context,@last_transition_id,@chart_name) ON CONFLICT (host_id, \
     alarm_id) DO UPDATE SET last_transition_id = excluded.last_transition_id, chart_name = excluded.chart_name, \
     config_hash_id=excluded.config_hash_id RETURNING health_log_id";

// C: `when_key < UNIXEPOCH() - @history`
const SQL_CLEANUP_HEALTH_LOG_DETAIL: &str = "DELETE FROM health_log_detail WHERE health_log_id IN  (SELECT \
     health_log_id FROM health_log WHERE host_id = @host_id) AND when_key < @now - @history  AND updated_by_id <> 0 \
     AND transition_id NOT IN  (SELECT last_transition_id FROM health_log hl WHERE hl.host_id = @host_id)";

const SQL_UPDATE_TRANSITION_IN_HEALTH_LOG: &str = "UPDATE health_log SET last_transition_id = @transition WHERE \
     alarm_id = @alarm_id AND  last_transition_id = @prev_trans AND host_id = @host_id";

const SQL_SET_UPDATED_BY_IN_HEALTH_LOG_DETAIL: &str = "UPDATE health_log_detail SET flags = flags | @flag, \
     updated_by_id = @updated_by WHERE unique_id = @unique_id AND transition_id = @transition_id";

// C: `when_key` and `delay_up_to_timestamp` are `UNIXEPOCH()`, `global_id` is `NOW_USEC(0)`
const SQL_INJECT_REMOVED: &str = "INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, alarm_event_id, \
     updated_by_id, updates_id, when_key, duration, non_clear_duration, flags, exec_run_timestamp, \
     delay_up_to_timestamp, info, exec_code, new_status, old_status, delay, new_value, old_value, last_repeat, \
     transition_id, global_id, summary) SELECT health_log_id, @max_unique_id, @alarm_id, @alarm_event_id, 0, \
     @unique_id, @now, 0, 0, flags,  exec_run_timestamp, @now, info, exec_code, -2,  new_status, delay, NULL, \
     new_value, 0, @transition_id, @now_usec, summary FROM health_log_detail  WHERE unique_id = @unique_id AND \
     transition_id = @last_transition_id RETURNING health_log_id, old_status";

/// `SQL_SELECT_ALERT_HASH_CLOUD` (`sqlite_aclk_alert.c`).
const SQL_SELECT_ALERT_HASH_CLOUD: &str = "SELECT 1 FROM alert_hash_cloud WHERE hash_id = @hash_id";

const SQL_SELECT_HEALTH_LAST_EXECUTED_EVENT: &str = "SELECT hld.new_status FROM health_log hl, health_log_detail hld \
     WHERE hl.host_id = @host_id AND hl.alarm_id = @alarm_id AND hld.unique_id != @unique_id AND hld.flags & @flags \
     AND hl.health_log_id = hld.health_log_id ORDER BY hld.unique_id DESC LIMIT 1";

const SQL_SELECT_MAX_UNIQUE_ID: &str = "SELECT MAX(hld.unique_id) FROM health_log_detail hld, health_log hl WHERE \
     hl.host_id = @host_id AND hl.health_log_id = hld.health_log_id";

const SQL_SELECT_LAST_STATUSES: &str = "SELECT hld.new_status, hld.unique_id, hld.alarm_id, hld.alarm_event_id, \
     hld.transition_id FROM health_log hl, health_log_detail hld WHERE hl.host_id = @host_id AND \
     hl.last_transition_id = hld.transition_id";

const SQL_LOAD_HEALTH_LOG: &str = "SELECT hld.unique_id, hld.alarm_id, hld.alarm_event_id, hl.config_hash_id, \
     hld.updated_by_id, hld.updates_id, hld.when_key, hld.duration, hld.non_clear_duration, hld.flags, \
     hld.exec_run_timestamp, hld.delay_up_to_timestamp, hl.name, hl.chart, hl.exec, hl.recipient, ah.source, \
     hl.units, hld.info, hld.exec_code, hld.new_status, hld.old_status, hld.delay, hld.new_value, hld.old_value, \
     hld.last_repeat, ah.class, ah.component, ah.type, hl.chart_context, hld.transition_id, hld.global_id, \
     hl.chart_name, hld.summary FROM health_log hl, alert_hash ah, health_log_detail hld WHERE hl.config_hash_id = \
     ah.hash_id and hl.host_id = @host_id and hl.last_transition_id = hld.transition_id";

const SQL_GET_EVENT_ID: &str = "SELECT MAX(alarm_event_id)+1 FROM health_log_detail WHERE health_log_id = \
     @health_log_id AND alarm_id = @alarm_id";

const SQL_GET_ALARM_ID_FROM_TRANSITION_ID: &str = "SELECT hld.alarm_id, hl.host_id, hl.chart_context FROM \
     health_log_detail hld, health_log hl WHERE hld.transition_id = @transition_id AND hld.health_log_id = \
     hl.health_log_id";

const SQL_GET_ALARM_ID: &str =
    "SELECT alarm_id, health_log_id FROM health_log WHERE host_id = @host_id AND chart = @chart AND name = @name";

const SQL_SELECT_HEALTH_LOG: &str = "SELECT hld.unique_id, hld.alarm_id, hld.alarm_event_id, hl.config_hash_id, \
     hld.updated_by_id, hld.updates_id, hld.when_key, hld.duration, hld.non_clear_duration, hld.flags, \
     hld.exec_run_timestamp, hld.delay_up_to_timestamp, hl.name, hl.chart, hl.exec, hl.recipient, ah.source, \
     hl.units, hld.info, hld.exec_code, hld.new_status, hld.old_status, hld.delay, hld.new_value, hld.old_value, \
     hld.last_repeat, ah.class, ah.component, ah.type, hl.chart_context, hld.transition_id, hld.summary FROM \
     health_log hl, alert_hash ah, health_log_detail hld WHERE hl.config_hash_id = ah.hash_id and hl.health_log_id \
     = hld.health_log_id and hl.host_id = @host_id AND hld.unique_id > @after ";

const SQL_DELETE_ORPHAN_HEALTH_LOG: &str = "DELETE FROM health_log WHERE host_id NOT IN (SELECT host_id FROM host)";
const SQL_DELETE_ORPHAN_HEALTH_LOG_DETAIL: &str =
    "DELETE FROM health_log_detail WHERE health_log_id NOT IN (SELECT health_log_id FROM health_log)";
const SQL_DELETE_ORPHAN_ALERT_VERSION: &str =
    "DELETE FROM alert_version WHERE health_log_id NOT IN (SELECT health_log_id FROM health_log)";

const SQL_DELETE_MISSING_CHART_ALERT: &str = "DELETE FROM health_log WHERE host_id = @host_id AND chart NOT IN \
     (SELECT type||'.'||id FROM chart WHERE host_id = @host_id)";
const SQL_HEALTH_CHECK_ALL_HOSTS: &str = "SELECT host_id, hostname FROM host";

// sql_alert_transitions(): the 31 columns of an entry, its alarm and its alarm's rule
macro_rules! sql_search_alert_transition_select {
    () => {
        "SELECT h.host_id, h.alarm_id, h.config_hash_id, h.name, h.chart, h.chart_name, h.family, h.recipient, \
         h.units, h.exec, h.chart_context,  d.when_key, d.duration, d.non_clear_duration, d.flags, \
         d.delay_up_to_timestamp, d.info, d.exec_code, d.new_status, d.old_status, d.delay, d.new_value, \
         d.old_value, d.last_repeat, d.transition_id, d.global_id, ah.class, ah.type, ah.component, \
         d.exec_run_timestamp, d.summary"
    };
}

const SQL_SEARCH_ALERT_TRANSITION_DIRECT: &str = concat!(
    sql_search_alert_transition_select!(),
    " FROM health_log h, health_log_detail d, alert_hash ah  WHERE h.config_hash_id = ah.hash_id AND \
     h.health_log_id = d.health_log_id AND transition_id = @transition "
);

/// `SQL_SEARCH_ALERT_TRANSITION` over the host list `table`, with the two optional tests and the order
/// `sql_alert_transitions()` appends.
fn sql_search_alert_transition(table: &str, context: bool, alert_name: bool) -> String {
    let mut sql = String::from(sql_search_alert_transition_select!());
    sql.push_str(" FROM health_log h, health_log_detail d, ");
    sql.push_str(table);
    sql.push_str(
        " t, alert_hash ah  WHERE h.host_id = t.host_id AND h.config_hash_id = ah.hash_id AND h.health_log_id = \
         d.health_log_id AND ( d.new_status > 2 OR d.old_status > 2 ) AND d.global_id BETWEEN @after AND @before ",
    );
    if context {
        sql.push_str(" AND h.chart_context = @context");
    }
    if alert_name {
        sql.push_str(" AND h.name = @alert_name");
    }
    sql.push_str(" ORDER BY d.global_id DESC");
    sql
}

/// The temporary tables of the host lists, one name per call. C names each after the address of the request's
/// node dictionary (`v_%p`); the table is the connection's own, so no client can tell.
static HOST_LISTS: AtomicU64 = AtomicU64::new(0);

const USEC_PER_SEC: i64 = 1_000_000;

/// The temporary tables of the rule lists, one name per call (C's `c_%p`, the address of the request's dictionary).
static CONFIG_LISTS: AtomicU64 = AtomicU64::new(0);

/// `SQL_SEARCH_CONFIG_LIST` over the rule list `table`: no ORDER BY, so the rows come as SQLite's plan gives them.
fn sql_search_config_list(table: &str) -> String {
    format!(
        "SELECT ah.hash_id, alarm, template, on_key, class, component, type, lookup, every,  units, calc, families, \
         green, red, warn, crit,  exec, to_key, info, delay, options, repeat, host_labels, p_db_lookup_dimensions, \
         p_db_lookup_method,  p_db_lookup_options, p_db_lookup_after, p_db_lookup_before, p_update_every, source, \
         chart_labels, summary,   time_group_condition, time_group_value, dims_group, data_source  FROM alert_hash \
         ah, {table} t where ah.hash_id = t.hash_id"
    )
}

// sqlite_aclk_alert.c
const SQL_SELECT_VARIABLE_ALERT_BY_UNIQUE_ID: &str = "SELECT hld.unique_id FROM health_log hl, alert_hash ah, \
     health_log_detail hld WHERE hld.unique_id = @unique_id AND hl.config_hash_id = ah.hash_id AND \
     hld.health_log_id = hl.health_log_id AND hl.host_id = @host_id AND ah.warn IS NULL AND ah.crit IS NULL";
const SQL_UPDATE_ALERT_VERSION_TRANSITION: &str =
    "UPDATE alert_version SET unique_id = @unique_id WHERE health_log_id = @health_log_id";
const SQL_SELECT_LAST_ALERT_STATUS: &str = "SELECT status FROM alert_version WHERE health_log_id = @health_log_id ";
// C: `date_created` is `UNIXEPOCH()`
const SQL_QUEUE_ALERT_TO_CLOUD: &str = "INSERT INTO aclk_queue (host_id, health_log_id, unique_id, date_created) \
     VALUES (@host_id, @health_log_id, @unique_id, @now) ON CONFLICT(host_id, health_log_id) DO UPDATE SET \
     unique_id=excluded.unique_id,  date_created=excluded.date_created";
const SQL_DELETE_PROCESSED_ROWS: &str =
    "DELETE FROM alert_queue WHERE host_id = @host_id AND rowid = @row AND unique_id = @unique_id";
// C: `date_scheduled <= UNIXEPOCH()`
const SQL_PROCESS_ALERT_PENDING_QUEUE: &str = "SELECT health_log_id, unique_id, status, rowid FROM alert_queue WHERE \
     host_id = @host_id AND date_scheduled <= @now ORDER BY rowid ASC";
const SQL_ALERT_VERSION_CALC: &str = "SELECT SUM(version) FROM health_log hl, alert_version av WHERE hl.host_id = \
     @host_uuid AND hl.health_log_id = av.health_log_id AND av.status <> -2";

/// `HEALTH_ENTRY_FLAG_UPDATED`.
const ENTRY_FLAG_UPDATED: i64 = 0x0000_0002;
/// `HEALTH_ENTRY_FLAG_EXEC_RUN`.
const ENTRY_FLAG_EXEC_RUN: i32 = 0x0000_0004;
/// `RRDCALC_STATUS_REMOVED`.
const STATUS_REMOVED: i32 = -2;

/// `calculate_delay()`: how long the Cloud waits before it is told of a status change, by the status left and the
/// status taken (C's `RRDCALC_STATUS` numbers): 600 seconds, 10, or none.
pub fn calculate_delay(old_status: i32, new_status: i32) -> i64 {
    const NONE: i64 = 0;
    const SHORT: i64 = 10;
    const LONG: i64 = 600;
    const REMOVED: i32 = -2;
    const UNDEFINED: i32 = -1;
    const UNINITIALIZED: i32 = 0;
    const CLEAR: i32 = 1;
    const WARNING: i32 = 3;
    const CRITICAL: i32 = 4;
    match old_status {
        REMOVED => match new_status {
            UNINITIALIZED => LONG,
            CLEAR => SHORT,
            _ => NONE,
        },
        UNDEFINED | UNINITIALIZED => match new_status {
            REMOVED | UNINITIALIZED | UNDEFINED => LONG,
            CLEAR => SHORT,
            _ => NONE,
        },
        CLEAR => match new_status {
            REMOVED | UNINITIALIZED | UNDEFINED => LONG,
            _ => NONE,
        },
        WARNING | CRITICAL => match new_status {
            UNINITIALIZED | UNDEFINED => LONG,
            REMOVED | CLEAR => SHORT,
            _ => NONE,
        },
        _ => NONE,
    }
}

/// An entry as `sql_health_alarm_log_save()` binds it. A `None` text is a NULL.
#[derive(Debug, Clone, Copy)]
pub struct EntryRow<'a> {
    pub unique_id: u32,
    pub alarm_id: u32,
    pub alarm_event_id: u32,
    pub config_hash_id: &'a [u8; 16],
    pub transition_id: &'a [u8; 16],
    pub updated_by_id: u32,
    pub updates_id: u32,
    pub when: i64,
    pub duration: i64,
    pub non_clear_duration: i64,
    pub flags: u32,
    pub exec_run_timestamp: i64,
    pub delay_up_to_timestamp: i64,
    pub name: Option<&'a [u8]>,
    pub chart: Option<&'a [u8]>,
    pub chart_context: Option<&'a [u8]>,
    pub chart_name: Option<&'a [u8]>,
    pub exec: Option<&'a [u8]>,
    pub recipient: Option<&'a [u8]>,
    pub units: Option<&'a [u8]>,
    pub info: Option<&'a [u8]>,
    pub summary: Option<&'a [u8]>,
    pub exec_code: i32,
    pub new_status: i32,
    pub old_status: i32,
    pub delay: i32,
    /// A NaN is stored as NULL, by SQLite.
    pub new_value: f64,
    pub old_value: f64,
    pub last_repeat: i64,
    pub global_id: u64,
}

/// A row of `SQL_LOAD_HEALTH_LOG`, as its columns are: what C checks of a row before it makes an entry of it is the
/// caller's to check.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedRow {
    pub unique_id: u32,
    pub alarm_id: u32,
    pub alarm_event_id: u32,
    pub config_hash_id: Uuid,
    pub updated_by_id: u32,
    pub updates_id: u32,
    pub when: i64,
    pub duration: i64,
    pub non_clear_duration: i64,
    pub flags: u32,
    pub exec_run_timestamp: i64,
    pub delay_up_to_timestamp: i64,
    pub name: Option<Vec<u8>>,
    pub chart: Option<Vec<u8>>,
    pub exec: Option<Vec<u8>>,
    pub recipient: Option<Vec<u8>>,
    pub source: Option<Vec<u8>>,
    pub units: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub exec_code: i32,
    pub new_status: i32,
    pub old_status: i32,
    pub delay: i32,
    /// A NULL reads as 0.0 (`sqlite3_column_double()`).
    pub new_value: f64,
    pub old_value: f64,
    pub last_repeat: i64,
    pub classification: Option<Vec<u8>>,
    pub component: Option<Vec<u8>>,
    pub r#type: Option<Vec<u8>>,
    pub chart_context: Option<Vec<u8>>,
    pub transition_id: Uuid,
    /// None for a NULL.
    pub global_id: Option<u64>,
    pub chart_name: Option<Vec<u8>>,
    pub summary: Option<Vec<u8>>,
}

/// A row of `/api/v1/alarm_log`'s query (`SQL_SELECT_HEALTH_LOG`), as its columns are. A `None` is a NULL.
#[derive(Debug, Clone, PartialEq)]
pub struct AlarmLogRow {
    pub unique_id: i64,
    pub alarm_id: i64,
    pub alarm_event_id: i64,
    pub config_hash_id: Uuid,
    pub updated_by_id: i64,
    pub updates_id: i64,
    pub when: i64,
    pub duration: i64,
    pub non_clear_duration: i64,
    pub flags: i64,
    pub exec_run_timestamp: i64,
    pub delay_up_to_timestamp: i64,
    pub name: Option<Vec<u8>>,
    pub chart: Option<Vec<u8>>,
    pub exec: Option<Vec<u8>>,
    pub recipient: Option<Vec<u8>>,
    pub source: Option<Vec<u8>>,
    pub units: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub exec_code: i32,
    pub new_status: i32,
    pub old_status: i32,
    pub delay: i32,
    pub new_value: Option<f64>,
    pub old_value: Option<f64>,
    pub last_repeat: i64,
    pub classification: Option<Vec<u8>>,
    pub component: Option<Vec<u8>>,
    pub r#type: Option<Vec<u8>>,
    pub chart_context: Option<Vec<u8>>,
    pub transition_id: Uuid,
    pub summary: Option<Vec<u8>>,
}

/// A rule's row of `alert_hash` as `sql_get_alert_configuration()` hands it on. A `None` is a NULL.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertConfigRow {
    pub hash_id: [u8; 16],
    /// An alarm's name; NULL for a template, whose name is in `template`.
    pub alarm: Option<Vec<u8>>,
    pub template: Option<Vec<u8>>,
    pub on_key: Option<Vec<u8>>,
    pub classification: Option<Vec<u8>>,
    pub component: Option<Vec<u8>>,
    pub r#type: Option<Vec<u8>>,
    pub lookup: Option<Vec<u8>>,
    pub every: Option<Vec<u8>>,
    pub units: Option<Vec<u8>>,
    pub calc: Option<Vec<u8>>,
    pub families: Option<Vec<u8>>,
    pub green: Option<Vec<u8>>,
    pub red: Option<Vec<u8>>,
    pub warn: Option<Vec<u8>>,
    pub crit: Option<Vec<u8>>,
    pub exec: Option<Vec<u8>>,
    pub to_key: Option<Vec<u8>>,
    pub info: Option<Vec<u8>>,
    pub delay: Option<Vec<u8>>,
    pub options: Option<Vec<u8>>,
    pub repeat: Option<Vec<u8>>,
    pub host_labels: Option<Vec<u8>>,
    pub db_dimensions: Option<Vec<u8>>,
    pub db_method: Option<Vec<u8>>,
    pub db_options: u32,
    pub db_after: i32,
    pub db_before: i32,
    pub update_every: i32,
    pub source: Option<Vec<u8>>,
    pub chart_labels: Option<Vec<u8>>,
    pub summary: Option<Vec<u8>>,
    pub time_group_condition: i32,
    pub time_group_value: f64,
    pub dims_group: i32,
    pub data_source: i32,
}

/// A UUID column as C's readers tell them apart: NULL, a blob of 16 bytes, or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Uuid {
    Null,
    Valid([u8; 16]),
    Invalid,
}

fn uuid_column(row: &Row<'_>, i: usize) -> Uuid {
    match row.get_ref(i) {
        Ok(ValueRef::Null) => Uuid::Null,
        Ok(ValueRef::Blob(b)) => b.try_into().map_or(Uuid::Invalid, Uuid::Valid),
        _ => Uuid::Invalid,
    }
}

/// A row of [`MetaDb::find_alert_transition`]: the host of the alert (its machine GUID's bytes), the context of
/// the alert's chart as the log has it (none for a NULL; C's callback takes an empty one as none too), and the
/// alert's id, read as C reads it, as an `int`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlertOfTransition {
    pub host_id: [u8; 16],
    pub context: Option<Vec<u8>>,
    pub alarm_id: i32,
}

/// A log entry as `sql_alert_transitions()` reads it and hands it to its callback: the entry's columns, its
/// alarm's, and the class, type and component of the rule its alarm points at now. A text is the row's own bytes
/// while the row stands, `None` for a NULL. The two statuses, the delay and the notifier's exit code are read as
/// C's `int` (the statuses are REAL columns), a NULL value as 0.0, the flags and the times as 64 bits.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionRow<'r> {
    pub host_id: [u8; 16],
    pub alarm_id: i64,
    pub config_hash_id: [u8; 16],
    pub alert_name: Option<Cow<'r, [u8]>>,
    pub chart: Option<Cow<'r, [u8]>>,
    pub chart_name: Option<Cow<'r, [u8]>>,
    pub family: Option<Cow<'r, [u8]>>,
    pub recipient: Option<Cow<'r, [u8]>>,
    pub units: Option<Cow<'r, [u8]>>,
    pub exec: Option<Cow<'r, [u8]>>,
    pub chart_context: Option<Cow<'r, [u8]>>,
    pub when_key: i64,
    pub duration: i64,
    pub non_clear_duration: i64,
    pub flags: i64,
    pub delay_up_to_timestamp: i64,
    pub info: Option<Cow<'r, [u8]>>,
    pub exec_code: i32,
    pub new_status: i32,
    pub old_status: i32,
    pub delay: i32,
    pub new_value: f64,
    pub old_value: f64,
    pub last_repeat: i64,
    pub transition_id: [u8; 16],
    pub global_id: i64,
    pub classification: Option<Cow<'r, [u8]>>,
    pub r#type: Option<Cow<'r, [u8]>>,
    pub component: Option<Cow<'r, [u8]>>,
    pub exec_run_timestamp: i64,
    pub summary: Option<Cow<'r, [u8]>>,
}

/// What [`MetaDb::alert_transitions`] is asked for.
#[derive(Debug, Clone, Copy)]
pub enum TransitionsOf<'a> {
    /// The entries with this transition id, whatever their host, their time and their statuses, in the table's
    /// order.
    Id(&'a [u8; 16]),
    /// The entries of the hosts listed (each id as `health_log.host_id` holds it) that changed from or to WARNING
    /// or CRITICAL, with a global id (the entry's time in microseconds) from `after_s` to `before_s`, in seconds
    /// and both included; of one chart context and of one alert name when given, each compared whole and with its
    /// case. Newest first.
    Window {
        hosts: &'a [[u8; 16]],
        after_s: i64,
        before_s: i64,
        context: Option<&'a [u8]>,
        alert_name: Option<&'a [u8]>,
    },
}

/// The id of a transition's row that is no 16-byte blob, in the order C tests them. C skips such a row.
#[derive(Clone, Copy)]
enum BadId {
    Host,
    ConfigHash,
    Transition,
}

/// One row of `sql_alert_transitions()`'s two statements.
fn transition_row<'r>(row: &'r Row<'_>) -> Result<TransitionRow<'r>, BadId> {
    let id = |i, bad| match uuid_column(row, i) {
        Uuid::Valid(id) => Ok(id),
        _ => Err(bad),
    };
    let host_id = id(0, BadId::Host)?;
    let config_hash_id = id(2, BadId::ConfigHash)?;
    let transition_id = id(24, BadId::Transition)?;
    let text = |i| text_ref(row, i);
    Ok(TransitionRow {
        host_id,
        alarm_id: int(row, 1),
        config_hash_id,
        alert_name: text(3),
        chart: text(4),
        chart_name: text(5),
        family: text(6),
        recipient: text(7),
        units: text(8),
        exec: text(9),
        chart_context: text(10),
        when_key: int(row, 11),
        duration: int(row, 12),
        non_clear_duration: int(row, 13),
        flags: int(row, 14),
        delay_up_to_timestamp: int(row, 15),
        info: text(16),
        // sqlite3_column_int(): the low 32 bits
        exec_code: int(row, 17) as i32,
        new_status: int(row, 18) as i32,
        old_status: int(row, 19) as i32,
        delay: int(row, 20) as i32,
        new_value: double(row, 21),
        old_value: double(row, 22),
        last_repeat: int(row, 23),
        transition_id,
        global_id: int(row, 25),
        classification: text(26),
        r#type: text(27),
        component: text(28),
        exec_run_timestamp: int(row, 29),
        summary: text(30),
    })
}

/// A temporary list as C fills it (a window's hosts in `host_id`, a request's rules in `hash_id`): one row per id;
/// an insert that fails is reported and the others go on. False when the insert cannot be prepared (reported).
fn fill_temp_list(c: &Connection, table: &str, column: &str, ids: &[[u8; 16]]) -> bool {
    let Ok(mut stmt) = c.prepare(&format!("INSERT INTO {table} ({column}) VALUES (@{column})")) else {
        netdata_log_error!("Failed to prepare statement to INSERT into {table}");
        return false;
    };
    for id in ids {
        let params: [&dyn ToSql; 1] = [&&id[..]];
        if conn::retry(|| stmt.execute(&params[..])).is_err() {
            netdata_log_error!("Error while populating temp table");
        }
    }
    true
}

/// One row of `SQL_SEARCH_CONFIG_LIST` as `sql_get_alert_configuration()` reads it; `None` when its hash is no
/// 16-byte blob (C skips the row and counts it).
fn alert_config_row(row: &Row<'_>) -> Option<AlertConfigRow> {
    let Uuid::Valid(hash_id) = uuid_column(row, 0) else {
        return None;
    };
    let text = |i| bytes_or_null(row, i);
    Some(AlertConfigRow {
        hash_id,
        alarm: text(1),
        template: text(2),
        on_key: text(3),
        classification: text(4),
        component: text(5),
        r#type: text(6),
        lookup: text(7),
        every: text(8),
        units: text(9),
        calc: text(10),
        families: text(11),
        green: text(12),
        red: text(13),
        warn: text(14),
        crit: text(15),
        exec: text(16),
        to_key: text(17),
        info: text(18),
        delay: text(19),
        options: text(20),
        repeat: text(21),
        host_labels: text(22),
        db_dimensions: text(23),
        db_method: text(24),
        db_options: int(row, 25) as u32,
        db_after: int(row, 26) as i32,
        db_before: int(row, 27) as i32,
        update_every: int(row, 28) as i32,
        source: text(29),
        chart_labels: text(30),
        summary: text(31),
        time_group_condition: int(row, 32) as i32,
        time_group_value: double(row, 33),
        dims_group: int(row, 34) as i32,
        data_source: int(row, 35) as i32,
    })
}

/// `SQLITE3_BIND_STRING_OR_NULL()`.
fn text_or_null(value: Option<&[u8]>) -> ToSqlOutput<'_> {
    value.map_or(ToSqlOutput::Owned(Value::Null), text)
}

/// `sqlite3_column_int64()`: NULL as 0, a real truncated, a text's leading number.
fn int(row: &Row<'_>, i: usize) -> i64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v,
        Ok(ValueRef::Real(v)) => v as i64,
        Ok(ValueRef::Text(t)) => {
            let t = String::from_utf8_lossy(t);
            let t = t.trim_start();
            let digits = |(i, c): (usize, char)| c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'));
            let end = t.char_indices().find(|&at| !digits(at)).map_or(t.len(), |(i, _)| i);
            t[..end].parse().unwrap_or(0)
        }
        _ => 0,
    }
}

/// `sqlite3_column_double()`: NULL as 0.0.
fn double(row: &Row<'_>, i: usize) -> f64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v as f64,
        Ok(ValueRef::Real(v)) => v,
        Ok(ValueRef::Text(t)) => String::from_utf8_lossy(t).trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// A value column: `None` for a NULL (`sqlite3_column_type() == SQLITE_NULL`), else `sqlite3_column_double()`.
fn double_or_null(row: &Row<'_>, i: usize) -> Option<f64> {
    match row.get_ref(i) {
        Ok(ValueRef::Null) | Err(_) => None,
        _ => Some(double(row, i)),
    }
}

/// `SQLITE3_COLUMN_STRINGDUP_OR_NULL()`: the column's bytes, `None` for a NULL.
fn bytes_or_null(row: &Row<'_>, i: usize) -> Option<Vec<u8>> {
    match row.get_ref(i) {
        Ok(ValueRef::Null) | Err(_) => None,
        Ok(ValueRef::Text(t) | ValueRef::Blob(t)) => Some(t.to_vec()),
        Ok(ValueRef::Integer(v)) => Some(v.to_string().into_bytes()),
        Ok(ValueRef::Real(v)) => Some(v.to_string().into_bytes()),
    }
}

/// `sqlite3_column_text()` without a copy: the column's bytes while the row stands, `None` for a NULL.
fn text_ref<'r>(row: &'r Row<'_>, i: usize) -> Option<Cow<'r, [u8]>> {
    match row.get_ref(i) {
        Ok(ValueRef::Text(t) | ValueRef::Blob(t)) => Some(Cow::Borrowed(t)),
        _ => bytes_or_null(row, i).map(Cow::Owned),
    }
}

/// Whether a step failed because the database is busy or locked: `sqlite3_step_monitored()` makes such a step
/// again, up to `MAX_RETRY` times.
fn busy(err: &rusqlite::Error) -> bool {
    matches!(conn::result_code(err), conn::SQLITE_BUSY | conn::SQLITE_LOCKED)
}

/// The rows of a query, each handed to `f`; false when the statement cannot be prepared (reported with the C
/// function's name). A step that fails ends the rows, as C's `while (step == SQLITE_ROW)`, and the finalize that
/// follows it is recorded; a busy or locked database is waited for before the first row (rusqlite cannot step
/// again a statement whose step failed, so the query starts over, which it can only do while `f` saw nothing).
fn rows(c: &Connection, sql: &str, function: &str, params: &[&dyn ToSql], f: impl FnMut(&Row<'_>) -> bool) -> bool {
    rows_ended(c, sql, function, params, End::Finalize, f)
}

/// [`rows`] for a statement C lets go of with `end`.
fn rows_ended(
    c: &Connection,
    sql: &str,
    function: &str,
    params: &[&dyn ToSql],
    end: End,
    f: impl FnMut(&Row<'_>) -> bool,
) -> bool {
    let mut stmt = match c.prepare(sql) {
        Ok(stmt) => stmt,
        Err(err) => {
            prepare_failed(&err, function);
            return false;
        }
    };
    statement_rows(&mut stmt, function, params, end, f);
    true
}

/// The rows of a statement that is prepared, as [`rows_ended`] hands them out.
fn statement_rows(
    stmt: &mut rusqlite::Statement<'_>,
    function: &str,
    params: &[&dyn ToSql],
    end: End,
    mut f: impl FnMut(&Row<'_>) -> bool,
) {
    let mut attempt = 1;
    loop {
        let Ok(mut rows) = stmt.query(params) else {
            return;
        };
        let mut seen = false;
        loop {
            match rows.next() {
                Ok(Some(row)) => {
                    seen = true;
                    if !f(row) {
                        return;
                    }
                }
                Ok(None) => return,
                Err(err) if !seen && busy(&err) && attempt < conn::MAX_RETRY => break,
                Err(err) => {
                    end.failed(conn::result_code(&err), function);
                    return;
                }
            }
        }
        attempt += 1;
        std::thread::sleep(conn::RETRY_DELAY);
    }
}

/// The next row of a walk C makes with `while (step == SQLITE_ROW)`: none at its end, and none at a step that
/// failed, which the statement's `end` in `function` then records.
fn next_row<'a, 's>(rows: &'a mut rusqlite::Rows<'s>, end: End, function: &str) -> Option<&'a Row<'s>> {
    match rows.next() {
        Ok(row) => row,
        Err(err) => {
            end.failed(conn::result_code(&err), function);
            None
        }
    }
}

/// `insert_alert_queue()` on a connection already held: the alarm's row of `alert_queue`, due `trigger_time` plus
/// the delay of the status change; an alarm already queued keeps its earliest due time.
#[allow(clippy::too_many_arguments)]
fn insert_alert_queue(
    c: &Connection,
    end: End,
    hostname: &str,
    host_id: &[u8; 16],
    health_log_id: i64,
    unique_id: u32,
    alarm_id: u32,
    old_status: i32,
    new_status: i32,
    trigger_time: i64,
) {
    let submit_delay = trigger_time.saturating_add(calculate_delay(old_status, new_status));
    let params: [&dyn ToSql; 6] =
        [&&host_id[..], &health_log_id, &i64::from(unique_id), &i64::from(alarm_id), &new_status, &submit_delay];
    if let Err(Step::Failed(rc)) = execute(c, SQL_INSERT_ALERT_PENDING_QUEUE, "insert_alert_queue", &params) {
        netdata_log_error!("HEALTH [{hostname}]: Failed to execute insert_alert_queue, rc = {rc}");
        end.failed(rc, "insert_alert_queue");
    }
}

impl MetaDb {
    /// `sql_health_alarm_log_insert()`: the alarm's row of `health_log` (made, or pointed at this entry), then the
    /// entry's row of `health_log_detail`, then, with `queue` (the host has its ACLK sync configuration), the
    /// alarm's row of `alert_queue`, whether or not the detail row went in. True when the detail row was inserted:
    /// C marks the entry as saved then. The row's `flags` are the entry's before that mark. `health_thread`: the
    /// caller is HEALTH, which keeps these statements compiled; it shows in the record of a step that failed.
    pub fn health_alarm_log_insert(
        &self,
        hostname: &str,
        host_id: &[u8; 16],
        entry: &EntryRow<'_>,
        queue: bool,
        health_thread: bool,
    ) -> bool {
        let end = End::of_health_statement(health_thread);
        let c = self.lock();
        let mut stmt = match c.prepare(SQL_INSERT_HEALTH_LOG) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "sql_health_alarm_log_insert");
                return false;
            }
        };
        let params: [&dyn ToSql; 11] = [
            &&host_id[..],
            &i64::from(entry.alarm_id),
            &&entry.config_hash_id[..],
            &text_or_null(entry.name),
            &text_or_null(entry.chart),
            &text_or_null(entry.exec),
            &text_or_null(entry.recipient),
            &text_or_null(entry.units),
            &text_or_null(entry.chart_context),
            &&entry.transition_id[..],
            &text_or_null(entry.chart_name),
        ];
        // C steps the alarm's statement once and resets it only after the entry's row and the queue's row are
        // written: the three writes are one transaction. The statement's rows are kept until then for the same.
        let mut attempt = 1;
        let (health_log_id, alarm_row) = loop {
            let stepped = stmt.query(&params[..]).and_then(|mut rows| {
                let id = rows.next()?.map(|row| row.get::<_, i64>(0)).transpose()?;
                Ok((id, rows))
            });
            match stepped {
                Ok((Some(id), rows)) => break (id, rows),
                // a step that ends without a row is SQLITE_DONE (101) in C's record
                Ok((None, _)) => {
                    netdata_log_error!("HEALTH [{hostname}]: Failed to execute SQL_INSERT_HEALTH_LOG, rc = 101");
                    return false;
                }
                Err(err) if busy(&err) && attempt < conn::MAX_RETRY => {
                    attempt += 1;
                    std::thread::sleep(conn::RETRY_DELAY);
                }
                Err(err) => {
                    let rc = conn::result_code(&err);
                    netdata_log_error!("HEALTH [{hostname}]: Failed to execute SQL_INSERT_HEALTH_LOG, rc = {rc}");
                    end.failed(rc, "sql_health_alarm_log_insert");
                    return false;
                }
            }
        };

        let detail: [&dyn ToSql; 23] = [
            &health_log_id,
            &i64::from(entry.unique_id),
            &i64::from(entry.alarm_id),
            &i64::from(entry.alarm_event_id),
            &i64::from(entry.updated_by_id),
            &i64::from(entry.updates_id),
            &entry.when,
            &entry.duration,
            &entry.non_clear_duration,
            &i64::from(entry.flags),
            &entry.exec_run_timestamp,
            &entry.delay_up_to_timestamp,
            &text_or_null(entry.info),
            &entry.exec_code,
            &entry.new_status,
            &entry.old_status,
            &entry.delay,
            &entry.new_value,
            &entry.old_value,
            &entry.last_repeat,
            &&entry.transition_id[..],
            &(entry.global_id as i64),
            &text_or_null(entry.summary),
        ];
        let function = "sql_health_alarm_log_insert_detail";
        let saved = match execute(&c, SQL_INSERT_HEALTH_LOG_DETAIL, function, &detail) {
            Ok(()) => true,
            Err(Step::Prepare) => false,
            Err(Step::Failed(rc)) => {
                netdata_log_error!("HEALTH [{hostname}]: Failed to execute SQL_INSERT_HEALTH_LOG_DETAIL, rc = {rc}");
                end.failed(rc, function);
                false
            }
        };
        if queue {
            let (unique_id, alarm_id) = (entry.unique_id, entry.alarm_id);
            let (old, new) = (entry.old_status, entry.new_status);
            insert_alert_queue(&c, end, hostname, host_id, health_log_id, unique_id, alarm_id, old, new, entry.when);
        }
        drop(alarm_row);
        saved
    }

    /// `sql_health_alarm_log_update()`: an entry that was saved before: who replaced it, its flags, and what its
    /// notification's execution left. `health_thread` as for the insert.
    pub fn health_alarm_log_update(&self, hostname: &str, entry: &EntryRow<'_>, health_thread: bool) {
        let params: [&dyn ToSql; 7] = [
            &i64::from(entry.updated_by_id),
            &i64::from(entry.flags),
            &entry.exec_run_timestamp,
            &entry.exec_code,
            &i64::from(entry.unique_id),
            &i64::from(entry.alarm_id),
            &&entry.transition_id[..],
        ];
        let c = self.lock();
        if let Err(Step::Failed(rc)) = execute(&c, SQL_UPDATE_HEALTH_LOG, "sql_health_alarm_log_update", &params) {
            netdata_log_error!("HEALTH [{hostname}]: Failed to update health log, rc = {rc}");
            End::of_health_statement(health_thread).failed(rc, "sql_health_alarm_log_update");
        }
    }

    /// `sql_check_removed_alerts_state()`: at a host's first health pass, every alarm whose last entry (the one its
    /// `last_transition_id` names) is not REMOVED gets a REMOVED row: the agent stopped without one. The walk over
    /// the alarms ends when `running` says the service stops. Each new row takes the next unique id above the
    /// host's highest, the alarm's next event id, a transition id from `transition_id`, and `now` as its time; the
    /// row it replaces is marked as updated by it, the alarm points at it, and with `queue` the alarm's row of
    /// `alert_queue` is written (`health_thread` as for an entry's insert).
    #[allow(clippy::too_many_arguments)]
    pub fn check_removed_alerts_state(
        &self,
        hostname: &str,
        host_id: &[u8; 16],
        running: &dyn Fn() -> bool,
        queue: bool,
        health_thread: bool,
        now_usec: &mut dyn FnMut() -> u64,
        transition_id: &mut dyn FnMut() -> [u8; 16],
    ) {
        let c = self.lock();
        let mut candidates: Vec<(u32, u32, u32, [u8; 16])> = Vec::new();
        let host: [&dyn ToSql; 1] = [&&host_id[..]];
        let prepared = rows(&c, SQL_SELECT_LAST_STATUSES, "sql_check_removed_alerts_state", &host, |row| {
            match uuid_column(row, 4) {
                Uuid::Valid(last_transition) => {
                    // C reads the REAL column as an int
                    if double(row, 0) as i32 != STATUS_REMOVED {
                        let ids = (int(row, 1) as u32, int(row, 2) as u32, int(row, 3) as u32);
                        candidates.push((ids.0, ids.1, ids.2, last_transition));
                    }
                }
                _ => {
                    netdata_log_error!(
                        "HEALTH [{hostname}]: Got invalid transition id while checking removed alerts. Ignoring it."
                    );
                    return true;
                }
            }
            running()
        });
        if !prepared || candidates.is_empty() {
            return;
        }

        // sql_get_max_unique_id()
        let mut max_unique_id = 0u32;
        rows(&c, SQL_SELECT_MAX_UNIQUE_ID, "sql_get_max_unique_id", &host, |row| {
            max_unique_id = int(row, 0) as u32;
            true
        });

        for (unique_id, alarm_id, alarm_event_id, last_transition) in candidates {
            max_unique_id = max_unique_id.wrapping_add(1);
            // sql_inject_removed_status()
            if alarm_id == 0 || alarm_event_id == 0 || unique_id == 0 || max_unique_id == 0 {
                continue;
            }
            let new_transition = transition_id();
            // each row is a statement of its own in C, with its own reads of the clock
            let now_usec = now_usec();
            let now = (now_usec / 1_000_000) as i64;
            let params: &[(&str, &dyn ToSql)] = &[
                ("@max_unique_id", &i64::from(max_unique_id)),
                ("@alarm_id", &i64::from(alarm_id)),
                ("@alarm_event_id", &(i64::from(alarm_event_id) + 1)),
                ("@unique_id", &i64::from(unique_id)),
                ("@transition_id", &&new_transition[..]),
                ("@last_transition_id", &&last_transition[..]),
                ("@now", &now),
                ("@now_usec", &(now_usec as i64)),
            ];
            let mut stmt = match c.prepare(SQL_INJECT_REMOVED) {
                Ok(stmt) => stmt,
                Err(err) => {
                    prepare_failed(&err, "sql_inject_removed_status");
                    continue;
                }
            };
            // C writes what follows from the new row while the statement that made it is still active: one
            // transaction. The statement's rows are kept until then for the same.
            let mut attempt = 1;
            let injected = loop {
                let stepped = stmt.query(params).and_then(|mut rows| {
                    let row = rows.next()?.map(|row| (int(row, 0), double(row, 1) as i32));
                    Ok((row, rows))
                });
                match stepped {
                    Err(err) if busy(&err) && attempt < conn::MAX_RETRY => {
                        attempt += 1;
                        std::thread::sleep(conn::RETRY_DELAY);
                    }
                    stepped => break stepped,
                }
            };
            // a step that failed shows in the statement's finalize only
            if let Err(err) = &injected {
                End::Finalize.failed(conn::result_code(err), "sql_inject_removed_status");
            }
            if let Ok((Some((health_log_id, old_status)), _new_row)) = injected {
                // sql_set_updated_by_in_health_log_detail()
                let updated: [&dyn ToSql; 4] =
                    [&ENTRY_FLAG_UPDATED, &i64::from(max_unique_id), &i64::from(unique_id), &&last_transition[..]];
                let function = "sql_set_updated_by_in_health_log_detail";
                let step = execute(&c, SQL_SET_UPDATED_BY_IN_HEALTH_LOG_DETAIL, function, &updated);
                if let Err(Step::Failed(rc)) = step {
                    netdata_log_error!("HEALTH [N/A]: Failed to execute SQL_INJECT_REMOVED_UPDATE_DETAIL, rc = {rc}");
                    End::Finalize.failed(rc, function);
                }
                // sql_update_transition_in_health_log()
                let pointed: [&dyn ToSql; 4] =
                    [&&new_transition[..], &i64::from(alarm_id), &&last_transition[..], &&host_id[..]];
                let function = "sql_update_transition_in_health_log";
                if let Err(Step::Failed(rc)) = execute(&c, SQL_UPDATE_TRANSITION_IN_HEALTH_LOG, function, &pointed) {
                    netdata_log_error!("HEALTH [N/A]: Failed to execute SQL_INJECT_REMOVED_UPDATE_DETAIL, rc = {rc}");
                    End::Finalize.failed(rc, function);
                }
                if queue {
                    let (id, removed, end) = (max_unique_id, STATUS_REMOVED, End::of_health_statement(health_thread));
                    let old = old_status;
                    insert_alert_queue(&c, end, hostname, host_id, health_log_id, id, alarm_id, old, removed, now);
                }
            }
        }
    }

    /// `sql_health_alarm_log_load()`'s query: the last entry of each alarm of the host whose rule is in
    /// `alert_hash`, in the order SQLite gives them, each handed to `each` until it says to stop. False when the
    /// statement cannot be prepared.
    pub fn load_health_log(&self, host_id: &[u8; 16], mut each: impl FnMut(LoadedRow) -> bool) -> bool {
        let c = self.lock();
        let host: [&dyn ToSql; 1] = [&&host_id[..]];
        rows(&c, SQL_LOAD_HEALTH_LOG, "sql_health_alarm_log_load", &host, |row| {
            each(LoadedRow {
                unique_id: int(row, 0) as u32,
                alarm_id: int(row, 1) as u32,
                alarm_event_id: int(row, 2) as u32,
                config_hash_id: uuid_column(row, 3),
                updated_by_id: int(row, 4) as u32,
                updates_id: int(row, 5) as u32,
                when: int(row, 6),
                duration: int(row, 7),
                non_clear_duration: int(row, 8),
                flags: int(row, 9) as u32,
                exec_run_timestamp: int(row, 10),
                delay_up_to_timestamp: int(row, 11),
                name: bytes_or_null(row, 12),
                chart: bytes_or_null(row, 13),
                exec: bytes_or_null(row, 14),
                recipient: bytes_or_null(row, 15),
                source: bytes_or_null(row, 16),
                units: bytes_or_null(row, 17),
                info: bytes_or_null(row, 18),
                exec_code: int(row, 19) as i32,
                new_status: double(row, 20) as i32,
                old_status: double(row, 21) as i32,
                delay: int(row, 22) as i32,
                new_value: double(row, 23),
                old_value: double(row, 24),
                last_repeat: int(row, 25),
                classification: bytes_or_null(row, 26),
                component: bytes_or_null(row, 27),
                r#type: bytes_or_null(row, 28),
                chart_context: bytes_or_null(row, 29),
                transition_id: uuid_column(row, 30),
                global_id: match row.get_ref(31) {
                    Ok(ValueRef::Null) | Err(_) => None,
                    _ => Some(int(row, 31) as u64),
                },
                chart_name: bytes_or_null(row, 32),
                summary: bytes_or_null(row, 33),
            })
        })
    }

    /// `sql_get_alarm_id()`: the alarm id the table has for a chart and a rule's name, whatever the rule's hash
    /// (the last row when several match), with its next event id: one above the highest saved for the alarm, 0
    /// when the alarm has no entry. `None` when the table does not know the alarm, or holds 0 for it.
    pub fn get_alarm_id(&self, host_id: &[u8; 16], chart: &[u8], name: Option<&[u8]>) -> Option<(u32, u32)> {
        let c = self.lock();
        let (mut alarm_id, mut health_log_id) = (0u32, 0i64);
        let params: [&dyn ToSql; 3] = [&&host_id[..], &text(chart), &text_or_null(name)];
        rows(&c, SQL_GET_ALARM_ID, "sql_get_alarm_id", &params, |row| {
            alarm_id = int(row, 0) as u32;
            health_log_id = int(row, 1);
            true
        });
        if alarm_id == 0 {
            return None;
        }
        // get_next_alarm_event_id(): the alarm id when the statement cannot be prepared
        let mut next_event_id = alarm_id;
        match c.prepare(SQL_GET_EVENT_ID) {
            Ok(mut stmt) => {
                let params: [&dyn ToSql; 2] = [&health_log_id, &i64::from(alarm_id)];
                if let Ok(mut rows) = stmt.query(&params[..]) {
                    while let Some(row) = next_row(&mut rows, End::Finalize, "get_next_alarm_event_id") {
                        next_event_id = int(row, 0) as u32;
                    }
                }
            }
            Err(_) => netdata_log_error!("Failed to prepare statement when trying to get an event id"),
        }
        Some((alarm_id, next_event_id))
    }

    /// `sql_find_alert_transition()`, after its parse of the id: the alert each log entry with this transition id
    /// belongs to, as C hands it to its callback. C's answer is "a row called back", which is false for an empty
    /// list; a statement that cannot be prepared gives the empty list too. A row whose host id is not a UUID is
    /// reported and skipped, and does not count.
    pub fn find_alert_transition(&self, transition: &[u8; 16]) -> Vec<AlertOfTransition> {
        let c = self.lock();
        let mut found = Vec::new();
        let params: [&dyn ToSql; 1] = [&&transition[..]];
        rows(&c, SQL_GET_ALARM_ID_FROM_TRANSITION_ID, "sql_find_alert_transition", &params, |row| {
            match uuid_column(row, 1) {
                Uuid::Valid(host_id) => found.push(AlertOfTransition {
                    host_id,
                    context: bytes_or_null(row, 2),
                    // sqlite3_column_int(): the stored id cut to 32 bits
                    alarm_id: int(row, 0) as i32,
                }),
                _ => {
                    let what = "Got invalid machine guid while looking up alert transition";
                    netdata_log_error!("HEALTH: {what}. Ignoring it.");
                }
            }
            true
        });
        found
    }

    /// `sql_health_get_last_executed_event()`: the status of the alarm's newest entry whose notification command
    /// was run (its row has the run mark), the entry `unique_id` aside. `None` when the statement cannot be
    /// prepared (C's -1), `Some(None)` without such an entry (C's 0). `health_thread` as for an entry's insert.
    pub fn get_last_executed_event(
        &self,
        host_id: &[u8; 16],
        alarm_id: u32,
        unique_id: u32,
        health_thread: bool,
    ) -> Option<Option<i32>> {
        let c = self.lock();
        let mut status = None;
        // C binds the two ids with sqlite3_bind_int(): an id above 2^31 is a negative number to the statement
        let params: [&dyn ToSql; 4] = [&&host_id[..], &(alarm_id as i32), &(unique_id as i32), &ENTRY_FLAG_EXEC_RUN];
        let (function, end) = ("sql_health_get_last_executed_event", End::of_health_statement(health_thread));
        let prepared = rows_ended(&c, SQL_SELECT_HEALTH_LAST_EXECUTED_EVENT, function, &params, end, |row| {
            status = Some(int(row, 0) as i32);
            true
        });
        prepared.then_some(status)
    }

    /// `alert_hash_has_transitioned()`: whether the table of the rules the Cloud was sent has this hash. False
    /// when the statement cannot be prepared (with C's record). `health_thread` as for an entry's insert.
    pub fn alert_hash_has_transitioned(&self, hash_id: &[u8; 16], health_thread: bool) -> bool {
        let c = self.lock();
        let mut found = false;
        let params: [&dyn ToSql; 1] = [&&hash_id[..]];
        let (function, end) = ("alert_hash_has_transitioned", End::of_health_statement(health_thread));
        rows_ended(&c, SQL_SELECT_ALERT_HASH_CLOUD, function, &params, end, |_| {
            found = true;
            false
        });
        found
    }

    /// `sql_health_alarm_log_cleanup()`'s statement: the host's rows older than `retention_s` at `now` that a
    /// newer entry replaced go, but for those an alarm's `last_transition_id` names. False when the statement
    /// cannot be prepared: C then leaves the memory log alone too.
    pub fn health_alarm_log_cleanup(&self, host_id: &[u8; 16], retention_s: u32, now: i64) -> bool {
        let c = self.lock();
        let params: &[(&str, &dyn ToSql)] =
            &[("@host_id", &&host_id[..]), ("@now", &now), ("@history", &i64::from(retention_s))];
        let mut stmt = match c.prepare(SQL_CLEANUP_HEALTH_LOG_DETAIL) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "sql_health_alarm_log_cleanup");
                return false;
            }
        };
        if let Err(err) = conn::retry(|| stmt.execute(params)) {
            let rc = conn::result_code(&err);
            netdata_log_error!("Failed to cleanup health log detail table, rc = {rc}");
            End::Finalize.failed(rc, "sql_health_alarm_log_cleanup");
        }
        true
    }
}

impl MetaDb {
    /// `sql_health_alarm_log2json()`'s query: the host's entries with a unique id above `after`, of `chart` only
    /// when one is given, newest first, at most `limit`, each with its alarm's and its rule's columns (an alarm
    /// whose rule is not in `alert_hash` has no row). False when the statement cannot be prepared, with C's two
    /// records: the body is then empty.
    pub fn alarm_log(
        &self,
        host_id: &[u8; 16],
        after: i64,
        chart: Option<&[u8]>,
        limit: u32,
        mut each: impl FnMut(AlarmLogRow),
    ) -> bool {
        let mut sql = SQL_SELECT_HEALTH_LOG.to_owned();
        if chart.is_some() {
            sql.push_str(" AND hl.chart = @chart ");
        }
        sql.push_str(" ORDER BY hld.unique_id DESC LIMIT @limit");

        let c = self.lock();
        let mut stmt = match c.prepare(&sql) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "sql_health_alarm_log2json");
                netdata_log_error!("Failed to prepare statement SQL_SELECT_HEALTH_LOG");
                return false;
            }
        };
        let (host, limit) = (&host_id[..], i64::from(limit));
        let chart = chart.map(text);
        let mut params: Vec<&dyn ToSql> = vec![&host, &after];
        if let Some(chart) = &chart {
            params.push(chart);
        }
        params.push(&limit);
        let Ok(mut rows) = stmt.query(&params[..]) else {
            return true;
        };
        while let Some(row) = next_row(&mut rows, End::Finalize, "sql_health_alarm_log2json") {
            each(AlarmLogRow {
                unique_id: int(row, 0),
                alarm_id: int(row, 1),
                alarm_event_id: int(row, 2),
                config_hash_id: uuid_column(row, 3),
                updated_by_id: int(row, 4),
                updates_id: int(row, 5),
                when: int(row, 6),
                duration: int(row, 7),
                non_clear_duration: int(row, 8),
                flags: int(row, 9),
                exec_run_timestamp: int(row, 10),
                delay_up_to_timestamp: int(row, 11),
                name: bytes_or_null(row, 12),
                chart: bytes_or_null(row, 13),
                exec: bytes_or_null(row, 14),
                recipient: bytes_or_null(row, 15),
                source: bytes_or_null(row, 16),
                units: bytes_or_null(row, 17),
                info: bytes_or_null(row, 18),
                exec_code: int(row, 19) as i32,
                new_status: double(row, 20) as i32,
                old_status: double(row, 21) as i32,
                delay: int(row, 22) as i32,
                new_value: double_or_null(row, 23),
                old_value: double_or_null(row, 24),
                last_repeat: int(row, 25),
                classification: bytes_or_null(row, 26),
                component: bytes_or_null(row, 27),
                r#type: bytes_or_null(row, 28),
                chart_context: bytes_or_null(row, 29),
                transition_id: uuid_column(row, 30),
                summary: bytes_or_null(row, 31),
            });
        }
        true
    }

    /// The end of `cleanup_health_log()`: the alarms of hosts the table `host` no longer has, the entries of alarms
    /// that are gone, and their `alert_version` rows (`db_execute()`: retried while busy, each failure recorded).
    pub fn delete_orphan_health_rows(&self) {
        let c = self.lock();
        let markers = self.markers();
        let orphans =
            [SQL_DELETE_ORPHAN_HEALTH_LOG, SQL_DELETE_ORPHAN_HEALTH_LOG_DETAIL, SQL_DELETE_ORPHAN_ALERT_VERSION];
        for sql in orphans {
            let _ = conn::db_execute(&c, sql, &markers);
        }
    }

    /// `sql_alert_cleanup()` after the database is open (`-W sqlite-alert-cleanup`): for every host of the table
    /// `host`, the alarms of charts the table `chart` no longer has go from `health_log` (their entries stay until
    /// an hourly cleanup). `checking` is told each host (its GUID as text and its hostname, `unknown` for a NULL)
    /// before its delete. False when the walk failed: C's record is then `Failed to check host alerts`.
    pub fn alert_cleanup(&self, mut checking: impl FnMut(&[u8; 16], &str)) -> bool {
        let c = self.lock();
        let mut stmt = match c.prepare(SQL_HEALTH_CHECK_ALL_HOSTS) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "sql_alert_cleanup");
                return false;
            }
        };
        let Ok(mut rows) = stmt.query([]) else {
            return false;
        };
        loop {
            let row = match rows.next() {
                Ok(Some(row)) => row,
                Ok(None) => return true,
                Err(err) => {
                    End::Finalize.failed(conn::result_code(&err), "sql_alert_cleanup");
                    return false;
                }
            };
            let Uuid::Valid(host_id) = uuid_column(row, 0) else {
                netdata_log_error!("Alert cleanup: skipping host with invalid host_id");
                continue;
            };
            let hostname = bytes_or_null(row, 1).map(|name| String::from_utf8_lossy(&name).into_owned());
            checking(&host_id, hostname.as_deref().unwrap_or("unknown"));
            // sql_remove_alerts_from_deleted_charts()
            let function = "sql_remove_alerts_from_deleted_charts";
            let params: &[(&str, &dyn ToSql)] = &[("@host_id", &&host_id[..])];
            match c.prepare(SQL_DELETE_MISSING_CHART_ALERT) {
                Ok(mut delete) => {
                    if let Err(err) = conn::retry(|| delete.execute(params)) {
                        netdata_log_error!("Failed to execute command to delete missing charts from health_log");
                        End::Finalize.failed(conn::result_code(&err), function);
                    }
                }
                Err(err) => prepare_failed(&err, function),
            }
        }
    }
}

impl MetaDb {
    /// `sql_get_alert_configuration()` for one hash: the rule's row, `Ok(None)` when `alert_hash` has none (also
    /// for a row whose hash is no 16-byte blob, which C skips with a record), `Err` when the statement cannot be
    /// prepared.
    #[allow(clippy::result_unit_err)]
    pub fn alert_config(&self, hash_id: &[u8; 16]) -> Result<Option<AlertConfigRow>, ()> {
        let mut found = None;
        // one hash, one row: the last one stands, as each row reaches C's callback
        self.alert_configs(std::slice::from_ref(hash_id), |row| found = Some(row))?;
        Ok(found)
    }

    /// `sql_get_alert_configuration()` for several hashes: the hashes go into a temporary table, in the order
    /// given, which is joined to `alert_hash` without an order, under one hold of the connection. The rows come as
    /// SQLite's plan gives them, as C's do (the same SQLite and statistics; decision D242 in the status repository):
    /// on a fresh database in the order given; once a stop's `PRAGMA optimize` has written that `alert_hash` holds
    /// one or two rows, in that table's row order. A hash without a row gives nothing; a row whose hash is no
    /// 16-byte blob is skipped, and one record counts them. `Err` when the table cannot be made or a statement
    /// cannot be prepared (reported); the table is dropped either way, unless it was never made.
    #[allow(clippy::result_unit_err)]
    pub fn alert_configs(&self, hashes: &[[u8; 16]], mut each: impl FnMut(AlertConfigRow)) -> Result<(), ()> {
        let c = self.lock();
        let markers = self.markers();
        let table = format!("c_{}", CONFIG_LISTS.fetch_add(1, Ordering::Relaxed));
        let create = format!("CREATE TEMP TABLE IF NOT EXISTS {table} (hash_id blob)");
        conn::db_execute(&c, &create, &markers).map_err(|_| ())?;
        // (the statement is gone before the table is dropped)
        let read = fill_temp_list(&c, &table, "hash_id", hashes)
            && match c.prepare(&sql_search_config_list(&table)) {
                Ok(mut stmt) => {
                    let mut invalid = 0usize;
                    if let Ok(mut rows) = stmt.query([]) {
                        while let Some(row) = next_row(&mut rows, End::Finalize, "sql_get_alert_configuration") {
                            match alert_config_row(row) {
                                Some(config) => each(config),
                                None => invalid += 1,
                            }
                        }
                    }
                    if invalid != 0 {
                        netdata_log_error!(
                            "HEALTH: Ignored {invalid} alert configuration rows with invalid config_hash_id."
                        );
                    }
                    true
                }
                Err(_) => {
                    netdata_log_error!("Failed to prepare statement sql_get_alert_configuration");
                    false
                }
            };
        let _ = conn::db_execute(&c, &format!("DROP TABLE IF EXISTS {table}"), &markers);
        if read { Ok(()) } else { Err(()) }
    }

    /// `sql_alert_transitions()`: the log entries asked for, each handed to `each` as C hands it to its callback,
    /// under one hold of the connection (decision D234 F8), so `each` must not ask the database. An entry shows
    /// the rule its alarm points at now, and is absent when `alert_hash` has no row for that rule. A row whose
    /// host id, rule hash or transition id is no 16-byte blob is skipped, and one record counts them.
    ///
    /// By id: nothing when the statement cannot be prepared (reported). By window: the hosts go into a temporary
    /// table of the connection, which is dropped at the end; nothing when the table cannot be made, or when one
    /// of the two statements cannot be prepared (each reported).
    pub fn alert_transitions(&self, of: &TransitionsOf<'_>, mut each: impl FnMut(&TransitionRow<'_>)) {
        const FUNCTION: &str = "sql_alert_transitions";
        let c = self.lock();
        let mut invalid = [0usize; 3];
        let mut hand = |row: &Row<'_>| {
            match transition_row(row) {
                Ok(transition) => each(&transition),
                Err(bad) => invalid[bad as usize] += 1,
            }
            true
        };
        match *of {
            TransitionsOf::Id(id) => {
                let params: [&dyn ToSql; 1] = [&&id[..]];
                rows(&c, SQL_SEARCH_ALERT_TRANSITION_DIRECT, FUNCTION, &params, &mut hand);
            }
            TransitionsOf::Window { hosts, after_s, before_s, context, alert_name } => {
                let markers = self.markers();
                let table = format!("v_{}", HOST_LISTS.fetch_add(1, Ordering::Relaxed));
                let create = format!("CREATE TEMP TABLE IF NOT EXISTS {table} (host_id blob)");
                if conn::db_execute(&c, &create, &markers).is_err() {
                    return;
                }
                if fill_temp_list(&c, &table, "host_id", hosts) {
                    let sql = sql_search_alert_transition(&table, context.is_some(), alert_name.is_some());
                    // (the statement is gone before the table is dropped)
                    match c.prepare(&sql) {
                        Ok(mut stmt) => {
                            let after = after_s.wrapping_mul(USEC_PER_SEC);
                            let before = before_s.wrapping_mul(USEC_PER_SEC);
                            let texts = [context.map(text), alert_name.map(text)];
                            let mut params: Vec<&dyn ToSql> = vec![&after, &before];
                            params.extend(texts.iter().flatten().map(|bound| bound as &dyn ToSql));
                            statement_rows(&mut stmt, FUNCTION, &params, End::Finalize, &mut hand);
                        }
                        Err(_) => netdata_log_error!("Failed to prepare statement sql_alert_transitions"),
                    }
                }
                let _ = conn::db_execute(&c, &format!("DROP TABLE IF EXISTS {table}"), &markers);
            }
        }
        if invalid.iter().any(|&count| count != 0) {
            let [hosts, hashes, transitions] = invalid;
            netdata_log_error!(
                "HEALTH: Ignored invalid alert transition rows (host_id={hosts}, config_hash_id={hashes}, \
                 transition_id={transitions})."
            );
        }
    }

    /// `process_alert_pending_queue()`'s statements: every row of the host's `alert_queue` that is due at `now`,
    /// in rowid order, leaves it; with `has_config` (the host has its ACLK sync configuration) it first goes
    /// toward the Cloud's queue (`insert_alert_to_submit_queue()`): not when `alert_version` already holds that
    /// status for the alarm (the version row then takes the entry's unique id; also when its statement cannot be
    /// prepared), not when the entry's rule is a variable (no warning, no critical expression), else the alarm's
    /// row of `aclk_queue` is made or pointed at the entry. Returns how many rows were processed and how many were
    /// queued; `None` when the walk's statement cannot be prepared. C's NOTICE is the caller's. `health_thread`:
    /// the caller is HEALTH, which keeps these statements compiled; it shows in the record of a step that failed.
    pub fn process_alert_pending_queue(
        &self,
        host_id: &[u8; 16],
        has_config: bool,
        now: i64,
        health_thread: bool,
    ) -> Option<(u32, u32)> {
        let end = End::of_health_statement(health_thread);
        let c = self.lock();
        let mut due: Vec<(i64, u32, i32, i64)> = Vec::new();
        let params: &[(&str, &dyn ToSql)] = &[("@host_id", &&host_id[..]), ("@now", &now)];
        let mut stmt = match c.prepare(SQL_PROCESS_ALERT_PENDING_QUEUE) {
            Ok(stmt) => stmt,
            Err(err) => {
                prepare_failed(&err, "process_alert_pending_queue");
                return None;
            }
        };
        if let Ok(mut rows) = stmt.query(params) {
            while let Some(row) = next_row(&mut rows, end, "process_alert_pending_queue") {
                due.push((int(row, 0), int(row, 1) as u32, double(row, 2) as i32, int(row, 3)));
            }
        }
        drop(stmt);

        let (mut count, mut added) = (0, 0);
        for (health_log_id, unique_id, status, rowid) in due {
            let queued = || insert_alert_to_submit_queue(&c, end, host_id, health_log_id, unique_id, status, now);
            if has_config && queued() == 0 {
                added += 1;
            }
            // delete_alert_from_pending_queue()
            let processed: [&dyn ToSql; 3] = [&&host_id[..], &rowid, &i64::from(unique_id)];
            let function = "delete_alert_from_pending_queue";
            if let Err(Step::Failed(rc)) = execute(&c, SQL_DELETE_PROCESSED_ROWS, function, &processed) {
                netdata_log_error!("Failed to delete processed rows, rc = {rc}");
                end.failed(rc, function);
            }
            count += 1;
        }
        Some((count, added))
    }

    /// `calculate_node_alert_version()`: the sum of the versions of the host's alarms the Cloud was told of and
    /// that are not REMOVED; 0 without any.
    pub fn node_alert_version(&self, host_id: &[u8; 16]) -> u64 {
        let c = self.lock();
        let mut version = 0u64;
        let host: [&dyn ToSql; 1] = [&&host_id[..]];
        rows(&c, SQL_ALERT_VERSION_CALC, "calculate_node_alert_version", &host, |row| {
            version = int(row, 0) as u64;
            true
        });
        version
    }
}

/// `insert_alert_to_submit_queue()`: 1 when the Cloud knows the status already, 2 for a variable's entry, 0 when
/// the entry was queued (also when the insert's step failed), -1 when the insert cannot be prepared. `end`: how the
/// calling thread lets go of these statements.
fn insert_alert_to_submit_queue(
    c: &Connection,
    end: End,
    host_id: &[u8; 16],
    health_log_id: i64,
    unique_id: u32,
    status: i32,
    now: i64,
) -> i32 {
    // cloud_status_matches(): true when its statement cannot be prepared
    let matches = match c.prepare(SQL_SELECT_LAST_ALERT_STATUS) {
        Ok(mut stmt) => {
            let known = stmt.query_row([health_log_id], |row| Ok(double(row, 0) as i32));
            // no row is no failure
            if let Err(err @ rusqlite::Error::SqliteFailure(..)) = &known {
                end.failed(conn::result_code(err), "cloud_status_matches");
            }
            known.is_ok_and(|known| known == status)
        }
        Err(err) => {
            prepare_failed(&err, "cloud_status_matches");
            true
        }
    };
    if matches {
        // update_alert_version_transition()
        let params: [&dyn ToSql; 2] = [&i64::from(unique_id), &health_log_id];
        let function = "update_alert_version_transition";
        if let Err(Step::Failed(rc)) = execute(c, SQL_UPDATE_ALERT_VERSION_TRANSITION, function, &params) {
            netdata_log_error!("Failed to update alert_version to latest transition");
            end.failed(rc, function);
        }
        return 1;
    }

    // is_event_from_alert_variable_config(): false when its statement cannot be prepared
    let variable = match c.prepare(SQL_SELECT_VARIABLE_ALERT_BY_UNIQUE_ID) {
        Ok(mut stmt) => {
            let params: [&dyn ToSql; 2] = [&i64::from(unique_id), &&host_id[..]];
            stmt.exists(&params[..]).unwrap_or_else(|err| {
                end.failed(conn::result_code(&err), "is_event_from_alert_variable_config");
                false
            })
        }
        Err(err) => {
            prepare_failed(&err, "is_event_from_alert_variable_config");
            false
        }
    };
    if variable {
        return 2;
    }

    let params: [&dyn ToSql; 4] = [&&host_id[..], &health_log_id, &i64::from(unique_id), &now];
    match execute(c, SQL_QUEUE_ALERT_TO_CLOUD, "insert_alert_to_submit_queue", &params) {
        Ok(()) => 0,
        Err(Step::Prepare) => -1,
        Err(Step::Failed(rc)) => {
            netdata_log_error!("Failed to insert alert in the submit queue {unique_id}, rc = {rc}");
            end.failed(rc, "insert_alert_to_submit_queue");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::health::AlertHashRow;
    use crate::open::SqliteSettings;

    const HOST: [u8; 16] = [0x11; 16];
    const HASH: [u8; 16] = [0xaa; 16];
    const T: i64 = 1_700_000_000;

    fn db() -> (tempfile::TempDir, MetaDb) {
        let dir = tempfile::tempdir().unwrap();
        let meta = MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        (dir, meta)
    }

    /// The rows of `sql` in rowid order, each column as text (`NULL`, a number, a text, a blob's first byte in hex).
    fn dump(meta: &MetaDb, sql: &str) -> Vec<String> {
        let c = meta.lock();
        let mut stmt = c.prepare(sql).unwrap();
        let columns = stmt.column_count();
        let mut out = Vec::new();
        let mut rows = stmt.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            let values: Vec<String> = (0..columns)
                .map(|i| match row.get_ref(i).unwrap() {
                    ValueRef::Null => "NULL".to_owned(),
                    ValueRef::Integer(v) => v.to_string(),
                    ValueRef::Real(v) => format!("{v:?}"),
                    ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned(),
                    ValueRef::Blob(b) => format!("x{:02x}", b[0]),
                })
                .collect();
            out.push(values.join(" "));
        }
        out
    }

    /// `sql_find_alert_transition()` on a real table: the alert of every entry with the transition id, through the
    /// entry's log row: the host, the chart's context (none for a NULL, the text for an empty one) and the alarm id
    /// cut to 32 bits as C's `sqlite3_column_int()` cuts it. A row whose host id is not 16 bytes is reported and
    /// left out, an id no entry has gives nothing, and so does an entry whose log row is gone.
    #[test]
    fn a_transition_finds_its_alerts() {
        let (_dir, meta) = db();
        let (wanted, other) = ([0x7a_u8; 16], [0x7b_u8; 16]);
        let (host2, bad_host) = ([0x22_u8; 16], [0x33_u8; 5]);
        {
            let c = meta.lock();
            let log = "INSERT INTO health_log (health_log_id, host_id, alarm_id, name, chart, chart_context) VALUES \
                       (?1, ?2, ?3, 'a', 't.c', ?4)";
            let detail = "INSERT INTO health_log_detail (health_log_id, unique_id, alarm_id, transition_id) VALUES \
                          (?1, ?2, ?3, ?4)";
            let none: Option<&str> = None;
            c.execute(log, rusqlite::params![1, &HOST[..], 7, "ctx.a"]).unwrap();
            c.execute(log, rusqlite::params![2, &host2[..], 8, none]).unwrap();
            c.execute(log, rusqlite::params![3, &bad_host[..], 9, "ctx.bad"]).unwrap();
            c.execute(log, rusqlite::params![4, &HOST[..], 10, ""]).unwrap();
            // the detail's own alarm id is what is read; one beyond 32 bits is cut
            c.execute(detail, rusqlite::params![1, 100, 7, &wanted[..]]).unwrap();
            c.execute(detail, rusqlite::params![2, 101, 0x1_0000_0008_i64, &wanted[..]]).unwrap();
            c.execute(detail, rusqlite::params![3, 102, 9, &wanted[..]]).unwrap();
            c.execute(detail, rusqlite::params![4, 103, -1, &wanted[..]]).unwrap();
            c.execute(detail, rusqlite::params![1, 104, 7, &other[..]]).unwrap();
            // an entry whose log row does not exist
            c.execute(detail, rusqlite::params![99, 105, 11, &[0x7c_u8; 16][..]]).unwrap();
        }
        let alert = |host_id: [u8; 16], context: Option<&str>, alarm_id: i32| AlertOfTransition {
            host_id,
            context: context.map(|c| c.as_bytes().to_vec()),
            alarm_id,
        };
        // (the rows' order is the database's; C calls back in it)
        let (mut found, records) = netdata_agent_log::capture(|| meta.find_alert_transition(&wanted));
        found.sort_by_key(|alert| alert.alarm_id);
        let expected = [alert(HOST, Some(""), -1), alert(HOST, Some("ctx.a"), 7), alert(host2, None, 8)];
        assert_eq!(found, expected);
        let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
        assert_eq!(messages, ["HEALTH: Got invalid machine guid while looking up alert transition. Ignoring it."]);
        assert_eq!(meta.find_alert_transition(&other), [alert(HOST, Some("ctx.a"), 7)]);
        assert!(meta.find_alert_transition(&[0x7c; 16]).is_empty());
        assert!(meta.find_alert_transition(&[0; 16]).is_empty());
    }

    /// The log of the transition reads. Two hosts and five alarms: `a_one` (HOST, rule HASH, every text set), with
    /// entries at T+10 (to WARNING), T+20 (back to CLEAR), T+30 (UNINITIALIZED to CLEAR: no status above RAISED)
    /// and T+60 (a transition id of 5 bytes); `a_two` (the second host, a rule without class, type and component,
    /// every text NULL) at T+40; `a_orphan` (HOST, a rule `alert_hash` has no row for) at T+45; `a_bad_host` (a
    /// host id of 5 bytes) at T+47; `a_bad_hash` (HOST, a rule hash of 3 bytes, which `alert_hash` has) at T+50.
    /// The entries of `a_bad_host` and `a_bad_hash` carry the transition id of `a_one`'s first entry.
    fn transitions_log(meta: &MetaDb) {
        let c = meta.lock();
        let (host2, hash2, bad_host, bad_hash) = ([0x22_u8; 16], [0xbb_u8; 16], [0x33_u8; 5], [1_u8, 2, 3]);
        let rule = "INSERT INTO alert_hash (hash_id, class, type, component) VALUES (?1, ?2, ?3, ?4)";
        let none: Option<&str> = None;
        c.execute(rule, rusqlite::params![&HASH[..], "Errors", "System", "Disk"]).unwrap();
        c.execute(rule, rusqlite::params![&hash2[..], none, none, none]).unwrap();
        c.execute(rule, rusqlite::params![&bad_hash[..], none, none, none]).unwrap();
        let log = "INSERT INTO health_log (health_log_id, host_id, alarm_id, config_hash_id, name, chart, \
                   chart_name, family, recipient, units, exec, chart_context) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, \
                   ?8, ?9, ?10, ?11, ?12)";
        c.execute(
            log,
            rusqlite::params![
                1, &HOST[..], 7, &HASH[..], "a_one", "t.c", "t.c_name", "fam", "sysadmin", "%", "/bin/x", "ctx.a"
            ],
        )
        .unwrap();
        c.execute(
            log,
            rusqlite::params![2, &host2[..], 8, &hash2[..], none, none, none, none, none, none, none, "ctx.b"],
        )
        .unwrap();
        let rest = |id: i32, host: &[u8], alarm: i32, hash: &[u8], name: &str| {
            let row = rusqlite::params![id, host, alarm, hash, name, "t.c", none, none, none, none, none, "ctx.a"];
            c.execute(log, row).unwrap();
        };
        rest(3, &HOST, 9, &[0xcc; 16], "a_orphan");
        rest(4, &bad_host, 10, &HASH, "a_bad_host");
        rest(5, &HOST, 11, &bad_hash, "a_bad_hash");
        let detail = "INSERT INTO health_log_detail (health_log_id, unique_id, new_status, old_status, \
                      transition_id, global_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)";
        let entry = |log_id: i32, unique_id: i32, new: f64, old: f64, transition: &[u8], second: i64| {
            let global_id = (T + second) * USEC_PER_SEC;
            c.execute(detail, rusqlite::params![log_id, unique_id, new, old, transition, global_id]).unwrap();
        };
        entry(1, 100, 3.0, 1.0, &[0x71; 16], 10);
        entry(1, 101, 1.0, 3.0, &[0x72; 16], 20);
        entry(1, 102, 1.0, 0.0, &[0x73; 16], 30);
        entry(2, 103, 4.0, 3.0, &[0x74; 16], 40);
        entry(3, 104, 4.0, 1.0, &[0x75; 16], 45);
        entry(4, 105, 4.0, 1.0, &[0x71; 16], 47);
        entry(5, 106, 4.0, 1.0, &[0x71; 16], 50);
        entry(1, 107, 4.0, 3.0, &[0x76; 5], 60);
        // every column of a_one's first entry; an exit code and flags beyond 32 bits; no old value
        let filled = "UPDATE health_log_detail SET when_key = ?1, duration = 60, non_clear_duration = 30, flags = \
                      ?2, exec_run_timestamp = ?3, delay_up_to_timestamp = ?4, info = 'the info', exec_code = ?5, \
                      delay = 5, new_value = 91.5, last_repeat = ?6, summary = 'the summary' WHERE unique_id = 100";
        c.execute(filled, rusqlite::params![T, 0x1_0000_0005_i64, T + 1, T + 2, 0x1_0000_0002_i64, T + 3]).unwrap();
    }

    /// The seconds after T of the entries a transitions read hands out, in its order, and the records it makes.
    fn transitions(meta: &MetaDb, of: &TransitionsOf<'_>) -> (Vec<i64>, Vec<String>) {
        let mut seconds = Vec::new();
        let ((), records) = netdata_agent_log::capture(|| {
            meta.alert_transitions(of, |row| seconds.push(row.global_id / USEC_PER_SEC - T));
        });
        (seconds, records.into_iter().filter_map(|record| record.message).collect())
    }

    /// `sql_alert_transitions()` by transition id: every entry with the id, whatever its host, its time and its
    /// statuses, through its alarm and its alarm's rule; each column as C reads it. A row whose host id or rule
    /// hash is no 16-byte blob is left out and counted in one record; an entry whose rule `alert_hash` does not
    /// have is absent; an id no entry has gives nothing and no record.
    #[test]
    fn a_transition_s_entries_are_read_by_its_id() {
        let (_dir, meta) = db();
        transitions_log(&meta);
        let mut found = Vec::new();
        let ((), records) = netdata_agent_log::capture(|| {
            meta.alert_transitions(&TransitionsOf::Id(&[0x71; 16]), |row| found.push(format!("{row:?}")));
        });
        let text = |text: &'static str| Some(Cow::Borrowed(text.as_bytes()));
        let expected = TransitionRow {
            host_id: HOST,
            alarm_id: 7,
            config_hash_id: HASH,
            alert_name: text("a_one"),
            chart: text("t.c"),
            chart_name: text("t.c_name"),
            family: text("fam"),
            recipient: text("sysadmin"),
            units: text("%"),
            exec: text("/bin/x"),
            chart_context: text("ctx.a"),
            when_key: T,
            duration: 60,
            non_clear_duration: 30,
            flags: 0x1_0000_0005,
            delay_up_to_timestamp: T + 2,
            info: text("the info"),
            exec_code: 2,
            new_status: 3,
            old_status: 1,
            delay: 5,
            new_value: 91.5,
            old_value: 0.0,
            last_repeat: T + 3,
            transition_id: [0x71; 16],
            global_id: (T + 10) * USEC_PER_SEC,
            classification: text("Errors"),
            r#type: text("System"),
            component: text("Disk"),
            exec_run_timestamp: T + 1,
            summary: text("the summary"),
        };
        assert_eq!(found, [format!("{expected:?}")]);
        let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
        let ignored = "HEALTH: Ignored invalid alert transition rows (host_id=1, config_hash_id=1, transition_id=0).";
        assert_eq!(messages, [ignored]);

        // no status above RAISED on either side, and outside any window: found by its id all the same
        assert_eq!(transitions(&meta, &TransitionsOf::Id(&[0x73; 16])), (vec![30], vec![]));
        // the second host's, whose texts are all NULL and whose numbers are all 0
        let mut found = Vec::new();
        meta.alert_transitions(&TransitionsOf::Id(&[0x74; 16]), |row| found.push(format!("{row:?}")));
        let bare = TransitionRow {
            host_id: [0x22; 16],
            alarm_id: 8,
            config_hash_id: [0xbb; 16],
            alert_name: None,
            chart: None,
            chart_name: None,
            family: None,
            recipient: None,
            units: None,
            exec: None,
            chart_context: text("ctx.b"),
            when_key: 0,
            duration: 0,
            non_clear_duration: 0,
            flags: 0,
            delay_up_to_timestamp: 0,
            info: None,
            exec_code: 0,
            new_status: 4,
            old_status: 3,
            delay: 0,
            new_value: 0.0,
            old_value: 0.0,
            last_repeat: 0,
            transition_id: [0x74; 16],
            global_id: (T + 40) * USEC_PER_SEC,
            classification: None,
            r#type: None,
            component: None,
            exec_run_timestamp: 0,
            summary: None,
        };
        assert_eq!(found, [format!("{bare:?}")]);
        // a rule `alert_hash` has no row for; an id no entry has
        assert_eq!(transitions(&meta, &TransitionsOf::Id(&[0x75; 16])), (vec![], vec![]));
        assert_eq!(transitions(&meta, &TransitionsOf::Id(&[0x7f; 16])), (vec![], vec![]));
    }

    /// `sql_alert_transitions()` over a window: the entries of the hosts listed that changed from or to WARNING or
    /// CRITICAL, newest first, both ends of the window included, of one context and of one alert name when given
    /// (whole, with their case). A row whose rule hash or transition id is no 16-byte blob is left out and counted
    /// in one record. The host list is a temporary table that is gone after each read, so two reads do not mix.
    #[test]
    fn a_window_s_transitions_are_read_for_the_hosts_listed() {
        let (_dir, meta) = db();
        transitions_log(&meta);
        fn window(hosts: &[[u8; 16]], after_s: i64, before_s: i64) -> TransitionsOf<'_> {
            let (after_s, before_s) = (T + after_s, T + before_s);
            TransitionsOf::Window { hosts, after_s, before_s, context: None, alert_name: None }
        }
        fn narrowed<'a>(
            hosts: &'a [[u8; 16]],
            context: Option<&'a str>,
            alert_name: Option<&'a str>,
        ) -> TransitionsOf<'a> {
            TransitionsOf::Window {
                hosts,
                after_s: T,
                before_s: T + 40,
                context: context.map(str::as_bytes),
                alert_name: alert_name.map(str::as_bytes),
            }
        }
        let both = &[HOST, [0x22_u8; 16]][..];
        let (first, second, ghosts) = (&both[..1], &both[1..], &[[0x44_u8; 16]][..]);
        let bad = |hashes: u32, transitions: u32| {
            vec![format!(
                "HEALTH: Ignored invalid alert transition rows (host_id=0, config_hash_id={hashes}, \
                 transition_id={transitions})."
            )]
        };
        // both hosts, everything: the entry at T+30 has no status above RAISED, the one at T+45 no rule, the one
        // at T+47 no host of the list; T+50's rule hash and T+60's transition id are no UUIDs
        assert_eq!(transitions(&meta, &window(both, 0, 100)), (vec![40, 20, 10], bad(1, 1)));
        // one host each, a host without entries, no host
        assert_eq!(transitions(&meta, &window(first, 0, 100)), (vec![20, 10], bad(1, 1)));
        assert_eq!(transitions(&meta, &window(second, 0, 100)), (vec![40], vec![]));
        assert_eq!(transitions(&meta, &window(ghosts, 0, 100)), (vec![], vec![]));
        assert_eq!(transitions(&meta, &window(&[], 0, 100)), (vec![], vec![]));
        // both ends are included, to the second
        assert_eq!(transitions(&meta, &window(both, 10, 40)), (vec![40, 20, 10], vec![]));
        assert_eq!(transitions(&meta, &window(both, 11, 39)), (vec![20], vec![]));
        assert_eq!(transitions(&meta, &window(both, 20, 20)), (vec![20], vec![]));
        assert_eq!(transitions(&meta, &window(both, 21, 20)), (vec![], vec![]));
        // a context and an alert name, each whole and with its case
        assert_eq!(transitions(&meta, &narrowed(both, Some("ctx.b"), None)).0, [40]);
        assert_eq!(transitions(&meta, &narrowed(both, Some("ctx.a"), None)).0, [20, 10]);
        assert_eq!(transitions(&meta, &narrowed(both, Some("ctx"), None)).0, [0_i64; 0]);
        assert_eq!(transitions(&meta, &narrowed(both, Some("CTX.A"), None)).0, [0_i64; 0]);
        assert_eq!(transitions(&meta, &narrowed(both, None, Some("a_one"))).0, [20, 10]);
        assert_eq!(transitions(&meta, &narrowed(both, None, Some("a_"))).0, [0_i64; 0]);
        assert_eq!(transitions(&meta, &narrowed(both, None, Some("A_ONE"))).0, [0_i64; 0]);
        assert_eq!(transitions(&meta, &narrowed(both, Some("ctx.a"), Some("a_one"))).0, [20, 10]);
        assert_eq!(transitions(&meta, &narrowed(both, Some("ctx.b"), Some("a_one"))).0, [0_i64; 0]);
        // no host list is left behind
        assert!(dump(&meta, "SELECT name FROM sqlite_temp_master").is_empty());
    }

    /// `sql_get_alert_configuration()` for several hashes on a fresh database (no statistics: SQLite scans the
    /// hashes and seeks each rule): the rules in the order asked, a hash without a rule giving nothing, a hash asked
    /// twice giving its rule twice; no rule list is left behind.
    #[test]
    fn several_rules_are_read_in_the_order_asked() {
        let (_dir, meta) = db();
        let (first, second, missing) = ([0xa1_u8; 16], [0xa2_u8; 16], [0xa3_u8; 16]);
        {
            let c = meta.lock();
            let rule = "INSERT INTO alert_hash (hash_id, alarm, p_update_every) VALUES (?1, ?2, ?3)";
            c.execute(rule, rusqlite::params![&first[..], "one", 10]).unwrap();
            c.execute(rule, rusqlite::params![&second[..], "two", 20]).unwrap();
        }
        let read = |hashes: &[[u8; 16]]| {
            let mut rules = Vec::new();
            let seen = |row: AlertConfigRow| {
                rules.push((row.hash_id[0], String::from_utf8(row.alarm.unwrap()).unwrap(), row.update_every));
            };
            meta.alert_configs(hashes, seen).unwrap();
            rules
        };
        let (one, two) = ((0xa1, "one".to_owned(), 10), (0xa2, "two".to_owned(), 20));
        assert_eq!(read(&[second, missing, first]), [two.clone(), one.clone()]);
        assert_eq!(read(&[first, second, first]), [one.clone(), two, one.clone()]);
        assert!(read(&[missing]).is_empty());
        assert!(read(&[]).is_empty());
        // the single read is the same lookup
        assert_eq!(meta.alert_config(&first).unwrap().map(|row| row.update_every), Some(10));
        assert_eq!(meta.alert_config(&missing), Ok(None));
        assert!(dump(&meta, "SELECT name FROM sqlite_temp_master").is_empty());
    }

    /// C's join after a stop (decision D242): the stop's `PRAGMA optimize` writes that `alert_hash` holds two rows,
    /// and the next life's plan scans that table first, so the rules come in its row order, not the order asked
    /// (the oracle's order, and its alert transitions row `rules-order/rowid`).
    #[test]
    fn after_a_stop_two_rules_come_in_their_row_order() {
        let dir = tempfile::tempdir().unwrap();
        let open = || MetaDb::open(dir.path(), &SqliteSettings::default()).unwrap();
        let (aa, bb) = ([0xaa_u8; 16], [0xbb_u8; 16]);
        let read = |meta: &MetaDb| {
            let mut names = Vec::new();
            meta.alert_configs(&[aa, bb], |row| names.push(String::from_utf8(row.alarm.unwrap()).unwrap())).unwrap();
            names
        };
        let meta = open();
        {
            let c = meta.lock();
            let rule = "INSERT INTO alert_hash (hash_id, alarm) VALUES (?1, ?2)";
            c.execute(rule, rusqlite::params![&bb[..], "bb"]).unwrap();
            c.execute(rule, rusqlite::params![&aa[..], "aa"]).unwrap();
        }
        assert_eq!(read(&meta), ["aa", "bb"]);
        meta.close();
        let meta = open();
        let stat: String = meta
            .lock()
            .query_row("SELECT stat FROM sqlite_stat1 WHERE tbl = 'alert_hash'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(stat, "2 1");
        assert_eq!(read(&meta), ["bb", "aa"]);
        assert!(dump(&meta, "SELECT name FROM sqlite_temp_master").is_empty());
    }

    const DETAIL: &str = "SELECT health_log_id, unique_id, alarm_event_id, updated_by_id, updates_id, when_key, \
                          flags, new_status, old_status, new_value, old_value, transition_id FROM \
                          health_log_detail ORDER BY rowid";
    const LOG: &str = "SELECT health_log_id, alarm_id, name, chart, last_transition_id, chart_name FROM health_log";
    const QUEUE: &str = "SELECT health_log_id, unique_id, alarm_id, status, date_scheduled FROM alert_queue";

    /// An entry of alarm 7 on `t.c`: unique id `unique_id`, event `event`, from `old` to `new` at `T + at`, with
    /// the transition id whose bytes are all `unique_id`.
    fn entry<'a>(unique_id: u32, event: u32, old: i32, new: i32, at: i64, transition: &'a [u8; 16]) -> EntryRow<'a> {
        EntryRow {
            unique_id,
            alarm_id: 7,
            alarm_event_id: event,
            config_hash_id: &HASH,
            transition_id: transition,
            updated_by_id: 0,
            updates_id: unique_id.saturating_sub(1),
            when: T + at,
            duration: 3,
            non_clear_duration: 0,
            flags: 0x80,
            exec_run_timestamp: 0,
            delay_up_to_timestamp: T + at,
            name: Some(b"a"),
            chart: Some(b"t.c"),
            chart_context: Some(b"t.ctx"),
            chart_name: Some(b"t.c_name"),
            exec: None,
            recipient: Some(b"root"),
            units: Some(b"things"),
            info: None,
            summary: Some(b"s"),
            exec_code: 0,
            new_status: new,
            old_status: old,
            delay: 0,
            new_value: if new == 3 { 70.0 } else { f64::NAN },
            old_value: f64::NAN,
            last_repeat: 0,
            global_id: 5,
        }
    }

    /// The insert makes the alarm's row once and points it at each entry inserted; the entry's row has the flags
    /// as given (the saved mark is the caller's, after it); a NaN value is NULL; the queue's row follows the last
    /// insert and keeps the earliest due time; the update finds the row by unique id, alarm id and transition id.
    #[test]
    fn an_entry_is_inserted_then_updated() {
        let (_dir, meta) = db();
        let (t1, t2) = ([1u8; 16], [2u8; 16]);
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, -2, 0, 0, &t1), false, true));
        assert_eq!(dump(&meta, LOG), ["1 7 a t.c x01 t.c_name"]);
        assert_eq!(dump(&meta, DETAIL), [format!("1 1 1 0 0 {T} 128 0.0 -2.0 NULL NULL x01")]);
        assert!(dump(&meta, QUEUE).is_empty(), "no ACLK sync configuration, no queue row");

        // REMOVED to UNINITIALIZED is due 600 s later; then UNINITIALIZED to WARNING at once, 5 s later: later
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, -2, 0, 0, &t1), true, true));
        assert_eq!(dump(&meta, QUEUE), [format!("1 1 7 0 {}", T + 600)]);
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(2, 2, 0, 3, 5, &t2), true, true));
        assert_eq!(dump(&meta, LOG), ["1 7 a t.c x02 t.c_name"]);
        assert_eq!(dump(&meta, QUEUE), [format!("1 2 7 3 {}", T + 5)]);
        let rows = dump(&meta, DETAIL);
        assert_eq!(rows.len(), 3, "no unique constraint: the first entry is there twice");
        assert_eq!(rows[2], format!("1 2 2 0 1 {} 128 3.0 0.0 70.0 NULL x02", T + 5));

        let mut replaced = entry(1, 1, -2, 0, 0, &t1);
        (replaced.updated_by_id, replaced.flags, replaced.exec_code) = (2, 0x1000_0082, 4);
        meta.health_alarm_log_update("h", &replaced, true);
        let rows = dump(&meta, DETAIL);
        assert_eq!(rows[0], format!("1 1 1 2 0 {T} 268435586 0.0 -2.0 NULL NULL x01"));
        assert_eq!(rows[1], rows[0]);
        // another transition id: no row
        let other = [9u8; 16];
        let mut stranger = entry(2, 2, 0, 3, 5, &other);
        stranger.updated_by_id = 99;
        meta.health_alarm_log_update("h", &stranger, true);
        assert_eq!(dump(&meta, DETAIL)[2], format!("1 2 2 0 1 {} 128 3.0 0.0 70.0 NULL x02", T + 5));
    }

    fn rule(meta: &MetaDb) {
        let row = AlertHashRow {
            hash_id: HASH,
            alarm: None,
            template: Some(b"a".to_vec()),
            on_key: Some(b"t.ctx".to_vec()),
            class: Some(b"Errors".to_vec()),
            component: None,
            r#type: None,
            update_every: 1,
            units: None,
            calc: None,
            warn: None,
            crit: None,
            exec: None,
            to_key: None,
            info: None,
            delay: b"multiplier 1.0 ".to_vec(),
            options: None,
            repeat: None,
            host_labels: None,
            lookup: None,
            source: Some(b"line=3,file=/etc/a.conf".to_vec()),
            chart_labels: None,
            summary: None,
            time_group_condition: 0,
            time_group_value: f64::NAN,
            dims_group: 0,
            data_source: 0,
        };
        assert!(meta.store_alert_config(&row));
    }

    /// Each REMOVED row a restart injects is a statement of its own in C, with its own read of the clock: two
    /// alarms get two times, and so two `global_id`s, which order and page the alert transitions.
    #[test]
    fn each_injected_row_reads_the_clock() {
        let (_dir, meta) = db();
        rule(&meta);
        let (t1, t2) = ([1u8; 16], [2u8; 16]);
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, 0, 3, 0, &t1), false, true));
        let other = EntryRow { alarm_id: 8, name: Some(b"b"), ..entry(2, 1, 0, 3, 0, &t2) };
        assert!(meta.health_alarm_log_insert("h", &HOST, &other, false, true));

        let mut clock = (T + 100) as u64 * 1_000_000;
        let mut now_usec = || {
            clock += 1_500_000;
            clock
        };
        let mut next = 0x30u8;
        let mut transition = || {
            next += 1;
            [next; 16]
        };
        meta.check_removed_alerts_state("h", &HOST, &|| true, false, true, &mut now_usec, &mut transition);
        let removed = "SELECT unique_id, when_key, global_id FROM health_log_detail WHERE new_status = -2";
        let injected = dump(&meta, removed);
        let first = (T + 101) as u64 * 1_000_000 + 500_000;
        assert_eq!(injected, [format!("3 {} {first}", T + 101), format!("4 {} {}", T + 103, first + 1_500_000)]);
    }

    /// At a restart the alarm whose last entry is not REMOVED gets a REMOVED row: the next unique id of the host,
    /// the next event id, the time given, the old row's flags; the old row is marked as replaced by it and the
    /// alarm points at it. The load then gives that row, with the rule's source and class. A second check finds
    /// nothing to do.
    #[test]
    fn a_restart_injects_a_removed_row_and_the_load_gives_the_last_entries() {
        let (_dir, meta) = db();
        rule(&meta);
        let (t1, t2) = ([1u8; 16], [2u8; 16]);
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, -2, 0, 0, &t1), false, true));
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(2, 2, 0, 3, 5, &t2), false, true));

        let mut next = 0x30u8;
        let mut transition = || {
            next += 1;
            [next; 16]
        };
        let at = |second: i64| second as u64 * 1_000_000 + 77;
        meta.check_removed_alerts_state("h", &HOST, &|| true, true, true, &mut || at(T + 100), &mut transition);
        let rows = dump(&meta, DETAIL);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1], format!("1 2 2 3 1 {} 130 3.0 0.0 70.0 NULL x02", T + 5), "UPDATED, by 3");
        assert_eq!(rows[2], format!("1 3 3 0 2 {} 128 -2.0 3.0 NULL 70.0 x31", T + 100));
        assert_eq!(dump(&meta, LOG), ["1 7 a t.c x31 t.c_name"]);
        // WARNING to REMOVED: due 10 s after the check
        assert_eq!(dump(&meta, QUEUE), [format!("1 3 7 -2 {}", T + 110)]);

        meta.check_removed_alerts_state("h", &HOST, &|| true, true, true, &mut || at(T + 200), &mut transition);
        assert_eq!(dump(&meta, DETAIL).len(), 3, "the last entry is REMOVED");

        let mut loaded = Vec::new();
        assert!(meta.load_health_log(&HOST, |row| {
            loaded.push(row);
            true
        }));
        assert_eq!(loaded.len(), 1);
        let row = &loaded[0];
        assert_eq!((row.unique_id, row.alarm_id, row.alarm_event_id, row.updates_id), (3, 7, 3, 2));
        assert_eq!((row.new_status, row.old_status, row.when, row.flags), (-2, 3, T + 100, 0x80));
        assert_eq!((row.new_value, row.old_value), (0.0, 70.0), "a NULL value reads as 0");
        assert_eq!((row.transition_id, row.config_hash_id), (Uuid::Valid([0x31; 16]), Uuid::Valid(HASH)));
        assert_eq!(row.source.as_deref(), Some(&b"line=3,file=/etc/a.conf"[..]));
        assert_eq!((row.classification.as_deref(), row.component.as_deref()), (Some(&b"Errors"[..]), None));
        assert_eq!((row.name.as_deref(), row.global_id), (Some(&b"a"[..]), Some(at(T + 100))));

        // another host has nothing; a rule that left alert_hash takes its alarm out of the load
        let mut other = 0;
        assert!(meta.load_health_log(&[0x22; 16], |_| {
            other += 1;
            true
        }));
        meta.lock().execute("DELETE FROM alert_hash", []).unwrap();
        assert!(meta.load_health_log(&HOST, |_| {
            other += 1;
            true
        }));
        assert_eq!(other, 0);
    }

    /// The alarm log's rows: newest first, above `after`, of one chart, at most the limit, with NULLs kept apart;
    /// an alarm whose rule is unknown has none.
    #[test]
    fn the_alarm_log_gives_the_entries_newest_first() {
        let (_dir, meta) = db();
        rule(&meta);
        let ids = [[1u8; 16], [2u8; 16], [3u8; 16]];
        for (i, new) in [0, 3, 1].into_iter().enumerate() {
            let row = entry(i as u32 + 1, i as u32 + 1, 0, new, 0, &ids[i]);
            assert!(meta.health_alarm_log_insert("h", &HOST, &row, false, true));
        }
        let listed = |after: i64, chart: Option<&[u8]>, limit: u32| {
            let mut rows = Vec::new();
            assert!(meta.alarm_log(&HOST, after, chart, limit, |row| rows.push(row)));
            rows
        };
        let ids_of = |rows: &[AlarmLogRow]| rows.iter().map(|row| row.unique_id).collect::<Vec<_>>();
        assert_eq!(ids_of(&listed(0, None, 10)), [3, 2, 1]);
        assert_eq!(ids_of(&listed(1, None, 10)), [3, 2], "above `after`");
        assert_eq!(ids_of(&listed(0, None, 2)), [3, 2]);
        assert!(listed(0, None, 0).is_empty(), "a host whose health never ran has a limit of 0");
        assert_eq!(ids_of(&listed(0, Some(b"t.c"), 10)), [3, 2, 1]);
        assert!(listed(0, Some(b"t.other"), 10).is_empty());

        let rows = listed(0, None, 10);
        let warning = &rows[1];
        assert_eq!((warning.new_value, warning.old_value), (Some(70.0), None));
        assert_eq!((warning.new_status, warning.old_status, warning.flags), (3, 0, 0x80));
        assert_eq!((warning.exec.as_deref(), warning.recipient.as_deref()), (None, Some(&b"root"[..])));
        assert_eq!(warning.source.as_deref(), Some(&b"line=3,file=/etc/a.conf"[..]));
        assert_eq!((warning.classification.as_deref(), warning.r#type.as_deref()), (Some(&b"Errors"[..]), None));
        assert_eq!((warning.config_hash_id, warning.transition_id), (Uuid::Valid(HASH), Uuid::Valid([2; 16])));
        assert_eq!((warning.info.as_deref(), warning.summary.as_deref()), (None, Some(&b"s"[..])));

        assert!(listed(0, None, 10).len() == 3 && meta.lock().execute("DELETE FROM alert_hash", []).is_ok());
        assert!(listed(0, None, 10).is_empty());
    }

    /// The hourly cleanup's last step drops the alarms of hosts the host table does not have, and their entries;
    /// the command-line cleanup drops the alarms of charts the chart table does not have, and leaves their entries.
    #[test]
    fn orphans_go_by_host_and_by_chart() {
        let (_dir, meta) = db();
        let t = [1u8; 16];
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, 0, 3, 0, &t), false, true));
        let counts = |meta: &MetaDb| {
            let log = dump(meta, "SELECT count(*) FROM health_log");
            let detail = dump(meta, "SELECT count(*) FROM health_log_detail");
            (log[0].clone(), detail[0].clone())
        };
        let host = |meta: &MetaDb| {
            let c = meta.lock();
            c.execute("INSERT INTO host (host_id, hostname) VALUES (?, 'known')", [&HOST[..]]).unwrap();
        };

        // the host is in the host table, its chart is not in the chart table
        host(&meta);
        meta.delete_orphan_health_rows();
        assert_eq!(counts(&meta), ("1".to_owned(), "1".to_owned()));
        let mut seen = Vec::new();
        assert!(meta.alert_cleanup(|id, hostname| seen.push((*id, hostname.to_owned()))));
        assert_eq!(seen, [(HOST, "known".to_owned())]);
        assert_eq!(counts(&meta), ("0".to_owned(), "1".to_owned()), "the entries stay");
        meta.delete_orphan_health_rows();
        assert_eq!(counts(&meta), ("0".to_owned(), "0".to_owned()));

        // an alarm of a host the host table does not have
        assert!(meta.health_alarm_log_insert("h", &[0x22; 16], &entry(1, 1, 0, 3, 0, &t), false, true));
        meta.delete_orphan_health_rows();
        assert_eq!(counts(&meta), ("0".to_owned(), "0".to_owned()));
    }

    /// A rule's row by its hash, and nothing for a hash the table does not have.
    #[test]
    fn a_rule_is_found_by_its_hash() {
        let (_dir, meta) = db();
        rule(&meta);
        let row = meta.alert_config(&HASH).unwrap().expect("the rule");
        assert_eq!((row.hash_id, row.alarm, row.template.as_deref()), (HASH, None, Some(&b"a"[..])));
        assert_eq!((row.on_key.as_deref(), row.classification.as_deref()), (Some(&b"t.ctx"[..]), Some(&b"Errors"[..])));
        assert_eq!((row.update_every, row.db_after, row.every.as_deref()), (1, 0, Some(&b"1"[..])));
        assert_eq!((row.green, row.warn, row.db_method), (None, None, None));
        assert_eq!(row.delay.as_deref(), Some(&b"multiplier 1.0 "[..]));
        assert_eq!(meta.alert_config(&[0x55; 16]), Ok(None));
    }

    /// The pending queue: a row that is due leaves `alert_queue`; with the host's configuration it goes to
    /// `aclk_queue`, unless the Cloud holds that status for the alarm already (the version row then takes the
    /// entry's unique id) or the rule is a variable; without the configuration it is lost; a row that is not due
    /// stays.
    #[test]
    fn a_due_row_leaves_the_queue_for_the_cloud_s() {
        let (_dir, meta) = db();
        rule(&meta);
        let t = [1u8; 16];
        let queue = |meta: &MetaDb| dump(meta, QUEUE).len();
        let cloud = |meta: &MetaDb| dump(meta, "SELECT health_log_id, unique_id, date_created FROM aclk_queue");
        let exec = |meta: &MetaDb, sql: &str| {
            meta.lock().execute(sql, []).unwrap();
        };

        // WARNING at T: due at once. The rule has no warning and no critical expression: a variable, not queued
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, 0, 3, 0, &t), true, true));
        assert_eq!(meta.process_alert_pending_queue(&HOST, true, T - 1, true), Some((0, 0)), "not due yet");
        assert_eq!(queue(&meta), 1);
        assert_eq!(meta.process_alert_pending_queue(&HOST, true, T, true), Some((1, 0)));
        assert_eq!((queue(&meta), cloud(&meta).len()), (0, 0));

        // with a warning expression the entry is queued, the alarm's row pointed at the newest
        exec(&meta, "UPDATE alert_hash SET warn = '$this > 1'");
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, 0, 3, 0, &t), true, true));
        assert_eq!(meta.process_alert_pending_queue(&HOST, true, T + 7, true), Some((1, 1)));
        assert_eq!(cloud(&meta), [format!("1 1 {}", T + 7)]);
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(2, 2, 3, 4, 0, &t), true, true));
        assert_eq!(meta.process_alert_pending_queue(&HOST, true, T + 9, true), Some((1, 1)));
        assert_eq!(cloud(&meta), [format!("1 2 {}", T + 9)]);

        // the Cloud holds CRITICAL (4) for the alarm: nothing is queued, its version row takes the unique id
        exec(&meta, "INSERT INTO alert_version (health_log_id, unique_id, status, version) VALUES (1, 2, 4, 50)");
        exec(&meta, "DELETE FROM aclk_queue");
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(3, 3, 3, 4, 0, &t), true, true));
        assert_eq!(meta.process_alert_pending_queue(&HOST, true, T + 9, true), Some((1, 0)));
        assert!(cloud(&meta).is_empty());
        assert_eq!(dump(&meta, "SELECT unique_id, status FROM alert_version"), ["3 4"]);
        assert_eq!(meta.node_alert_version(&HOST), 50);
        assert_eq!(meta.node_alert_version(&[0x22; 16]), 0);
        // an alarm the Cloud was told is REMOVED (-2) does not count
        exec(&meta, "UPDATE alert_version SET status = -2");
        assert_eq!(meta.node_alert_version(&HOST), 0);
        exec(&meta, "UPDATE alert_version SET status = 4");

        // a host without the configuration: the due row is deleted and nothing is queued
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(4, 4, 4, 3, 0, &t), true, true));
        assert_eq!(meta.process_alert_pending_queue(&HOST, false, T + 9, true), Some((1, 0)));
        assert_eq!((queue(&meta), cloud(&meta).len()), (0, 0));
    }

    /// The alarm id of a chart and a name, whatever the rule's hash, with the next event id; nothing for a chart
    /// or a name the table does not have.
    /// The newest row of the alarm that has the run mark answers, the asking entry's own row aside; rows of another
    /// alarm or another host do not count; C binds the ids as ints.
    #[test]
    fn the_last_executed_event_is_the_alarm_s_newest_row_with_the_run_mark() {
        let (_dir, meta) = db();
        let insert = |unique_id: u32, alarm_id: u32, status: i32, flags: u32, host: &[u8; 16]| {
            let transition = unique_id.to_be_bytes().repeat(4);
            let transition: &[u8; 16] = transition[..].try_into().unwrap();
            let mut row = entry(unique_id, unique_id, 0, status, i64::from(unique_id), transition);
            (row.alarm_id, row.flags) = (alarm_id, flags);
            assert!(meta.health_alarm_log_insert("h", host, &row, false, true));
        };
        let asked = |alarm_id: u32, unique_id: u32| meta.get_last_executed_event(&HOST, alarm_id, unique_id, true);
        assert_eq!(asked(7, 1), Some(None), "an empty table");

        // not in the order of their ids: the newest is the highest id, not the last row
        insert(3, 7, 1, 0x0000_0005, &HOST);
        insert(1, 7, 3, 0x0000_0045, &HOST);
        insert(2, 7, 4, 0x0000_0001, &HOST);
        insert(4, 8, 4, 0x0000_0005, &HOST);
        insert(5, 7, 4, 0x0000_0005, &[0x22; 16]);
        // the newest executed row of alarm 7 on this host is entry 3 (CLEAR); entry 2 was not executed
        assert_eq!(asked(7, 9), Some(Some(1)));
        // the asking entry's own row does not count
        assert_eq!(asked(7, 3), Some(Some(3)));
        assert_eq!(asked(7, 1), Some(Some(1)));
        assert_eq!(asked(8, 9), Some(Some(4)));
        assert_eq!(asked(9, 9), Some(None), "an alarm without a row");
        assert_eq!(meta.get_last_executed_event(&[0x33; 16], 7, 9, true), Some(None), "a host without a row");

        // C binds the ids as ints: an entry whose id is above 2^31 does not find its own row to leave it out, and
        // an alarm id above 2^31 matches no row
        let high = 0x8000_0001u32;
        insert(high, 7, 2, 0x0000_0005, &HOST);
        assert_eq!(asked(7, high), Some(Some(2)));
        insert(6, high, 4, 0x0000_0005, &HOST);
        assert_eq!(asked(high, 9), Some(None));

        // a statement that cannot be prepared: C's record, and its -1
        meta.lock().execute_batch("ALTER TABLE health_log_detail RENAME TO gone").unwrap();
        let (answer, records) = netdata_agent_log::capture(|| asked(7, 9));
        assert_eq!(answer, None);
        let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
        assert_eq!(messages, ["Failed to prepare statement, rc=1 in sql_health_get_last_executed_event"]);
    }

    #[test]
    fn an_alarm_s_id_is_found_by_chart_and_name() {
        let (_dir, meta) = db();
        let (t1, t2) = ([1u8; 16], [2u8; 16]);
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(1, 1, -2, 0, 0, &t1), false, true));
        assert!(meta.health_alarm_log_insert("h", &HOST, &entry(2, 6, 0, 3, 5, &t2), false, true));
        assert_eq!(meta.get_alarm_id(&HOST, b"t.c", Some(b"a")), Some((7, 7)));
        assert_eq!(meta.get_alarm_id(&HOST, b"t.other", Some(b"a")), None);
        assert_eq!(meta.get_alarm_id(&HOST, b"t.c", Some(b"b")), None);
        assert_eq!(meta.get_alarm_id(&[0x22; 16], b"t.c", Some(b"a")), None);
    }

    /// The cleanup takes a row that is older than the retention and was replaced, unless its alarm points at it.
    #[test]
    fn the_cleanup_takes_old_replaced_rows() {
        let (_dir, meta) = db();
        let ids = [[1u8; 16], [2u8; 16], [3u8; 16]];
        for (i, at) in [0, 10, 20].into_iter().enumerate() {
            let mut row = entry(i as u32 + 1, i as u32 + 1, 0, 3, at, &ids[i]);
            row.updated_by_id = if i < 2 { i as u32 + 2 } else { 0 };
            assert!(meta.health_alarm_log_insert("h", &HOST, &row, false, true));
        }
        let left = |meta: &MetaDb| dump(meta, "SELECT unique_id FROM health_log_detail ORDER BY rowid");
        // at T+60 with 50 s: only the first is older (when < now - retention is strict)
        assert!(meta.health_alarm_log_cleanup(&HOST, 50, T + 50));
        assert_eq!(left(&meta), ["1", "2", "3"]);
        assert!(meta.health_alarm_log_cleanup(&HOST, 50, T + 51));
        assert_eq!(left(&meta), ["2", "3"]);
        // the last entry is never taken: nothing replaced it, and the alarm points at it
        assert!(meta.health_alarm_log_cleanup(&HOST, 0, T + 1000));
        assert_eq!(left(&meta), ["3"]);
        // a replaced row the alarm still points at stays
        meta.lock().execute("UPDATE health_log_detail SET updated_by_id = 9", []).unwrap();
        assert!(meta.health_alarm_log_cleanup(&HOST, 0, T + 1000));
        assert_eq!(left(&meta), ["3"]);
        // another host's cleanup leaves these rows
        assert!(meta.health_alarm_log_cleanup(&[0x22; 16], 0, T + 1000));
        assert_eq!(left(&meta), ["3"]);
    }
}
