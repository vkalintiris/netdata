//! What the web API shows of a host's alerts and variables: the bodies of `/api/v1/alarms`, `/api/v1/alarms_values`
//! and `/api/v1/alarm_count` (`health_json.c`), the body of `/api/v1/alarm_variables`
//! (`health_api_v1_chart_variables2json()`), a chart's `alarms` member (`rrdset2json()`), and the counts of
//! `/api/v1/charts` and `/api/v1/info`.
//!
//! None of them looks at whether health is enabled: a host without alerts gives empty members and zero counts.

use std::sync::Arc;

use indexmap::IndexMap;
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use netdata_agent_query::tables::options_to_text;
use netdata_agent_text::print::{html_escape, print_fixed, print_netdata_double_or_null};
use netdata_agent_text::units::format_value_and_unit;

use crate::alert::{Alert, ExpressionText, Snapshot, Status, run_flags};
use crate::alerts::HostAlerts;
use crate::config::HealthConfig;
use crate::pass::PassCounts;
use crate::tables::ACTION_OPTION_NO_CLEAR_NOTIFICATION;

/// `rrdset2json()`'s `alarms` members: the chart's alerts in link order, each with its published status.
pub fn chart_alarms_json(w: &mut JsonWriter, alerts: Option<&HostAlerts>, chart: &Chart) {
    for alert in alerts.map(|alerts| alerts.chart_alerts(chart)).unwrap_or_default() {
        w.member_add_object(alert.name());
        w.member_add_string_or_empty("id", Some(alert.name()));
        w.member_add_string_or_empty("status", Some(alert.snapshot().status.name().as_bytes()));
        w.member_add_string_or_empty("units", alert.config.units.as_deref());
        w.member_add_int64("duration", i64::from(alert.config.update_every));
        w.object_close();
    }
}

/// `rrdvar_to_json_members()`: custom variables in insertion order, each a double.
pub fn variables_json(w: &mut JsonWriter, variables: &[(String, f64)]) {
    for (name, value) in variables {
        w.member_add_double(name, *value);
    }
}

/// `charts2json()`'s `alarms_count`: the host's alerts that have a chart, which every linked alert has.
pub fn linked_count(alerts: Option<&HostAlerts>) -> usize {
    alerts.map_or(0, HostAlerts::count)
}

/// `/api/v1/info`'s `alarms`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
    pub normal: u64,
    pub warning: u64,
    pub critical: u64,
}

/// The published status of each of the host's alerts whose chart was collected at least once: the walk that
/// `web_client_api_request_v1_info_summary_alarm_statuses()` and `rrdhost_status_health_internal()` both make (C's
/// also skip an alert without a chart; an alert here always has its chart).
fn collected_statuses(alerts: Option<&HostAlerts>) -> impl Iterator<Item = Status> {
    alerts
        .map(HostAlerts::alerts)
        .unwrap_or_default()
        .into_iter()
        .filter(|alert| alert.chart.collection().last_collected.0 != 0)
        .map(|alert| alert.snapshot().status)
}

/// `web_client_api_request_v1_info_summary_alarm_statuses()`: the alerts whose chart was collected at least once,
/// by published status. Every status that is not WARNING or CRITICAL counts as normal.
pub fn status_counts(alerts: Option<&HostAlerts>) -> StatusCounts {
    let mut counts = StatusCounts::default();
    for status in collected_statuses(alerts) {
        match status {
            Status::Warning => counts.warning += 1,
            Status::Critical => counts.critical += 1,
            _ => counts.normal += 1,
        }
    }
    counts
}

/// The alert counts of `rrdhost_status_health_internal()` (`rrdhost-status.c:323-361`), which a host's `health`
/// object prints: the same alerts, counted as the request finds them, each under its own status; REMOVED and any
/// other status count nowhere.
pub fn alert_counts(alerts: Option<&HostAlerts>) -> PassCounts {
    let mut counts = PassCounts::default();
    for status in collected_statuses(alerts) {
        counts.add(status);
    }
    counts
}

/// `health_alarms2json_fill_alarms()`'s walk: the host's alerts in the dictionary's order, each with its published
/// snapshot, that are linked to a chart that was collected, is not obsolete and has a dimension.
fn listed(host: &Host, alerts: Option<&HostAlerts>) -> Vec<(Arc<Alert>, Snapshot)> {
    let Some(alerts) = alerts else {
        return Vec::new();
    };
    let mut listed = Vec::new();
    for alert in alerts.alerts() {
        let chart = &alert.chart;
        if !alerts.is_linked(host, &alert)
            || chart.collection().last_collected.0 == 0
            || chart.flags() & netdata_agent_rrd::chart::flags::OBSOLETE != 0
            || chart.dim_count() == 0
        {
            continue;
        }
        let snapshot = alert.snapshot();
        listed.push((alert, snapshot));
    }
    listed
}

