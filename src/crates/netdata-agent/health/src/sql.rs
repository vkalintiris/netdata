//! The alert log's SQL on the health side (`sqlite_health.c`): an entry's save as HEALTH or the metadata thread
//! makes it, and what a host's first pass makes of a row the table has.

use netdata_agent_metadata::health_log::{EntryRow, LoadedRow, Uuid};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_text::units::format_value_and_unit;

use crate::Clock;
use crate::alert::Status;
use crate::alerts::HostAlerts;
use crate::entry::{Entry, entry_flags};

/// `sql_health_alarm_log_save()`: an entry that was saved before is updated, another is inserted (with `queue`,
/// the alarm's row of the unclaimed queue too). True when a row was inserted: the caller marks the entry SAVED.
pub fn save(meta: &MetaDb, hostname: &str, host_id: &[u8; 16], entry: &Entry, queue: bool) -> bool {
    let row = EntryRow {
        unique_id: entry.unique_id,
        alarm_id: entry.alarm_id,
        alarm_event_id: entry.alarm_event_id,
        config_hash_id: &entry.config_hash_id,
        transition_id: &entry.transition_id,
        updated_by_id: entry.updated_by_id,
        updates_id: entry.updates_id,
        when: entry.when,
        duration: entry.duration,
        non_clear_duration: entry.non_clear_duration,
        flags: entry.flags,
        exec_run_timestamp: entry.exec_run_timestamp,
        delay_up_to_timestamp: entry.delay_up_to_timestamp,
        name: entry.name.as_deref(),
        chart: Some(&entry.chart),
        chart_context: Some(&entry.chart_context),
        chart_name: Some(&entry.chart_name),
        exec: entry.exec.as_deref(),
        recipient: entry.recipient.as_deref(),
        units: entry.units.as_deref(),
        info: entry.info.as_deref(),
        summary: entry.summary.as_deref(),
        exec_code: entry.exec_code,
        new_status: entry.new_status as i32,
        old_status: entry.old_status as i32,
        delay: entry.delay,
        new_value: entry.new_value,
        old_value: entry.old_value,
        last_repeat: entry.last_repeat,
        global_id: entry.global_id,
    };
    if entry.flags & entry_flags::SAVED != 0 {
        meta.health_alarm_log_update(hostname, &row);
        false
    } else {
        meta.health_alarm_log_insert(hostname, host_id, &row, queue)
    }
}

/// `sql_health_alarm_log_cleanup()`: the host's entries older than its retention that a newer one replaced go from
/// the table (but for the one its alarm points at), then from memory. A host health never ran for has no alerts
/// here and a retention of 0. When the statement cannot be prepared the memory log is left alone, as in C.
pub fn cleanup(meta: &MetaDb, host_id: &[u8; 16], alerts: Option<&HostAlerts>, clock: Clock) {
    let retention_s = alerts.map_or(0, HostAlerts::log_retention_s);
    if meta.health_alarm_log_cleanup(host_id, retention_s, clock())
        && let Some(alerts) = alerts
    {
        alerts.log_cleanup(clock);
    }
}

/// A text column as C keeps it: read up to its first NUL, and no text at all when it is empty
/// (`string_strdupz()`).
fn text(column: Option<Vec<u8>>) -> Option<Vec<u8>> {
    let mut text = column?;
    if let Some(end) = text.iter().position(|&byte| byte == 0) {
        text.truncate(end);
    }
    (!text.is_empty()).then_some(text)
}

/// A status as the table holds it. A number that is no status (C would carry it as it is) reads as
/// UNINITIALIZED: no agent writes one.
fn status(number: i32) -> Status {
    match number {
        -2 => Status::Removed,
        -1 => Status::Undefined,
        1 => Status::Clear,
        2 => Status::Raised,
        3 => Status::Warning,
        4 => Status::Critical,
        _ => Status::Uninitialized,
    }
}

/// `sql_health_alarm_log_load()`'s first look at a row, before it asks whether the row is a repeating alert's:
/// what C's record says is wrong with it.
pub(crate) fn row_lacks(row: &LoadedRow) -> Option<&'static str> {
    if row.unique_id == 0 {
        Some("Got invalid unique id. Ignoring it.")
    } else if row.alarm_id == 0 {
        Some("Got invalid alarm id. Ignoring it.")
    } else if row.name.is_none() {
        Some("Got null name field. Ignoring it.")
    } else if row.chart.is_none() {
        Some("Got null chart field. Ignoring it.")
    } else {
        None
    }
}

/// `sql_health_alarm_log_load()`'s entry of a row that passed `row_lacks()`: every column as saved, marked SAVED,
/// the two value strings made again from the values and the units. `Err` is what C's record says of a transition
/// id or a rule's hash that is no UUID.
pub(crate) fn entry_of(row: LoadedRow) -> Result<Entry, &'static str> {
    let uuid = |column: Uuid, wrong: &'static str| match column {
        Uuid::Null => Ok([0; 16]),
        Uuid::Valid(id) => Ok(id),
        Uuid::Invalid => Err(wrong),
    };
    let transition_id = uuid(row.transition_id, "Got invalid transition id. Ignoring entry.")?;
    let config_hash_id = uuid(row.config_hash_id, "Got invalid config hash id. Ignoring entry.")?;
    let units = text(row.units);
    let value_string = |value| format_value_and_unit(value, units.as_deref().unwrap_or(b""));
    Ok(Entry {
        unique_id: row.unique_id,
        alarm_id: row.alarm_id,
        alarm_event_id: row.alarm_event_id,
        global_id: row.global_id.unwrap_or(0),
        config_hash_id,
        transition_id,
        when: row.when,
        duration: row.duration,
        non_clear_duration: row.non_clear_duration,
        name: text(row.name),
        chart: text(row.chart).unwrap_or_default(),
        chart_context: text(row.chart_context).unwrap_or_default(),
        chart_name: text(row.chart_name).unwrap_or_default(),
        classification: text(row.classification),
        component: text(row.component),
        r#type: text(row.r#type),
        exec: text(row.exec),
        recipient: text(row.recipient),
        source: text(row.source),
        summary: text(row.summary),
        info: text(row.info),
        exec_run_timestamp: row.exec_run_timestamp,
        exec_code: row.exec_code,
        old_value: row.old_value,
        new_value: row.new_value,
        old_value_string: value_string(row.old_value),
        new_value_string: value_string(row.new_value),
        units,
        old_status: status(row.old_status),
        new_status: status(row.new_status),
        flags: row.flags | entry_flags::SAVED,
        delay: row.delay,
        delay_up_to_timestamp: row.delay_up_to_timestamp,
        updated_by_id: row.updated_by_id,
        updates_id: row.updates_id,
        last_repeat: row.last_repeat,
        pending_save_count: 0,
    })
}
