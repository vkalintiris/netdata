//! The alert log's SQL on the health side (`sqlite_health.c`): an entry's save as HEALTH or the metadata thread
//! makes it, and what a host's first pass makes of a row the table has.

use netdata_agent_log::netdata_log_error;
use netdata_agent_metadata::health_log::{AlertConfigRow, EntryRow, LoadedRow, Uuid};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_text::c::c_str;
use netdata_agent_query::tables::options_to_json_array;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::parse::{str2ndd, uuid_parse_flexi};
use netdata_agent_text::print::print_uuid_lower;
use netdata_agent_text::units::format_value_and_unit;

use crate::Clock;
use crate::alert::Status;
use crate::alerts::HostAlerts;
use crate::entry::{Entry, entry_flags};
use crate::tables::{DataSource, DimsGrouping, GroupCondition};

/// `sql_health_alarm_log_save()`: an entry that was saved before is updated, another is inserted (with `queue`,
/// the alarm's row of the unclaimed queue too). True when a row was inserted: the caller marks the entry SAVED.
/// `health_thread`: the caller is HEALTH (C's `is_health_thread`), which shows in the record of a failed step.
pub fn save(
    meta: &MetaDb,
    hostname: &str,
    host_id: &[u8; 16],
    entry: &Entry,
    queue: bool,
    health_thread: bool,
) -> bool {
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
        meta.health_alarm_log_update(hostname, &row, health_thread);
        false
    } else {
        meta.health_alarm_log_insert(hostname, host_id, &row, queue, health_thread)
    }
}

/// `sql_health_alarm_log_cleanup()`: the host's entries older than its retention that a newer one replaced go from
/// the table (but for the one its alarm points at), then from memory. A host whose health never ran and that no
/// child attached to has a retention of 0, and no alerts here. When the statement cannot be prepared the memory
/// log is left alone, as in C.
pub fn cleanup(meta: &MetaDb, host: &Host, host_id: &[u8; 16], alerts: Option<&HostAlerts>, clock: Clock) {
    let retention_s = host.health_log_retention_s();
    if meta.health_alarm_log_cleanup(host_id, retention_s, clock())
        && let Some(alerts) = alerts
    {
        alerts.log_cleanup(retention_s, clock);
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
        owed_saves: 0,
    })
}

/// `rrdcalc_status2string()` of a number a table holds: one that is no status is recorded and reads `UNKNOWN`.
pub fn status_name(number: i32) -> &'static str {
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

/// What `/api/v2/alert_config` and `/api/v3/alert_config` answer for a hash.
#[derive(Debug, PartialEq, Eq)]
pub enum ConfigAnswer {
    /// 200: the rule's configuration.
    Found(Vec<u8>),
    /// 404, `Config is not found.`
    NotFound,
    /// 500, `Failed to execute SQL query.`
    Failed,
}

/// `contexts_v2_alert_config_to_json()`: the rule of that hash as its row of `alert_hash` has it. The hash's text
/// is read as C's flexible parser reads a UUID (any case, with or without dashes); a text that is no UUID finds no
/// rule (C asks the table with a buffer it never set). Without a database the query fails. `default_recipient` is
/// localhost's, for a rule that names none.
pub fn alert_config_json(meta: Option<&MetaDb>, hash: &[u8], default_recipient: &[u8]) -> ConfigAnswer {
    let Some(meta) = meta else {
        return ConfigAnswer::Failed;
    };
    let Some(hash_id) = uuid_parse_flexi(hash) else {
        return ConfigAnswer::NotFound;
    };
    let row = match meta.alert_config(&hash_id) {
        Ok(Some(row)) => row,
        Ok(None) => return ConfigAnswer::NotFound,
        Err(()) => return ConfigAnswer::Failed,
    };

    let mut wb = JsonWriter::new(JsonOptions::DEFAULT);
    alert_config_members(&mut wb, &row, default_recipient, ConfigOptions::default());
    wb.finalize();
    ConfigAnswer::Found(wb.into_bytes())
}

/// What `contexts_v2_alert_config_to_json_from_sql_alert_config_data()` takes from the request that asked for a
/// rule: all off for `alert_config`, the request's options for the `configurations` of `alert_transitions`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfigOptions {
    /// The lookup, the calculation and the status object are printed even when the rule has none.
    pub debug: bool,
    /// No `name`, no `green` and no `red`.
    pub mcp: bool,
    /// The lookup's two ends go through the RFC 3339 time writer.
    pub rfc3339: bool,
}