/// The alerts the two listings show: all of [`listed`], or only those raised (WARNING or CRITICAL).
fn selected(host: &Host, alerts: Option<&HostAlerts>, all: bool) -> Vec<(Arc<Alert>, Snapshot)> {
    let mut selected = listed(host, alerts);
    if !all {
        selected.retain(|(_, snapshot)| matches!(snapshot.status, Status::Warning | Status::Critical));
    }
    selected
}

/// `web_client_api_request_v1_alarms_select()`: the request's tokens between `&`; `all` or `all=true` selects every
/// alert, `active` or `active=true` the raised ones; the last of them decides, and raised is the default.
pub fn alarms_select(query: &[u8]) -> bool {
    let mut all = false;
    for token in query.split(|&c| c == b'&') {
        match token {
            b"all" | b"all=true" => all = true,
            b"active" | b"active=true" => all = false,
            _ => {}
        }
    }
    all
}

/// `health_string2json()`: a member whose text is HTML-escaped, or `null` for an empty one; no space after its
/// colon.
fn string2json(out: &mut Vec<u8>, label: &str, value: &[u8]) {
    out.extend_from_slice(b"\t\t\t\"");
    out.extend_from_slice(label.as_bytes());
    if value.is_empty() {
        out.extend_from_slice(b"\":null,\n");
    } else {
        out.extend_from_slice(b"\":\"");
        html_escape(out, value);
        out.extend_from_slice(b"\",\n");
    }
}

/// One member of an alarm on its line: a text between quotes, or a bare number or word; a space after the colon.
fn member(out: &mut Vec<u8>, key: &str, value: &[u8], quoted: bool) {
    out.extend_from_slice(b"\t\t\t\"");
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(if quoted { b"\": \"" } else { b"\": " });
    out.extend_from_slice(value);
    out.extend_from_slice(if quoted { b"\",\n" } else { b",\n" });
}

