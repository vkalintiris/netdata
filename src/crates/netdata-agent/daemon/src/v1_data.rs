//! `/api/v1/data`, ported from `api_v1_data()` in `src/web/api/v1/api_v1_data.c`, with the timeout checkpoint of
//! `src/web/server/web_client.c`. Spec §2.3, §2.10-2.12.

use std::sync::Arc;
use std::time::Duration;

use netdata_agent_query::STORAGE_TIERS;
use netdata_agent_query::execute::Control;
use netdata_agent_query::output::data_query_execute;
use netdata_agent_query::request::{is_valid_sp, parse_v1};
use netdata_agent_query::tables::Format;
use netdata_agent_query::target::{Source, chart_is_queryable, create};
use netdata_agent_query::window::calculate;
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;
use netdata_agent_web::status;

use crate::router::Route;
use crate::server::{self, Reply};

/// `rrdset_find_and_acquire(host, id, false)`, then by name: an obsolete chart only while it replicates.
fn find_chart(host: &Host, chart: &[u8]) -> Option<Arc<Chart>> {
    let chart = std::str::from_utf8(chart).ok()?;
    let queryable = |st: &Arc<Chart>| chart_is_queryable(st.meta().flags);
    host.charts()
        .find(chart)
        .filter(queryable)
        .or_else(|| host.charts().find_by_name(chart).filter(queryable))
}

/// `api_v1_data()`.
pub fn data(route: &Route<'_>, host: &Arc<Host>, query: &[u8]) -> Reply {
    let params = parse_v1(query, STORAGE_TIERS);
    let req = params.request;
    if !is_valid_sp(params.chart.as_deref()) && !is_valid_sp(req.contexts.as_deref()) {
        return Reply::text(status::BAD_REQUEST, "No chart or context is given.");
    }
    let chart = match (&params.chart, &req.contexts) {
        (Some(chart), None) => find_chart(host, chart),
        _ => None,
    };
    // `st->last_updated`, echoed as the datasource signature.
    let chart_last_updated = chart
        .as_ref()
        .map_or(0, |st| st.collection().last_updated.0);
    let google = req.google.clone();
    let format = req.format;
    let timeout_ms = req.timeout_ms;
    let now_s = server::now();
    let mut qt = create(req, Source::V1 { host, chart }, now_s);
    let window = if qt.query.is_empty() {
        None
    } else {
        calculate(&qt, now_s)
    };
    let Some(mut window) = window else {
        return Reply::text(status::NOT_FOUND, "No metrics where matched to query.");
    };

    // web_client_timeout_checkpoint_and_check(): `timeout_ms * 1000ULL` never fires for a negative timeout.
    if let Ok(timeout_ms) = u64::try_from(timeout_ms)
        && timeout_ms != 0
        && route.received.elapsed() >= Duration::from_millis(timeout_ms)
    {
        return Reply::text(status::GATEWAY_TIMEOUT, "Query timeout exceeded");
    }

    let mut headers = Vec::new();
    if let Some(name) = google.out_file_name.as_deref().filter(|n| !n.is_empty()) {
        headers.extend_from_slice(b"Content-Disposition: attachment; filename=\"");
        headers.extend_from_slice(name);
        headers.extend_from_slice(b"\"\r\n");
    }
    let mut body = Vec::new();
    let handler: &[u8] = match format {
        Format::Datasource => google
            .response_handler
            .as_deref()
            .unwrap_or(b"google.visualization.Query.setResponse"),
        _ => google.response_handler.as_deref().unwrap_or(b"callback"),
    };
    let jsonp = |body: &mut Vec<u8>, parts: &[&[u8]]| {
        for part in parts {
            body.extend_from_slice(part);
        }
    };
    match format {
        Format::Datasource => jsonp(
            &mut body,
            &[
                handler,
                b"({version:'",
                &google.version,
                b"',reqId:'",
                &google.req_id,
                b"',status:'ok',sig:'",
                chart_last_updated.to_string().as_bytes(),
                b"',table:",
            ],
        ),
        Format::Jsonp => jsonp(&mut body, &[handler, b"("]),
        _ => {}
    }

    let control = Control {
        received: route.received,
        interrupted: route.interrupted,
    };
    let response = data_query_execute(&mut qt, &mut window, &control);
    body.extend_from_slice(&response.body);

    match format {
        Format::Datasource => {
            if google.timestamp < response.latest_timestamp.unwrap_or(0) {
                body.extend_from_slice(b"});");
            } else {
                // The client already has the latest data.
                body.clear();
                jsonp(
                    &mut body,
                    &[
                        handler,
                        b"({version:'",
                        &google.version,
                        b"',reqId:'",
                        &google.req_id,
                        b"',status:'error',errors:[{reason:'not_modified',message:'Data not modified'}]});",
                    ],
                );
            }
        }
        Format::Jsonp => body.extend_from_slice(b");"),
        _ => {}
    }

    Reply {
        code: response.code,
        content_type: response.content_type,
        body,
        // The handler's last word: absolute windows are cacheable (the router still forces no-cache off 200).
        cacheable: !qt.window.relative,
        headers,
        ..Reply::default()
    }
}