/// `contexts_v2_alert_config_to_json_from_sql_alert_config_data()`: a rule's members, as its row of `alert_hash`
/// has it, into the object the caller has open. `default_recipient` is localhost's, for a rule that names none.
pub fn alert_config_members(
    wb: &mut JsonWriter,
    row: &AlertConfigRow,
    default_recipient: &[u8],
    options: ConfigOptions,
) {
    if !options.mcp {
        wb.member_add_string_opt("name", column(&row.alarm));
    }
    wb.member_add_uuid_ptr("config_hash_id", Some(&row.hash_id));

    wb.member_add_object("selectors");
    let template = column(&row.template).filter(|template| !template.is_empty());
    wb.member_add_string("type", if template.is_some() { "template" } else { "alarm" });
    wb.member_add_string_opt("on", template.or(column(&row.on_key)));
    wb.member_add_string_opt("families", column(&row.families));
    wb.member_add_string_opt("host_labels", column(&row.host_labels));
    wb.member_add_string_opt("chart_labels", column(&row.chart_labels));
    wb.object_close();

    wb.member_add_object("value");
    wb.member_add_string_opt("units", column(&row.units));
    // C hands the 32-bit number to an unsigned 64-bit parameter
    wb.member_add_uint64("update_every", i64::from(row.update_every) as u64);
    if row.db_after != 0 || options.debug {
        wb.member_add_object("db");
        wb.member_add_time_t_formatted("after", i64::from(row.db_after), options.rfc3339);
        wb.member_add_time_t_formatted("before", i64::from(row.db_before), options.rfc3339);
        wb.member_add_string("time_group_condition", GroupCondition::name_of_id(row.time_group_condition as u8));
        wb.member_add_double("time_group_value", row.time_group_value);
        wb.member_add_string("dims_group", DimsGrouping::name_of_id(row.dims_group as u8));
        wb.member_add_string("data_source", DataSource::name_of_id(row.data_source as u8));
        wb.member_add_string_opt("method", column(&row.db_method));
        wb.member_add_string_opt("dimensions", column(&row.db_dimensions));
        options_to_json_array(wb, b"options", u64::from(row.db_options));
        wb.object_close();
    }
    let calc = column(&row.calc);
    if calc.is_some() || options.debug {
        wb.member_add_string_opt("calc", calc);
    }
    wb.object_close();

    let (warn, crit) = (column(&row.warn), column(&row.crit));
    if warn.is_some() || crit.is_some() || options.debug {
        wb.member_add_object("status");
        for (key, text) in [("green", &row.green), ("red", &row.red)] {
            let number = column(text).map_or(f64::NAN, |text| str2ndd(text).0);
            if !options.mcp && (!number.is_nan() || options.debug) {
                wb.member_add_double(key, number);
            }
        }
        for (key, expression) in [("warn", warn), ("crit", crit)] {
            if expression.is_some() || options.debug {
                wb.member_add_string_opt(key, expression);
            }
        }
        wb.object_close();
    }

    wb.member_add_object("notification");
    wb.member_add_string("type", "agent");
    wb.member_add_string_opt("exec", column(&row.exec));
    wb.member_add_string("to", column(&row.to_key).unwrap_or(c_str(default_recipient)));
    wb.member_add_string_opt("delay", column(&row.delay));
    wb.member_add_string_opt("repeat", column(&row.repeat));
    wb.member_add_string_opt("options", column(&row.options));
    wb.object_close();

    wb.member_add_string_opt("class", column(&row.classification));
    wb.member_add_string_opt("component", column(&row.component));
    wb.member_add_string_opt("type", column(&row.r#type));
    wb.member_add_string_opt("info", column(&row.info));
    wb.member_add_string_opt("summary", column(&row.summary));
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A rule with a name, units and an update frequency, and nothing else.
    fn rule() -> AlertConfigRow {
        AlertConfigRow {
            hash_id: [0xa1; 16],
            alarm: Some(b"the_rule".to_vec()),
            template: None,
            on_key: Some(b"system.cpu".to_vec()),
            classification: None,
            component: None,
            r#type: None,
            lookup: None,
            every: None,
            units: Some(b"%".to_vec()),
            calc: None,
            families: None,
            green: None,
            red: None,
            warn: None,
            crit: None,
            exec: None,
            to_key: None,
            info: None,
            delay: None,
            options: None,
            repeat: None,
            host_labels: None,
            db_dimensions: None,
            db_method: None,
            db_options: 0,
            db_after: 0,
            db_before: 0,
            update_every: 10,
            source: None,
            chart_labels: None,
            summary: None,
            time_group_condition: 0,
            time_group_value: 0.0,
            dims_group: 0,
            data_source: 0,
        }
    }

    /// What `write` prints into an open object, minified, without the object's braces.
    fn members(write: impl FnOnce(&mut JsonWriter)) -> String {
        let mut wb = JsonWriter::new(JsonOptions::MINIFY);
        write(&mut wb);
        wb.finalize();
        let text = String::from_utf8(wb.into_bytes()).unwrap();
        text[1..text.len() - 1].to_owned()
    }

    fn printed(row: &AlertConfigRow, options: ConfigOptions) -> String {
        members(|wb| alert_config_members(wb, row, b"sysadmin", options))
    }

    /// `contexts_v2_alert_config_to_json_from_sql_alert_config_data()` by the request's options. Plain: the
    /// lookup, the calculation and the status object only when the rule has them, `green` and `red` only when
    /// they are numbers. `debug`: all of them, a missing one as null. `mcp`: no `name`, no `green`, no `red`.
    /// `rfc3339`: the lookup's ends through the time writer, where an end of 0 is null.
    #[test]
    fn a_rule_is_written_by_the_request_s_options() {
        let (plain, debug) = (ConfigOptions::default(), ConfigOptions { debug: true, ..Default::default() });
        let mcp = ConfigOptions { mcp: true, ..Default::default() };
        let bare = rule();
        let name = r#""name":"the_rule","#;
        let value = r#""value":{"units":"%","update_every":10},"notification":{"#;
        let text = printed(&bare, plain);
        assert!(text.starts_with(&format!(r#"{name}"config_hash_id":"a1a1a1a1-"#)), "{text}");
        assert!(text.contains(value), "{text}");
        assert!(!text.contains(r#""status""#) && !text.contains(r#""db""#) && !text.contains(r#""calc""#), "{text}");
        assert!(text.contains(r#""notification":{"type":"agent","exec":null,"to":"sysadmin","#), "{text}");

        // `mcp` on the bare rule: the same without its name
        assert_eq!(printed(&bare, mcp), text[name.len()..]);

        // `debug` on the bare rule: the lookup with its zeros, a null calculation, the whole status object
        let text = printed(&bare, debug);
        assert!(text.contains(r#""update_every":10,"db":{"after":0,"before":0,"time_group_condition":"#), "{text}");
        let status = r#""calc":null},"status":{"green":null,"red":null,"warn":null,"crit":null},"notification":{"#;
        assert!(text.contains(status), "{text}");
        // with `mcp` too: neither of the two thresholds
        let text = printed(&bare, ConfigOptions { debug: true, mcp: true, rfc3339: false });
        assert!(text.contains(r#""calc":null},"status":{"warn":null,"crit":null},"notification":{"#), "{text}");
        assert!(text.starts_with(r#""config_hash_id":"#), "{text}");

        // a rule with a lookup, a calculation, one threshold and one expression
        let full = AlertConfigRow {
            db_after: -600,
            calc: Some(b"$this * 2".to_vec()),
            green: Some(b"80.5".to_vec()),
            warn: Some(b"$this > $green".to_vec()),
            ..rule()
        };
        let green = members(|wb| wb.member_add_double("green", 80.5));
        let text = printed(&full, plain);
        assert!(text.contains(r#""db":{"after":-600,"before":0,"time_group_condition":"#), "{text}");
        let status = format!(r#""calc":"$this * 2"}},"status":{{{green},"warn":"$this > $green"}},"notification":{{"#);
        assert!(text.contains(&status), "{text}");
        // `debug` adds what the rule lacks, as nulls: the other threshold, the other expression
        let text = printed(&full, debug);
        let status = format!(r#""status":{{{green},"red":null,"warn":"$this > $green","crit":null}},"#);
        assert!(text.contains(&status), "{text}");
        // `mcp` drops the thresholds and keeps the expression
        let text = printed(&full, mcp);
        assert!(text.contains(r#""calc":"$this * 2"},"status":{"warn":"$this > $green"},"notification":{"#), "{text}");
        // `rfc3339`: a relative end stays a number, an end of 0 is null
        let text = printed(&full, ConfigOptions { rfc3339: true, ..Default::default() });
        assert!(text.contains(r#""db":{"after":-600,"before":null,"time_group_condition":"#), "{text}");
    }

    /// The statuses a table can hold, by their numbers; a number that is no status reads `UNKNOWN`.
    #[test]
    fn a_stored_status_has_c_s_name() {
        let names: Vec<&str> = (-2..=4).map(status_name).collect();
        assert_eq!(names, ["REMOVED", "UNDEFINED", "UNINITIALIZED", "CLEAR", "RAISED", "WARNING", "CRITICAL"]);
        assert_eq!((status_name(5), status_name(-3)), ("UNKNOWN", "UNKNOWN"));
    }
}
