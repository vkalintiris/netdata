//! The alert log's SQL on the health side (`sqlite_health.c`): an entry's save as HEALTH or the metadata thread
//! makes it, and what a host's first pass makes of a row the table has.

use netdata_agent_log::netdata_log_error;
use netdata_agent_metadata::health_log::{EntryRow, LoadedRow, Uuid};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_text::c::c_str;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::print::print_uuid_lower;
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

/// `rrdcalc_status2string()` of a number a table holds: one that is no status is recorded and reads `UNKNOWN`.
fn status_name(number: i32) -> &'static str {
    match number {
        -2 => Status::Removed.name(),
        -1 => Status::Undefined.name(),
        0 => Status::Uninitialized.name(),
        1 => Status::Clear.name(),
        2 => Status::Raised.name(),
        3 => Status::Warning.name(),
        4 => Status::Critical.name(),
        _ => {
            netdata_log_error!("Unknown alarm status {number}");
            "UNKNOWN"
        }
    }
}

/// `health_edit_command_from_source()`: the command that opens a rule's file at its line, from the rule's source
/// text. The text is `line=<n>,file=<path>`, or in the old form `<n>@<path>` (any `@` makes it the old form). The
/// file is what follows the path's last `/`. A new-form text without a `,` after its line gives the whole text as
/// the line. A text of neither form gives no command.
pub fn edit_command_from_source(source: &[u8], user_config_dir: &[u8], registry_hostname: &[u8]) -> Vec<u8> {
    let source = c_str(source);
    let find = |needle: &[u8]| source.windows(needle.len()).position(|window| window == needle);
    // C cuts its copy of the text at `cut` and then prints from two places of it: each runs to the cut when it
    // starts before it, else to the text's end
    let from = |start: usize, cut: Option<usize>| match cut {
        Some(cut) if start <= cut => &source[start..cut],
        _ => &source[start..],
    };
    let (line, file) = match source.iter().position(|&byte| byte == b'@') {
        Some(at) => {
            let Some(slash) = source.iter().rposition(|&byte| byte == b'/') else {
                return Vec::new();
            };
            (from(0, Some(at)), from(slash + 1, Some(at)))
        }
        None => {
            let (Some(line), Some(file)) = (find(b"line="), find(b"file=/")) else {
                return Vec::new();
            };
            let (number, path) = (line + b"line=".len(), file + b"file=".len());
            let slash = path + source[path..].iter().rposition(|&byte| byte == b'/').unwrap_or(0);
            match source[number..].iter().position(|&byte| byte == b',') {
                Some(comma) => (from(number, Some(number + comma)), from(slash + 1, Some(number + comma))),
                None => (source, from(slash + 1, None)),
            }
        }
    };
    [b"sudo ", user_config_dir, b"/edit-config health.d/", file, b"=", line, b"=", registry_hostname].concat()
}

/// A text column as C prints it: up to its first NUL; `None` for a NULL.
fn column(column: &Option<Vec<u8>>) -> Option<&[u8]> {
    column.as_deref().map(c_str)
}

/// A text column that reads `Unknown` when it is NULL.
fn or_unknown(text: &Option<Vec<u8>>) -> Option<&[u8]> {
    Some(column(text).unwrap_or(b"Unknown"))
}

/// What the alert log's answer takes from outside the table: the host's name and timezone, the notification
/// defaults of its health configuration, and what an edit command names.
pub struct LogView<'a> {
    pub hostname: &'a [u8],
    pub utc_offset: i32,
    pub abbrev_timezone: &'a [u8],
    pub default_exec: &'a [u8],
    pub default_recipient: &'a [u8],
    pub user_config_dir: &'a [u8],
    /// Localhost's registry hostname.
    pub registry_hostname: &'a [u8],
}