/// `health_rrdcalc2json_nolock()`: one alarm of `/api/v1/alarms`. C prints every text as it is: nothing is escaped
/// but the lookup's dimensions and the expressions.
fn alarm_json(out: &mut Vec<u8>, config: &HealthConfig, alert: &Alert, snapshot: &Snapshot) {
    let rule = &alert.config;
    let text = |value: &Option<Vec<u8>>| value.clone().unwrap_or_default();
    let or_unknown = |value: &Option<Vec<u8>>| value.clone().unwrap_or_else(|| b"Unknown".to_vec());
    let flag = |flag: u32| if snapshot.run_flags & flag != 0 { "true" } else { "false" };
    let (chart, name) = (alert.chart.id().as_bytes(), alert.name());
    let number = |value: u64| value.to_string().into_bytes();
    let signed = |value: i32| value.to_string().into_bytes();
    let mut multiplier = Vec::new();
    print_fixed(&mut multiplier, f64::from(rule.delay_multiplier), 6);
    let mut hash = Vec::new();
    netdata_agent_text::print::print_uuid_lower(&mut hash, &rule.hash_id);
    let units = text(&rule.units);

    member(out, "id", &number(u64::from(alert.id)), false);
    member(out, "config_hash_id", &hash, true);
    member(out, "name", name, true);
    member(out, "chart", chart, true);
    member(out, "class", &or_unknown(&rule.classification), true);
    member(out, "component", &or_unknown(&rule.component), true);
    member(out, "type", &or_unknown(&rule.r#type), true);
    // a listed alert has its chart
    member(out, "active", b"true", false);
    member(out, "disabled", flag(run_flags::DISABLED).as_bytes(), false);
    member(out, "silenced", flag(run_flags::SILENCED).as_bytes(), false);
    member(out, "exec", rule.exec.as_deref().unwrap_or(&config.default_exec), true);
    member(out, "recipient", rule.recipient.as_deref().unwrap_or(&config.default_recipient), true);
    member(out, "source", &text(&rule.source), true);
    member(out, "units", &units, true);
    member(out, "summary", &text(&snapshot.summary), true);
    member(out, "info", &text(&snapshot.info), true);
    member(out, "status", snapshot.status.name().as_bytes(), true);
    member(out, "last_status_change", &number(snapshot.last_status_change as u64), false);
    member(out, "last_updated", &number(snapshot.last_updated as u64), false);
    member(out, "next_update", &number(snapshot.next_update as u64), false);
    member(out, "update_every", &signed(rule.update_every), false);
    member(out, "delay_up_duration", &signed(rule.delay_up_duration), false);
    member(out, "delay_down_duration", &signed(rule.delay_down_duration), false);
    member(out, "delay_max_duration", &signed(rule.delay_max_duration), false);
    member(out, "delay_multiplier", &multiplier, false);
    member(out, "delay", &signed(snapshot.delay_last), false);
    member(out, "delay_up_to_timestamp", &number(snapshot.delay_up_to_timestamp as u64), false);
    member(out, "warn_repeat_every", &number(u64::from(rule.warn_repeat_every)), true);
    member(out, "crit_repeat_every", &number(u64::from(rule.crit_repeat_every)), true);
    member(out, "value_string", &format_value_and_unit(snapshot.value, &units), true);
    member(out, "last_repeat", &number(snapshot.last_repeat as u64), true);
    member(out, "times_repeat", &number(u64::from(snapshot.times_repeat)), false);

    if rule.alert_action_options & ACTION_OPTION_NO_CLEAR_NOTIFICATION != 0 {
        member(out, "no_clear_notification", b"true", false);
    }
    if rule.has_db_lookup() {
        if let Some(dimensions) = &rule.dimensions {
            string2json(out, "lookup_dimensions", dimensions);
        }
        member(out, "db_after", &number(snapshot.db_after as u64), false);
        member(out, "db_before", &number(snapshot.db_before as u64), false);
        member(out, "lookup_method", rule.time_group_name().as_bytes(), true);
        member(out, "lookup_after", &signed(rule.after), false);
        member(out, "lookup_before", &signed(rule.before), false);
        member(out, "lookup_options", options_to_text(rule.options).as_bytes(), true);
    }
    let labels = [("calc", "calc_parsed"), ("warn", "warn_parsed"), ("crit", "crit_parsed")];
    for ((label, parsed_label), expression) in labels.into_iter().zip(&alert.texts) {
        if let Some(ExpressionText { source, parsed_as }) = expression {
            string2json(out, label, source);
            string2json(out, parsed_label, parsed_as);
        }
    }
    out.extend_from_slice(b"\t\t\t\"green\":null,\n\t\t\t\"red\":null,\n\t\t\t\"value\":");
    print_netdata_double_or_null(out, snapshot.value);
    out.extend_from_slice(b"\n\t\t}");
}

/// The key an alarm is listed under: its chart's id and its name.
fn alarm_key(out: &mut Vec<u8>, alert: &Alert) {
    out.extend_from_slice(b"\t\t\"");
    out.extend_from_slice(alert.chart.id().as_bytes());
    out.push(b'.');
    out.extend_from_slice(alert.name());
    out.extend_from_slice(b"\": {\n");
}

/// `health_alarms2json()`: the body of `/api/v1/alarms`. `now` is the wall clock's seconds. A host health never
/// passed over has no alarm and 0 as its latest log id; its `status` is whether health is enabled for it.
pub fn alarms_json(host: &Host, config: &HealthConfig, alerts: Option<&HostAlerts>, all: bool, now: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(4096);
    let latest = alerts.map_or(0, HostAlerts::latest_log_unique_id);
    let status = if host.info().health_enabled { "true" } else { "false" };
    out.extend_from_slice(
        format!(
            "{{\n\t\"hostname\": \"{}\",\n\t\"latest_alarm_log_unique_id\": {latest},\n\t\"status\": {status},\n\t\
             \"now\": {},\n\t\"alarms\": {{\n",
            host.hostname(),
            now as u64
        )
        .as_bytes(),
    );
    for (i, (alert, snapshot)) in selected(host, alerts, all).iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b",\n");
        }
        alarm_key(&mut out, alert);
        alarm_json(&mut out, config, alert, snapshot);
    }
    out.extend_from_slice(b"\n\t}\n}\n");
    out
}

/// `health_alarms_values2json()`: the body of `/api/v1/alarms_values`.
pub fn alarms_values_json(host: &Host, alerts: Option<&HostAlerts>, all: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(1024);
    out.extend_from_slice(format!("{{\n\t\"hostname\": \"{}\",\n\t\"alarms\": {{\n", host.hostname()).as_bytes());
    for (i, (alert, snapshot)) in selected(host, alerts, all).iter().enumerate() {
        if i > 0 {
            out.extend_from_slice(b",\n");
        }
        alarm_key(&mut out, alert);
        out.extend_from_slice(format!("\t\t\t\"id\": {},\n\t\t\t\"value\":", alert.id).as_bytes());
        print_netdata_double_or_null(&mut out, snapshot.value);
        out.extend_from_slice(
            format!(
                ",\n\t\t\t\"last_updated\":{},\n\t\t\t\"status\": \"{}\"\n\t\t}}",
                snapshot.last_updated as u64,
                snapshot.status.name()
            )
            .as_bytes(),
        );
    }
    out.extend_from_slice(b"\n\t}\n}\n");
    out
}

/// What `/api/v1/alarm_count` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountStatus {
    /// The default: WARNING and CRITICAL.
    Raised,
    Is(Status),
}

