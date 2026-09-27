use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, ChartType};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_rrd::pulse::QuerySource;

use super::*;

fn localhost() -> Arc<Host> {
    let info = HostInfo {
        hostname: "parent".into(),
        registry_hostname: "parent".into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "netdata".into(),
        program_version: "1".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 3600,
        health_enabled: false,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    };
    Arc::new(Host::new(
        "5a1e0000-0000-4000-8000-000000000001",
        true,
        info,
    ))
}

fn pulse(host: &Arc<Host>) -> Pulse {
    Pulse::new(
        Arc::clone(host),
        Settings {
            gap_when_lost_iterations_above: 1,
            page_size: 4096,
        },
    )
}

/// A chart as `rrdset_create_localhost()` makes it: id, family, context, title, units, module, priority, type, and
/// its dimensions (id, multiplier, divisor, algorithm).
type Shape = (
    String,
    String,
    String,
    String,
    String,
    String,
    i64,
    ChartType,
    Vec<(String, i32, i32, Algorithm)>,
);

fn shapes(host: &Host) -> Vec<Shape> {
    host.charts()
        .all()
        .iter()
        .map(|chart| {
            let m = chart.meta();
            // no name: the chart is named after its id
            assert_eq!(
                (m.name.as_deref(), m.plugin.as_str(), m.update_every),
                (Some(chart.id()), "netdata", 1),
                "{}",
                chart.id()
            );
            let dims = chart
                .dims()
                .iter()
                .map(|d| {
                    let dm = d.meta();
                    (d.id().to_string(), dm.multiplier, dm.divisor, dm.algorithm)
                })
                .collect();
            (
                chart.id().to_string(),
                m.family,
                m.context,
                m.title,
                m.units,
                m.module,
                m.priority,
                m.chart_type,
                dims,
            )
        })
        .collect()
}

/// `[id, family, context, title, units]`, then the priority, the type and the dimensions.
fn shape(
    text: [&str; 5],
    priority: i64,
    chart_type: ChartType,
    dims: &[(&str, i32, i32, Algorithm)],
) -> Shape {
    let [id, family, context, title, units] = text.map(str::to_string);
    (
        id,
        family,
        context,
        title,
        units,
        "pulse".to_string(),
        priority,
        chart_type,
        dims.iter()
            .map(|&(id, m, d, a)| (id.to_string(), m, d, a))
            .collect(),
    )
}

