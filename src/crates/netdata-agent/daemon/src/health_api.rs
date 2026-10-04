//! The web API's two variable endpoints (`src/web/api/v1/api_v1_alarms.c`): what an alert on a chart could name, and
//! the trace of one name's lookup. Neither looks at whether health is on: a host without alerts answers too.

use netdata_agent_health::api::alarm_variables_json;
use netdata_agent_health::variable::trace_json;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_web::status;

use crate::router::{Host, Route};
use crate::server::Reply;
use crate::v1_charts::{json_reply, named_chart, parameters, single_chart};

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