/// `api_v1_alarm_count()`'s reading of its request: the last `status=` that names a status (in any case) decides
/// what is counted; the values of every `context=` and `ctx=` are collected.
pub fn alarm_count_request(query: &[u8]) -> (CountStatus, Option<Vec<u8>>) {
    let (mut status, mut contexts) = (CountStatus::Raised, None::<Vec<u8>>);
    for token in query.split(|&c| c == b'&').filter(|token| !token.is_empty()) {
        // strsep_skip_consecutive_separators(&value, "="): the name ends at the first run of `=`
        let Some(equals) = token.iter().position(|&c| c == b'=') else {
            continue;
        };
        let (name, rest) = token.split_at(equals);
        let value = &rest[rest.iter().take_while(|&&c| c == b'=').count()..];
        if name.is_empty() || value.is_empty() {
            continue;
        }
        match name {
            b"status" => {
                let named = match value.to_ascii_uppercase().as_slice() {
                    b"CRITICAL" => Some(Status::Critical),
                    b"WARNING" => Some(Status::Warning),
                    b"UNINITIALIZED" => Some(Status::Uninitialized),
                    b"UNDEFINED" => Some(Status::Undefined),
                    b"REMOVED" => Some(Status::Removed),
                    b"CLEAR" => Some(Status::Clear),
                    _ => None,
                };
                if let Some(named) = named {
                    status = CountStatus::Is(named);
                }
            }
            b"context" | b"ctx" => {
                let contexts = contexts.get_or_insert_with(Vec::new);
                contexts.push(b'|');
                contexts.extend_from_slice(value);
            }
            _ => {}
        }
    }
    (status, contexts)
}

/// `health_aggregate_alarms()`: how many listed alerts have the status. With contexts (texts between `,`, space
/// and `|`) the alerts of each context are counted for each time it is named.
pub fn alarm_count(host: &Host, alerts: Option<&HostAlerts>, status: CountStatus, contexts: Option<&[u8]>) -> usize {
    let listed = listed(host, alerts);
    let counted = |snapshot: &Snapshot| match status {
        CountStatus::Raised => snapshot.status as i32 >= Status::Warning as i32,
        CountStatus::Is(status) => snapshot.status == status,
    };
    match contexts {
        None => listed.iter().filter(|(_, snapshot)| counted(snapshot)).count(),
        Some(contexts) => {
            let named = contexts.split(|c| b", |".contains(c)).filter(|context| !context.is_empty());
            named
                .map(|context| {
                    let of_context = |alert: &Alert| alert.chart.meta().context.as_bytes() == context;
                    listed.iter().filter(|(alert, snapshot)| of_context(alert) && counted(snapshot)).count()
                })
                .sum()
        }
    }
}

/// The alert that answers for its name in `alarm_variables`.
struct Scored {
    chart: Arc<Chart>,
    value: f64,
    score: usize,
}