/// The first cycle creates C's charts in C's order, with C's definitions; the traffic and the points generated come
/// once they have something to show.
#[test]
fn a_cycle_creates_the_charts_as_c() {
    use Algorithm::{Absolute as A, Incremental as I};
    let host = localhost();
    let mut pulse = pulse(&host);
    pulse.cycle();
    let sources = |dims: &[&'static str]| -> Vec<(&'static str, i32, i32, Algorithm)> {
        dims.iter().map(|&d| (d, 1, 1, I)).collect()
    };
    let eight = [
        "/api/vX/data",
        "/api/vX/weights",
        "/api/vX/badge",
        "health",
        "ml",
        "exporters",
        "backfill",
        "replication",
    ];
    let mut expected = vec![
        shape(
            [
                "netdata.db_samples_collected",
                "Data Collection Samples",
                "netdata.db_samples_collected",
                "Netdata Time-Series Collected Samples",
                "samples/s",
            ],
            131003,
            ChartType::Stacked,
            &[("tier0", 1, 1, I)],
        ),
        shape(
            [
                "netdata.clients",
                "HTTP API",
                "netdata.http_api_clients",
                "Netdata Web API Clients",
                "connected clients",
            ],
            130200,
            ChartType::Line,
            &[("clients", 1, 1, A)],
        ),
        shape(
            [
                "netdata.requests",
                "HTTP API",
                "netdata.http_api_requests",
                "Netdata Web API Requests Received",
                "requests/s",
            ],
            130300,
            ChartType::Line,
            &[("requests", 1, 1, I)],
        ),
        shape(
            [
                "netdata.response_time",
                "HTTP API",
                "netdata.http_api_response_time",
                "Netdata Web API Response Time",
                "milliseconds/request",
            ],
            130500,
            ChartType::Line,
            &[("average", 1, 1000, A), ("max", 1, 1000, A)],
        ),
        shape(
            [
                "netdata.queries",
                "Time-Series Queries",
                "netdata.db_queries",
                "Netdata Time-Series DB Queries",
                "queries/s",
            ],
            131000,
            ChartType::Stacked,
            &sources(&eight),
        ),
        shape(
            [
                "netdata.db_samples_read",
                "Time-Series Queries",
                "netdata.db_samples_read",
                "Netdata Time-Series DB Samples Read",
                "samples/s",
            ],
            131001,
            ChartType::Stacked,
            &sources(&eight),
        ),
    ];
    assert_eq!(shapes(&host), expected);

    let counters = host.storage().pulse();
    counters.network.api_received(10);
    counters
        .queries
        .rrdr_query_completed(1, 20, 5, QuerySource::ApiData);
    counters.network.stream_sent(30);
    pulse.cycle();
    let traffic = |id, endpoint| {
        let chart = host.charts().find(id).unwrap();
        assert_eq!(chart.meta().labels.get(b"endpoint"), Some(endpoint), "{id}");
        shape(
            [
                id,
                "Network Traffic",
                "netdata.network",
                "Netdata Network Traffic",
                "kilobits/s",
            ],
            130150,
            ChartType::Area,
            &[("in", 8, 1000, I), ("out", -8, 1000, I)],
        )
    };
    expected.push(shape(
        [
            "netdata.db_points_results",
            "Time-Series Queries",
            "netdata.db_points_results",
            "Netdata Time-Series Points Generated",
            "points/s",
        ],
        131002,
        ChartType::Stacked,
        &sources(&[
            "/api/vX/data",
            "/api/vX/weights",
            "/api/vX/badge",
            "health",
            "ml",
            "replication",
        ]),
    ));
    expected.push(traffic("netdata.network_api", &b"web-server"[..]));
    expected.push(traffic("netdata.network_streaming", &b"streaming"[..]));
    assert_eq!(shapes(&host), expected);
}

/// `pulse_web_do()`'s response time: 0 and -1 before any request, then the average of the cycle's requests and its
/// slowest; an idle cycle keeps the average, and its max falls back to it once the reset max is read.
#[test]
fn response_time_as_c() {
    let host = localhost();
    let mut pulse = pulse(&host);
    let web = &host.storage().pulse().web;
    let collected = || {
        let chart = host.charts().find("netdata.response_time").unwrap();
        let value = |id| chart.dim(id).unwrap().collection().last_collected_value;
        (value("average"), value("max"))
    };
    pulse.cycle();
    assert_eq!(collected(), (0, -1));
    web.request_completed(1000, 0, 0);
    web.request_completed(5000, 0, 0);
    pulse.cycle();
    assert_eq!(collected(), (3000, 5000));
    pulse.cycle();
    assert_eq!(collected(), (3000, 3000));
    web.client_connected();
    pulse.cycle();
    let clients = host.charts().find("netdata.clients").unwrap();
    assert_eq!(
        clients
            .dim("clients")
            .unwrap()
            .collection()
            .last_collected_value,
        1
    );
}

/// Each tier has its dimension, fed with the points stored since the start.
#[test]
fn the_stored_points_per_tier() {
    let host = localhost();
    let mut pulse = pulse(&host);
    netdata_agent_rrd::pulse::point_stored(0);
    netdata_agent_rrd::pulse::point_stored(0);
    host.storage().pulse().ingestion.collection_completed(1);
    pulse.cycle();
    let chart = host.charts().find("netdata.db_samples_collected").unwrap();
    assert_eq!(
        chart
            .dim("tier0")
            .unwrap()
            .collection()
            .last_collected_value,
        2
    );
}
