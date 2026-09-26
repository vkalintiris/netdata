//! `/api/v1/data`, `/api/v2/data` and `/api/v3/data`, ported from `api_v1_data()` in
//! `src/web/api/v1/api_v1_data.c` and `api_v23_data_internal()` in `src/web/api/v2/api_v2_data.c`, with the timeout
//! checkpoint of `src/web/server/web_client.c`. Spec §2.3-2.4, §2.10-2.12.

use std::sync::Arc;
use std::time::{Duration, Instant};

use netdata_agent_query::execute::Control;
use netdata_agent_query::jsonwrap_v2::Agent;
use netdata_agent_query::output::data_query_execute;
use netdata_agent_query::request::{DataRequest, is_valid_sp, parse_v1, parse_v2};
use netdata_agent_query::tables::Format;
use netdata_agent_query::target::{QueryTarget, Source, chart_is_queryable, create};
use netdata_agent_query::window::{Window, calculate};
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;
use netdata_agent_web::status;

use crate::router::Route;
use crate::server::{self, Reply};

/// `nd_profile` as the data queries read it: the tiers in use and the agent's update every.
fn profile(route: &Route<'_>) -> netdata_agent_query::request::Profile {
    let storage = route.shared.hosts.storage();
    netdata_agent_query::request::Profile {
        storage_tiers: storage.storage_tiers() as u64,
        update_every: storage.update_every(),
    }
}

/// `rrdset_find_and_acquire(host, id, false)`, then by name: an obsolete chart only while it replicates.
pub fn find_chart(host: &Host, chart: &[u8]) -> Option<Arc<Chart>> {
    let chart = std::str::from_utf8(chart).ok()?;
    let queryable = |st: &Arc<Chart>| chart_is_queryable(st.meta().flags);
    host.charts()
        .find(chart)
        .filter(queryable)
        .or_else(|| host.charts().find_by_name(chart).filter(queryable))
}

/// `api_v1_data()`.
pub fn v1(route: &Route<'_>, host: &Arc<Host>, query: &[u8]) -> Reply {
    let params = parse_v1(query, &profile(route));
    let req = params.request;
    if !is_valid_sp(params.chart.as_deref()) && !is_valid_sp(req.contexts.as_deref()) {
        return Reply::text(status::BAD_REQUEST, "No chart or context is given.");
    }
    let chart = match (&params.chart, &req.contexts) {
        (Some(chart), None) => find_chart(host, chart),
        _ => None,
    };
    // `st->last_updated`, echoed as the datasource signature.
    let signature = chart
        .as_ref()
        .map_or(0, |st| st.collection().last_updated.0);
    let request = req.clone();
    let now_s = server::now();
    // v1 sets no `received_ut`: the query target's clock starts at its creation.
    let received = Instant::now();
    let qt = create(req, Source::V1 { host, chart }, now_s);
    let window = if qt.query.is_empty() {
        None
    } else {
        calculate(&qt, now_s)
    };
    let Some(window) = window else {
        return Reply::text(status::NOT_FOUND, "No metrics where matched to query.");
    };
    execute(route, &request, qt, window, received, signature)
}

/// `api_v23_data_internal()`: every host, whatever host the URL routed to.
pub fn v23(route: &Route<'_>, query: &[u8], version: u8) -> Reply {
    let received = Instant::now();
    let req = parse_v2(query, version, &profile(route));
    let request = req.clone();
    let now_s = server::now();
    let hosts = &route.shared.hosts;
    let qt = create(
        req,
        Source::V2 {
            hosts: hosts.all(),
            nodes_hard_hash: u64::from(hosts.version()),
        },
        now_s,
    );
    let Some(window) = calculate(&qt, now_s) else {
        return Reply::text(
            status::INTERNAL_SERVER_ERROR,
            "Failed to prepare the query.",
        );
    };
    execute(route, &request, qt, window, received, now_s)
}

/// What both handlers do once the query target is ready: the timeout checkpoint, the file name header, the Google
/// datasource or JSONP framing around `data_query_execute()`, and cacheability. `signature` is the datasource's
/// `sig` (v1: the chart's last update, v2/v3: now).
fn execute(
    route: &Route<'_>,
    req: &DataRequest,
    mut qt: QueryTarget,
    mut window: Window,
    received: Instant,
    signature: i64,
) -> Reply {
    // web_client_timeout_checkpoint_and_check(): `timeout_ms * 1000ULL` never fires for a negative timeout.
    if let Ok(timeout_ms) = u64::try_from(req.timeout_ms)
        && timeout_ms != 0
        && route.received.elapsed() >= Duration::from_millis(timeout_ms)
    {
        return Reply::text(status::GATEWAY_TIMEOUT, "Query timeout exceeded");
    }

    let google = &req.google;
    let mut headers = Vec::new();
    if let Some(name) = google.out_file_name.as_deref().filter(|n| !n.is_empty()) {
        headers.extend_from_slice(b"Content-Disposition: attachment; filename=\"");
        headers.extend_from_slice(name);
        headers.extend_from_slice(b"\"\r\n");
    }
    let handler: &[u8] = match req.format {
        Format::Datasource => google
            .response_handler
            .as_deref()
            .unwrap_or(b"google.visualization.Query.setResponse"),
        _ => google.response_handler.as_deref().unwrap_or(b"callback"),
    };
    let mut body = Vec::new();
    match req.format {
        Format::Datasource => body.extend_from_slice(
            &[
                handler,
                b"({version:'",
                &google.version,
                b"',reqId:'",
                &google.req_id,
                b"',status:'ok',sig:'",
                signature.to_string().as_bytes(),
                b"',table:",
            ]
            .concat(),
        ),
        Format::Jsonp => body.extend_from_slice(&[handler, b"("].concat()),
        _ => {}
    }

    let localhost = route.shared.hosts.localhost();
    let hostname = localhost.hostname();
    let agent = Agent {
        machine_guid: localhost.machine_guid(),
        node_id: localhost.node_id(),
        hostname: &hostname,
    };
    let control = Control {
        received,
        interrupted: route.interrupted,
        windows: route.shared.grouping_windows,
    };
    let response = data_query_execute(&mut qt, &mut window, &control, &agent);
    body.extend_from_slice(&response.body);

    match req.format {
        Format::Datasource => {
            if google.timestamp < response.latest_timestamp.unwrap_or(0) {
                body.extend_from_slice(b"});");
            } else {
                // The client already has the latest data.
                body = [
                    handler,
                    b"({version:'",
                    &google.version,
                    b"',reqId:'",
                    &google.req_id,
                    b"',status:'error',errors:[{reason:'not_modified',message:'Data not modified'}]});",
                ]
                .concat();
            }
        }
        Format::Jsonp => body.extend_from_slice(b");"),
        _ => {}
    }

    Reply {
        code: response.code,
        content_type: response.content_type,
        body,
        // Absolute windows are cacheable; the header builder still forces no-cache off 200.
        no_cacheable: qt.window.relative,
        headers,
        ..Reply::default()
    }
}
