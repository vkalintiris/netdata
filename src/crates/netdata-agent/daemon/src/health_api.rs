//! The web API's alert endpoints (`src/web/api/v1/api_v1_alarms.c`, `src/web/api/v2/api_v2_alert_config.c`): the
//! host's alarms, their values and their count, its alert log, a rule's configuration, what an alert on a chart
//! could name, and the trace of one name's lookup; and the badge (`src/web/api/v1/api_v1_badge/web_buffer_svg.c`).
//! None looks at whether health is on: a host without alerts answers too.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_health::api::{
    alarm_count as count_alarms, alarm_count_request, alarm_variables_json, alarms_json, alarms_select,
    alarms_values_json,
};
use netdata_agent_health::badge::{Source, api_v1_badge};
use netdata_agent_health::sql::{ConfigAnswer, LogView, alarm_log_json, alert_config_json};
use netdata_agent_health::variable::trace_json;
use netdata_agent_log::netdata_log_error;
use netdata_agent_query::execute::Control;
use netdata_agent_query::value::{ValueRequest, ValueResult, chart_value};
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::pulse::QuerySource;
use netdata_agent_web::content_type::ContentType;
use netdata_agent_text::parse::strtoul0;
use netdata_agent_web::status;

use crate::router::{Host, Route};
use crate::server::{Reply, Shared};
use crate::v1_charts::{json_reply, named_chart, parameters, single_chart};

/// `api_v1_alarms()`: the host's alarms, all of them or the raised ones.
pub fn alarms(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let health = &route.shared.health;
    let alerts = health.host(host);
    json_reply(alarms_json(host, health.config(), alerts.as_deref(), alarms_select(query), now_realtime_s()))
}

/// `api_v1_alarm_log()`: the host's alert log as the table has it: the entries above the unique id `after` (read as
/// `strtoul(value, NULL, 0)` reads it; the last one given counts), of the chart `chart` when one is named, at most
/// the host's limit, which is 0 before its first health pass. Always 200: without a database, or when the
/// statement cannot be prepared, the body is empty.
pub fn alarm_log(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let (mut after, mut chart) = (0, None);
    for (name, value) in parameters(query) {
        match name {
            b"after" => after = strtoul0(value).0 as i64,
            b"chart" => chart = Some(value),
            _ => {}
        }
    }
    let shared = route.shared;
    let Some(meta) = &shared.meta else {
        crate::meta_store::no_database("sql_health_alarm_log2json");
        netdata_log_error!("Failed to prepare statement SQL_SELECT_HEALTH_LOG");
        return json_reply(Vec::new());
    };
    let (Some(meta), Some(host_id)) = (meta.upgrade(), crate::meta_store::host_id(host)) else {
        return json_reply(Vec::new());
    };
    let (info, localhost) = (host.info(), shared.hosts.localhost().info());
    let (default_exec, default_recipient) = shared.health.host_defaults(host);
    let view = LogView {
        hostname: info.hostname.as_bytes(),
        utc_offset: info.utc_offset,
        abbrev_timezone: info.abbrev_timezone.as_bytes(),
        default_exec,
        default_recipient,
        user_config_dir: shared.user_config_dir.as_bytes(),
        registry_hostname: localhost.registry_hostname.as_bytes(),
    };
    let limit = shared.health.host(host).map_or(0, |alerts| alerts.log_max());
    json_reply(alarm_log_json(&meta, &host_id, &view, after, chart, limit))
}

/// `api_v2_alert_config()`, also `/api/v3/alert_config`: the rule whose hash `config` names (the last one given),
/// as its row of `alert_hash` has it, with localhost's default recipient where the rule names none.
pub fn alert_config(route: &Route<'_>, _: &Host, query: &[u8]) -> Reply {
    let Some((_, config)) = parameters(query).filter(|(name, _)| *name == b"config").last() else {
        return Reply::text(status::BAD_REQUEST, "A config hash ID is required. Add ?config=UUID query param");
    };
    let shared = route.shared;
    let meta = shared.meta.as_ref().and_then(std::sync::Weak::upgrade);
    let recipient = shared.health.host_defaults(shared.hosts.localhost()).1;
    match alert_config_json(meta.as_deref(), config, recipient) {
        ConfigAnswer::Found(body) => json_reply(body),
        ConfigAnswer::NotFound => Reply::text(status::NOT_FOUND, "Config is not found."),
        ConfigAnswer::Failed => Reply::text(status::INTERNAL_SERVER_ERROR, "Failed to execute SQL query."),
    }
}