/// `health_api_v1_chart_variables2json()`: the body of `/api/v1/alarm_variables` for `chart`: what an alert on it
/// could name. `now` is the wall clock's seconds.
pub fn alarm_variables_json(host: &Host, alerts: Option<&HostAlerts>, chart: &Chart, now: i64) -> Vec<u8> {
    let meta = chart.meta();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("chart", chart.id());
    w.member_add_string("chart_name", meta.name.as_deref().unwrap_or(chart.id()));
    w.member_add_string("chart_context", &meta.context);
    w.member_add_string("family", &meta.family);
    w.member_add_string("host", host.hostname());

    w.member_add_object("current_alert_values");
    w.member_add_double("this", f64::NAN);
    w.member_add_double("after", now as f64 - 1.0);
    w.member_add_double("before", now as f64);
    w.member_add_double("now", now as f64);
    w.member_add_double("status", f64::from(Status::Removed as i32));
    let constants =
        [Status::Removed, Status::Undefined, Status::Uninitialized, Status::Clear, Status::Warning, Status::Critical];
    for status in constants {
        w.member_add_double(status.name(), f64::from(status as i32));
    }
    w.member_add_double("green", f64::NAN);
    w.member_add_double("red", f64::NAN);
    w.object_close();

    // each dimension by its id, and by its name when that differs
    let dims: Vec<_> = chart
        .dims()
        .iter()
        .map(|dim| {
            let name = dim.meta().name;
            let names = if name == dim.id() { vec![name] } else { vec![dim.id().to_owned(), name] };
            (names, dim.collection(), dim.is_float())
        })
        .collect();

    w.member_add_object("dimensions_last_stored_values");
    for (names, collection, _) in &dims {
        for name in names {
            w.member_add_double(name, collection.last_stored_value);
        }
    }
    w.object_close();

    w.member_add_object("dimensions_last_collected_values");
    for (names, collection, is_float) in &dims {
        for name in names {
            let name = format!("{name}_raw");
            if *is_float {
                w.member_add_double(&name, collection.last_collected_as_double(true));
            } else {
                w.member_add_int64(&name, collection.last_collected_value);
            }
        }
    }
    w.object_close();

    w.member_add_object("dimensions_last_collected_time");
    for (names, collection, _) in &dims {
        for name in names {
            w.member_add_int64(format!("{name}_last_collected_t"), collection.last_collected_time.0);
        }
    }
    w.object_close();

    w.member_add_object("chart_variables");
    w.member_add_int64("update_every", i64::from(meta.update_every));
    w.member_add_uint64("last_collected_t", chart.collection().last_collected.0 as u64);
    variables_json(&mut w, &chart.variables());
    w.object_close();

    w.member_add_object("host_variables");
    variables_json(&mut w, &host.variables());
    w.object_close();

    // per name, the alert whose chart shares the most labels with this chart; the first on a tie
    w.member_add_object("alerts");
    let mut by_name: IndexMap<Vec<u8>, Scored> = IndexMap::new();
    for alert in alerts.map(HostAlerts::alerts).unwrap_or_default() {
        let scored = Scored {
            value: alert.snapshot().value,
            score: alert.chart.with_meta(|alert_meta| alert_meta.labels.common_count(&meta.labels)),
            chart: Arc::clone(&alert.chart),
        };
        match by_name.get_mut(alert.name()) {
            Some(best) if scored.score > best.score => *best = scored,
            Some(_) => {}
            None => {
                by_name.insert(alert.name().to_vec(), scored);
            }
        }
    }
    for (name, scored) in &by_name {
        w.member_add_object(name);
        w.member_add_double("value", scored.value);
        w.member_add_string("instance", scored.chart.id());
        scored.chart.with_meta(|meta| w.member_add_string("context", &meta.context));
        w.member_add_uint64("score", scored.score as u64);
        w.object_close();
    }
    w.object_close();

    w.finalize();
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        find_chart, health_with, recorded_integer, variables_case_collect, variables_case_host, variables_case_rows,
        variables_case_rules,
    };

    const NOW: i64 = 1_700_000_000;

    /// The bodies the C agent answered `/api/v1/alarm_variables` with, with health off and with two linked rules:
    /// the same bytes, at the wall clock and the collection second each recorded answer shows.
    #[test]
    fn alarm_variables_match_c() {
        for (case, bodies) in [("off", 4), ("on", 3)] {
            let health = health_with(&if case == "on" { variables_case_rules() } else { String::new() });
            let host = variables_case_host();
            variables_case_collect(&host, NOW);
            if case == "on" {
                health.host_link(&host, &|| NOW, &|| true);
            }
            let alerts = health.host(&host);

            let mut compared = 0;
            for (request, status, body) in variables_case_rows(case) {
                let Some(chart) = request.strip_prefix("/api/v1/alarm_variables?chart=").filter(|_| status == 200) else {
                    continue;
                };
                let chart = find_chart(&host, chart);
                let now = recorded_integer(&body, "now").expect("the recorded clock");
                if chart.id() == "hv.a" {
                    let second = recorded_integer(&body, "last_collected_t").expect("the recorded second");
                    variables_case_collect(&host, second);
                }
                let json = alarm_variables_json(&host, alerts.as_deref(), &chart, now);
                assert_eq!(String::from_utf8_lossy(&json), String::from_utf8_lossy(&body), "{case}: {request}");
                compared += 1;
            }
            assert_eq!(compared, bodies, "{case}");
        }
    }

    /// A float dimension's last collected value is printed from its float lane, as a double; an integer one as an
    /// integer.
    #[test]
    fn a_float_dimension_s_raw_value_is_a_double() {
        use netdata_agent_rrd::chart::dim_flags;
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        let a = find_chart(&host, "hv.a");
        let b = a.dim("b").expect("the dimension");
        b.update_meta(|meta| meta.flags |= dim_flags::FLOAT);
        b.update_collection(|collection| collection.last_collected_value_float = 1.5);
        let body = String::from_utf8(alarm_variables_json(&host, None, &a, NOW)).expect("text");
        assert!(body.contains("\"a_raw\":10,\n        \"b_raw\":1.5\n"), "{body}");
    }

    /// A chart's `alarms` member and the two counts, before the host's first pass and after it.
    #[test]
    fn a_chart_shows_its_alerts_and_the_host_counts_them() {
        let health = health_with(&variables_case_rules());
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        let chart = |id: &str| host.charts().find(id, true).expect("the chart");
        let alarms = |id: &str| {
            let mut w = JsonWriter::new(JsonOptions::MINIFY);
            w.member_add_object("alarms");
            chart_alarms_json(&mut w, health.host(&host).as_deref(), &chart(id));
            w.object_close();
            w.finalize();
            String::from_utf8(w.into_bytes()).expect("text")
        };
        let counts = || status_counts(health.host(&host).as_deref());

        assert_eq!(alarms("hv.a"), r#"{"alarms":{}}"#);
        assert_eq!(linked_count(health.host(&host).as_deref()), 0);
        assert_eq!(counts(), StatusCounts::default());

        health.host_link(&host, &|| NOW, &|| true);
        // link order; the duration is the rule's `every`, on a chart collected every 5 seconds too
        assert_eq!(
            alarms("hv.a"),
            concat!(
                r#"{"alarms":{"hv_same":{"id":"hv_same","status":"UNINITIALIZED","units":"things","duration":1},"#,
                r#""hv_one":{"id":"hv_one","status":"UNINITIALIZED","units":"things","duration":1}}}"#
            )
        );
        assert_eq!(
            alarms("hv.b"),
            r#"{"alarms":{"hv_same":{"id":"hv_same","status":"UNINITIALIZED","units":"things","duration":1}}}"#
        );
        assert_eq!(alarms("hv.d"), r#"{"alarms":{}}"#);
        assert_eq!(linked_count(health.host(&host).as_deref()), 4);
        // the two alerts of the chart that was collected; any status but WARNING and CRITICAL is normal
        assert_eq!(counts(), StatusCounts { normal: 2, warning: 0, critical: 0 });

        let alerts = health.host(&host).expect("the host's alerts");
        let publish = |chart_id: &str, name: &[u8], status: Status| {
            let linked = alerts.chart_alerts(&chart(chart_id));
            let alert = linked.iter().find(|alert| alert.name() == name).expect("the alert");
            let mut run = alert.run();
            run.status = status;
            alert.publish(&run, None);
        };
        publish("hv.a", b"hv_same", Status::Warning);
        assert_eq!(counts(), StatusCounts { normal: 1, warning: 1, critical: 0 });
        publish("hv.a", b"hv_one", Status::Critical);
        assert_eq!(counts(), StatusCounts { normal: 0, warning: 1, critical: 1 });
        publish("hv.a", b"hv_one", Status::Removed);
        assert_eq!(counts(), StatusCounts { normal: 1, warning: 1, critical: 0 });
        // an alert of a chart that was never collected does not count, whatever its status
        publish("hv.b", b"hv_same", Status::Critical);
        assert_eq!(counts(), StatusCounts { normal: 1, warning: 1, critical: 0 });
        assert!(alarms("hv.a").starts_with(r#"{"alarms":{"hv_same":{"id":"hv_same","status":"WARNING","#));
    }

    /// The count a host's `health` object prints (`rrdhost-status.c:323-361`): the alerts of the charts that were
    /// collected, each under its own status as the request finds it; REMOVED and RAISED under none; nothing before
    /// the host's first pass.
    #[test]
    fn the_five_way_count_follows_the_status() {
        let health = health_with(&variables_case_rules());
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        let counts = || alert_counts(health.host(&host).as_deref());
        assert_eq!(counts(), PassCounts::default());

        health.host_link(&host, &|| NOW, &|| true);
        // hv.a was collected and has two alerts; hv.b's and the other charts' were not
        assert_eq!(counts(), PassCounts { uninitialized: 2, ..PassCounts::default() });

        let alerts = health.host(&host).expect("the host's alerts");
        let publish = |chart_id: &str, name: &[u8], status: Status| {
            let chart = host.charts().find(chart_id, true).expect("the chart");
            let linked = alerts.chart_alerts(&chart);
            let alert = linked.iter().find(|alert| alert.name() == name).expect("the alert");
            let mut run = alert.run();
            run.status = status;
            alert.publish(&run, None);
        };
        let one = |status: Status| {
            publish("hv.a", b"hv_one", status);
            counts()
        };
        publish("hv.a", b"hv_same", Status::Warning);
        let warning = PassCounts { warning: 1, ..PassCounts::default() };
        assert_eq!(one(Status::Critical), PassCounts { critical: 1, ..warning });
        assert_eq!(one(Status::Clear), PassCounts { clear: 1, ..warning });
        assert_eq!(one(Status::Undefined), PassCounts { undefined: 1, ..warning });
        assert_eq!(one(Status::Uninitialized), PassCounts { uninitialized: 1, ..warning });
        assert_eq!(one(Status::Warning), PassCounts { warning: 2, ..PassCounts::default() });
        assert_eq!(one(Status::Removed), warning);
        assert_eq!(one(Status::Raised), warning);
        // an alert of a chart that was never collected counts nowhere, whatever its status
        publish("hv.b", b"hv_same", Status::Critical);
        assert_eq!(counts(), warning);
    }

    /// The recorded case `tests/vectors/loop-api/flags/`: answers of the running C agent, two seconds into its
    /// second phase, with fourteen alerts on one chart. Its files, by name.
    fn flags_case(file: &str) -> Vec<u8> {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/loop-api/flags");
        std::fs::read(dir.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"))
    }

    /// The lines of one alarm object of a recorded `/api/v1/alarms` body, by member name.
    fn members(object: &str) -> std::collections::HashMap<&str, &str> {
        fn member(line: &str) -> Option<(&str, &str)> {
            let line = line.trim().trim_end_matches(',');
            let (key, value) = line.strip_prefix('"')?.split_once("\":")?;
            Some((key, value.trim().trim_matches('"')))
        }
        object.lines().filter_map(member).collect()
    }

    /// The three alarm endpoints against the bytes the C agent sent. The alerts are linked here from the case's
    /// rules, in C's order and with C's ids; each alert's published state is then set to what the recorded listing
    /// of all alerts shows of it, and every recorded answer of that moment must come out byte for byte: the
    /// listing of all alerts, of the raised ones, their values, and the two counts.
    #[test]
    fn the_alarm_endpoints_answer_as_c() {
        use std::os::unix::ffi::OsStrExt;

        let all = String::from_utf8(flags_case("002.body")).unwrap();
        let objects: Vec<&str> = all.split("\n\t\t\"hf.values.").skip(1).collect();
        assert_eq!(objects.len(), 14);
        let first_id: i64 = members(objects[0])["id"].parse().unwrap();

        // the rules, read from a file as the agent's were
        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("parity.conf");
        std::fs::write(&rules, flags_case("inputs/health.d-parity.conf")).unwrap();
        let config = HealthConfig {
            default_exec: b"{run}/notify/stub".to_vec(),
            default_recipient: b"root".to_vec(),
            ..HealthConfig::default()
        };
        let health = crate::Health::init(config, Box::new(|_| {}));
        assert!(crate::readfile::health_readfile(&health, rules.as_os_str().as_bytes(), false));

        // the case's chart (inputs/plugin-lines.txt), collected, and its alerts
        let host = crate::testing::host(&[]);
        host.update_info(|info| info.hostname = "parity-parent".into());
        let chart = crate::testing::chart_every(&host, "hf.values", None, Some("hf.ctx"), 1, &[]);
        chart.dim_add("a", None, 1, 1, netdata_agent_rrd::chart::Algorithm::Absolute);
        chart.update_collection(|collection| collection.last_collected = (first_id, 0));
        // without a database the first alarm's id is the second it is linked at
        health.host_link(&host, &|| first_id, &|| true);
        let alerts = health.host(&host).expect("the host's alerts");
        assert_eq!(alerts.count(), 14);

        // each alert's published state, from the recorded listing
        for object in &objects {
            let name = object.split_once('"').unwrap().0;
            let recorded = members(object);
            let alert = alerts.by_name(name.as_bytes()).pop().unwrap_or_else(|| panic!("no alert {name}"));
            let number = |member: &str| recorded[member].parse::<i64>().unwrap_or_else(|_| panic!("{name} {member}"));
            let mut run = alert.run();
            run.status = match recorded["status"] {
                "CLEAR" => Status::Clear,
                "WARNING" => Status::Warning,
                "CRITICAL" => Status::Critical,
                "UNDEFINED" => Status::Undefined,
                "UNINITIALIZED" => Status::Uninitialized,
                other => panic!("{name}: status {other}"),
            };
            run.value = match recorded["value"] {
                "null" => f64::NAN,
                value => value.parse().unwrap(),
            };
            run.last_status_change = number("last_status_change");
            run.last_updated = number("last_updated");
            run.next_update = number("next_update");
            run.delay_last = number("delay") as i32;
            run.delay_up_to_timestamp = number("delay_up_to_timestamp");
            run.last_repeat = number("last_repeat");
            run.times_repeat = number("times_repeat") as u32;
            if recorded.contains_key("db_after") {
                (run.db_after, run.db_before) = (number("db_after"), number("db_before"));
            }
            alert.publish(&run, None);
        }

        // a recorded body with what the replay here cannot know put in: the path the rules were read from, and
        // the log's latest id, which counts every entry the agent made
        let source = "{run}/etc/health.d/parity.conf";
        let recorded =
            |file: &str| String::from_utf8(flags_case(file)).unwrap().replace(source, &rules.to_string_lossy());
        let latest = |body: &str| -> String {
            let line = body.lines().find(|line| line.contains("latest_alarm_log_unique_id")).expect("the member");
            line.to_owned()
        };
        let now = |body: &str| -> i64 {
            let line = body.lines().find(|line| line.contains("\"now\": ")).expect("now");
            line.trim().trim_start_matches("\"now\": ").trim_end_matches(',').parse().unwrap()
        };
        let answer = |all: bool, expected: &str| {
            let body = alarms_json(&host, health.config(), Some(&alerts), all, now(expected));
            let body = String::from_utf8(body).unwrap();
            body.replace(&latest(&body), &latest(expected))
        };

        let (listing, raised) = (recorded("002.body"), recorded("003.body"));
        assert_eq!(answer(true, &listing), listing);
        assert_eq!(answer(false, &raised), raised);
        assert_eq!(alarms_values_json(&host, Some(&alerts), true), flags_case("009.body"));
        assert_eq!(alarms_values_json(&host, Some(&alerts), false), flags_case("010.body"));

        let count = |query: &[u8]| {
            let (status, contexts) = alarm_count_request(query);
            format!("[{}]\n", alarm_count(&host, Some(&alerts), status, contexts.as_deref())).into_bytes()
        };
        assert_eq!(count(b""), flags_case("011.body"));
        assert_eq!(count(b"status=UNDEFINED"), flags_case("012.body"));

        // which request lists all alerts: the C agent answered `all=true` and `active&all` with the listing of
        // all, and `all&active=true`, `all=1`, `ALL` and (for the values) `all=yes` with the raised ones
        for (query, all) in [
            (&b"all"[..], true),
            (b"all=true", true),
            (b"active&all", true),
            (b"", false),
            (b"all&active=true", false),
            (b"all=1", false),
            (b"ALL", false),
            (b"all=yes", false),
        ] {
            assert_eq!(alarms_select(query), all, "{}", String::from_utf8_lossy(query));
        }
    }

    /// `/api/v1/alarm_count`: the status is asked in any case, and a text that names none leaves what an earlier
    /// `status=` set; a context counts each time it is named; a chart that was never collected, is obsolete or has
    /// no dimension has no alert to count or list.
    #[test]
    fn the_count_follows_the_status_and_the_contexts() {
        let parse = |query: &[u8]| alarm_count_request(query);
        assert_eq!(parse(b""), (CountStatus::Raised, None));
        assert_eq!(parse(b"status=clear"), (CountStatus::Is(Status::Clear), None));
        assert_eq!(parse(b"status=CLEAR&status=bogus"), (CountStatus::Is(Status::Clear), None));
        assert_eq!(parse(b"status=raised"), (CountStatus::Raised, None));
        assert_eq!(parse(b"status=&status"), (CountStatus::Raised, None));
        assert_eq!(parse(b"context=a&ctx=b,c&other=d"), (CountStatus::Raised, Some(b"|a|b,c".to_vec())));

        let text = crate::testing::rule_text("template", "t", "t.ctx", &["warn: $this > 0"]);
        let health = health_with(&text);
        let host = crate::testing::host(&[]);
        let charts: Vec<_> = ["t.a", "t.b", "t.c", "t.d"]
            .iter()
            .map(|id| crate::testing::chart(&host, id, None, "t.ctx", &[]))
            .collect();
        for chart in &charts {
            chart.dim_add("d", None, 1, 1, netdata_agent_rrd::chart::Algorithm::Absolute);
            chart.update_collection(|collection| collection.last_collected = (NOW, 0));
        }
        // a second context, never collected, obsolete, without a dimension
        let other = crate::testing::chart(&host, "t.o", None, "t.other", &[]);
        other.dim_add("d", None, 1, 1, netdata_agent_rrd::chart::Algorithm::Absolute);
        other.update_collection(|collection| collection.last_collected = (NOW, 0));
        health.host_link(&host, &|| NOW, &|| true);
        let alerts = health.host(&host).expect("the host's alerts");
        for (chart, status) in charts.iter().zip([Status::Warning, Status::Critical, Status::Clear, Status::Warning]) {
            let alert = alerts.chart_alerts(chart).pop().expect("the alert");
            let mut run = alert.run();
            run.status = status;
            alert.publish(&run, None);
        }
        let count = |status: CountStatus, contexts: Option<&[u8]>| alarm_count(&host, Some(&alerts), status, contexts);
        assert_eq!(count(CountStatus::Raised, None), 3);
        assert_eq!(count(CountStatus::Is(Status::Warning), None), 2);
        assert_eq!(count(CountStatus::Is(Status::Clear), None), 1);
        assert_eq!(count(CountStatus::Raised, Some(b"|t.ctx")), 3);
        assert_eq!(count(CountStatus::Raised, Some(b"|t.ctx|t.ctx, t.other,,nope")), 6);
        assert_eq!(count(CountStatus::Raised, Some(b"|nope")), 0);

        // the fourth chart: never collected, then obsolete, then without its dimension
        let listed = |all: bool| selected(&host, Some(&alerts), all).len();
        assert_eq!((listed(true), listed(false)), (4, 3));
        charts[3].update_collection(|collection| collection.last_collected = (0, 0));
        assert_eq!((listed(true), listed(false), count(CountStatus::Raised, None)), (3, 2, 2));
        charts[3].update_collection(|collection| collection.last_collected = (NOW, 0));
        charts[3].update_meta(|meta| meta.flags |= netdata_agent_rrd::chart::flags::OBSOLETE);
        assert_eq!((listed(true), count(CountStatus::Raised, None)), (3, 2));

        // without alerts: an empty listing, with the host's health setting as its status
        let none = String::from_utf8(alarms_json(&host, health.config(), None, true, NOW)).unwrap();
        let empty = format!(
            "{{\n\t\"hostname\": \"testhost\",\n\t\"latest_alarm_log_unique_id\": 0,\n\t\"status\": true,\n\t\"now\": \
             {NOW},\n\t\"alarms\": {{\n\n\t}}\n}}\n"
        );
        assert_eq!(none, empty);
        let no_values = b"{\n\t\"hostname\": \"testhost\",\n\t\"alarms\": {\n\n\t}\n}\n";
        assert_eq!(alarms_values_json(&host, None, true), no_values);
    }
}