/// `sql_health_alarm_log2json()`: the body of `/api/v1/alarm_log`: the host's entries above `after`, of `chart`
/// only when one is given, newest first, at most `limit` (the host's `in memory max health log entries`, 0 before
/// its first pass): a bare array, each entry with its alarm's and its rule's columns. An entry whose rule's hash
/// is no UUID, or whose transition id is neither one nor NULL, is recorded and left out. When the statement cannot
/// be prepared the body is empty.
pub fn alarm_log_json(
    meta: &MetaDb,
    host_id: &[u8; 16],
    view: &LogView<'_>,
    after: i64,
    chart: Option<&[u8]>,
    limit: u32,
) -> Vec<u8> {
    let mut rows = Vec::new();
    if !meta.alarm_log(host_id, after, chart, limit, |row| rows.push(row)) {
        return Vec::new();
    }
    let hostname = String::from_utf8_lossy(view.hostname);
    let uuid_text = |id: &[u8; 16]| {
        let mut text = Vec::with_capacity(36);
        print_uuid_lower(&mut text, id);
        text
    };
    let mut wb = JsonWriter::with_quotes(b"\"", b"\"", 0, false, JsonOptions::DEFAULT);
    wb.member_add_array(None);
    for row in rows {
        let Uuid::Valid(config_hash_id) = row.config_hash_id else {
            netdata_log_error!(
                "HEALTH [{hostname}]: Got invalid config hash id while exporting health log. Ignoring entry."
            );
            continue;
        };
        let transition_id = match row.transition_id {
            Uuid::Null => Vec::new(),
            Uuid::Valid(id) => uuid_text(&id),
            Uuid::Invalid => {
                netdata_log_error!(
                    "HEALTH [{hostname}]: Got invalid transition id while exporting health log. Ignoring entry."
                );
                continue;
            }
        };
        let flag = |bit: u32| row.flags & i64::from(bit) != 0;
        let source = column(&row.source);
        let command = match source {
            Some(source) if !source.is_empty() => {
                edit_command_from_source(source, view.user_config_dir, view.registry_hostname)
            }
            _ => b"UNKNOWN=0=UNKNOWN".to_vec(),
        };
        let units = column(&row.units);
        let value_string = |value: Option<f64>| match value {
            Some(value) => format_value_and_unit(value, units.unwrap_or(b"")),
            None => b"-".to_vec(),
        };

        wb.add_array_item_object();
        wb.member_add_string_or_empty("hostname", Some(view.hostname));
        wb.member_add_int64("utc_offset", i64::from(view.utc_offset));
        wb.member_add_string_or_empty("timezone", Some(view.abbrev_timezone));
        wb.member_add_int64("unique_id", row.unique_id);
        wb.member_add_int64("alarm_id", row.alarm_id);
        wb.member_add_int64("alarm_event_id", row.alarm_event_id);
        wb.member_add_string_or_empty("config_hash_id", Some(&uuid_text(&config_hash_id)));
        wb.member_add_string_or_empty("transition_id", Some(&transition_id));
        wb.member_add_string_or_empty("name", column(&row.name));
        wb.member_add_string_or_empty("chart", column(&row.chart));
        wb.member_add_string_or_empty("context", column(&row.chart_context));
        wb.member_add_string_or_empty("class", or_unknown(&row.classification));
        wb.member_add_string_or_empty("component", or_unknown(&row.component));
        wb.member_add_string_or_empty("type", or_unknown(&row.r#type));
        wb.member_add_boolean("processed", flag(entry_flags::PROCESSED));
        wb.member_add_boolean("updated", flag(entry_flags::UPDATED));
        wb.member_add_int64("exec_run", row.exec_run_timestamp);
        wb.member_add_boolean("exec_failed", flag(entry_flags::EXEC_FAILED));
        wb.member_add_string_or_empty("exec", Some(column(&row.exec).unwrap_or(view.default_exec)));
        wb.member_add_string_or_empty("recipient", Some(column(&row.recipient).unwrap_or(view.default_recipient)));
        wb.member_add_int64("exec_code", i64::from(row.exec_code));
        wb.member_add_string_or_empty("source", Some(source.unwrap_or(b"Unknown")));
        wb.member_add_string_or_empty("command", Some(&command));
        wb.member_add_string_or_empty("units", units);
        wb.member_add_int64("when", row.when);
        wb.member_add_int64("duration", row.duration);
        wb.member_add_int64("non_clear_duration", row.non_clear_duration);
        wb.member_add_string_or_empty("status", Some(status_name(row.new_status).as_bytes()));
        wb.member_add_string_or_empty("old_status", Some(status_name(row.old_status).as_bytes()));
        wb.member_add_int64("delay", i64::from(row.delay));
        wb.member_add_int64("delay_up_to_timestamp", row.delay_up_to_timestamp);
        // C reads the two ids as unsigned 32-bit numbers
        wb.member_add_int64("updated_by_id", i64::from(row.updated_by_id as u32));
        wb.member_add_int64("updates_id", i64::from(row.updates_id as u32));
        wb.member_add_string_or_empty("value_string", Some(&value_string(row.new_value)));
        wb.member_add_string_or_empty("old_value_string", Some(&value_string(row.old_value)));
        wb.member_add_int64("last_repeat", row.last_repeat);
        wb.member_add_boolean("silenced", flag(entry_flags::SILENCED));
        wb.member_add_string_or_empty("summary", column(&row.summary));
        wb.member_add_string_or_empty("info", column(&row.info));
        wb.member_add_boolean("no_clear_notification", flag(entry_flags::NO_CLEAR_NOTIFICATION));
        for (key, value) in [("value", row.new_value), ("old_value", row.old_value)] {
            match value {
                Some(value) => wb.member_add_double(key, value),
                None => wb.member_add_string_opt(key, None),
            }
        }
        wb.object_close();
    }
    wb.array_close();
    wb.finalize();
    wb.into_bytes()
}