/// `api_v1_alarms_values()`.
pub fn alarms_values(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let alerts = route.shared.health.host(host);
    json_reply(alarms_values_json(host, alerts.as_deref(), alarms_select(query)))
}

/// `api_v1_alarm_count()`: `[N]`, the alarms of a status, of the named contexts when any is named.
pub fn alarm_count(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let alerts = route.shared.health.host(host);
    let (status, contexts) = alarm_count_request(query);
    let count = count_alarms(host, alerts.as_deref(), status, contexts.as_deref());
    json_reply(format!("[{count}]\n").into_bytes())
}

/// `api_v1_alarm_variables()`: `api_v1_single_chart_helper()` with `health_api_v1_chart_variables2json()`.
pub fn alarm_variables(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    single_chart(host, query, |chart| {
        let alerts = route.shared.health.host(host);
        alarm_variables_json(host, alerts.as_deref(), chart, now_realtime_s())
    })
}

/// `api_v1_variable()`: the lookup of `variable` for a blank alert on `chart`, traced.
pub fn variable(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let (mut chart, mut variable) = (None, None);
    for (name, value) in parameters(query) {
        match name {
            b"chart" => chart = Some(value),
            b"variable" => variable = Some(value),
            _ => {}
        }
    }
    let (Some(chart), Some(variable)) = (chart, variable) else {
        return Reply::text(status::BAD_REQUEST, "A chart= and a variable= are required.");
    };
    match named_chart(host, chart) {
        Ok(chart) => {
            let alerts = route.shared.health.host(host);
            json_reply(trace_json(host, alerts.as_deref(), &chart, variable, &now_realtime_s))
        }
        Err(not_found) => not_found,
    }
}

/// What a badge asks of the running agent for a chart value: the chart's last entry, and one query that counts as
/// a badge's (`QUERY_SOURCE_API_BADGE`), which C does not let the web client interrupt.
struct BadgeSource<'a>(&'a Shared);

impl Source for BadgeSource<'_> {
    fn last_entry_s(&self, chart: &Chart) -> i64 {
        chart.retention().1
    }

    fn value(&self, host: &Host, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult {
        let storage = self.0.hosts.storage();
        let control = Control {
            received: Instant::now(),
            interrupted: &|_| false,
            windows: self.0.grouping_windows,
            pulse: Some((&storage.pulse().queries, QuerySource::ApiBadge)),
            progress: None,
        };
        chart_value(host, chart, request, &crate::data::profile_of(storage), &control, now_realtime_s())
    }
}

/// `api_v1_badge()`, for `/api/v1/badge.svg` and `/api/v3/badge.svg`: the badge of an alert or of a chart's value,
/// as [`api_v1_badge`] leaves it: the SVG (or the text of a request without a chart), whether it may be cached, its
/// date and expiry, and the `Refresh` header a request asked for.
pub fn badge(route: &Route<'_>, host: &Host, query: &[u8]) -> Reply {
    let shared = route.shared;
    let alerts = shared.health.host(host);
    let gap = shared.gap_when_lost_iterations_above;
    let badge = api_v1_badge(host, alerts.as_deref(), query, gap, &BadgeSource(shared), &now_realtime_s);
    Reply {
        code: badge.code,
        content_type: if badge.svg { ContentType::ImageSvgXml } else { ContentType::TextPlain },
        body: badge.body,
        no_cacheable: badge.no_cacheable,
        date: badge.date,
        expires: badge.expires,
        headers: badge.headers,
        tracking_required: false,
    }
}
