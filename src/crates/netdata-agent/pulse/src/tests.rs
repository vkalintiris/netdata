use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, ChartType};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts};
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_rrd::pulse::QuerySource;

use super::*;

fn info(hostname: &str) -> HostInfo {
    HostInfo {
        hostname: hostname.into(),
        registry_hostname: hostname.into(),
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
    }
}

fn hosts() -> Arc<Hosts> {
    Arc::new(Hosts::new(Host::new(
        "5a1e0000-0000-4000-8000-000000000001",
        true,
        info("parent"),
    )))
}

fn pulse(hosts: &Arc<Hosts>, parents: Gates) -> Pulse {
    Pulse::new(
        Arc::clone(hosts),
        Settings {
            gap_when_lost_iterations_above: 1,
            page_size: 4096,
            parents,
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
    let hosts = hosts();
    let host = hosts.localhost();
    let mut pulse = pulse(&hosts, Gates::default());
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
    assert_eq!(shapes(host), expected);

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
    assert_eq!(shapes(host), expected);
}

/// `pulse_web_do()`'s response time: 0 and -1 before any request, then the average of the cycle's requests and its
/// slowest; an idle cycle keeps the average, and its max falls back to it once the reset max is read.
#[test]
fn response_time_as_c() {
    let hosts = hosts();
    let host = hosts.localhost();
    let mut pulse = pulse(&hosts, Gates::default());
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
    let hosts = hosts();
    let host = hosts.localhost();
    let mut pulse = pulse(&hosts, Gates::default());
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

/// The last collected value of each dimension of a chart, in order.
fn values(host: &Host, id: &str) -> Vec<(String, i64)> {
    let chart = host.charts().find(id).unwrap();
    chart
        .dims()
        .iter()
        .map(|d| (d.id().to_string(), d.collection().last_collected_value))
        .collect()
}

/// `pulse_parents_do()` on a parent: every classified host counted in the inbound nodes of its ephemerality (localhost
/// as local), and each child's four charts, found or created on every pass, created before the inbound ones, with
/// the child's labels under its identity, its state one-hot and its connections.
#[test]
fn a_parent_charts_its_children() {
    use netdata_agent_rrd::labels::SRC_CONFIG;
    use netdata_agent_rrd::pulse::host_status::*;
    const CHILD: &str = "5a1e0000-0000-4000-8000-0000000000c1";
    const EPHEMERAL_CHILD: &str = "5a1e0000-0000-4000-8000-0000000000c2";
    let hosts = hosts();
    let host = hosts.localhost();
    host.pulse_status(LOCAL);
    let child = hosts.add_archived(CHILD, info("child"), |_| {});
    child.update_labels(|l| l.add(b"env", b"prod", SRC_CONFIG));
    child.set_node_id([0xab; 16]);
    child.pulse_status(RCV_RUNNING);
    let ephemeral = hosts.add_archived(EPHEMERAL_CHILD, info("gone"), |_| {});
    ephemeral.set_ephemeral(true);
    ephemeral.pulse_status(ARCHIVED);
    // not classified yet: neither counted nor charted
    hosts.add_archived("5a1e0000-0000-4000-8000-0000000000c3", info("new"), |_| {});
    let gates = Gates {
        is_parent: true,
        stream_is_parent: true,
        is_child: false,
    };
    let mut pulse = pulse(&hosts, gates);
    pulse.cycle();

    let ids: Vec<String> = host
        .charts()
        .all()
        .iter()
        .map(|c| c.id().to_string())
        .skip(6)
        .collect();
    let per_child = |guid: &str| {
        ["traffic", "state", "reconnects", "age"]
            .map(|kind| format!("netdata.streaming.in.{kind}.{guid}"))
    };
    let mut expected: Vec<String> = per_child(CHILD).into();
    expected.extend(per_child(EPHEMERAL_CHILD));
    expected.push("netdata.netdata.streaming_inbound_permanent".into());
    expected.push("netdata.netdata.streaming_inbound_ephemeral".into());
    assert_eq!(ids, expected);

    let nodes = |ones: &[&str]| -> Vec<(String, i64)> {
        [
            "local",
            "virtual",
            "loading",
            "stale archived",
            "stale disconnected",
            "waiting",
            "waiting replication",
            "replicating",
            "running",
        ]
        .iter()
        .map(|d| (d.to_string(), i64::from(ones.contains(d))))
        .collect()
    };
    assert_eq!(
        values(host, "netdata.netdata.streaming_inbound_permanent"),
        nodes(&["local", "running"])
    );
    assert_eq!(
        values(host, "netdata.netdata.streaming_inbound_ephemeral"),
        nodes(&["stale archived"])
    );
    let inbound = host
        .charts()
        .find("netdata.netdata.streaming_inbound_ephemeral")
        .unwrap();
    let m = inbound.meta();
    assert_eq!(
        (
            m.family.as_str(),
            m.context.as_str(),
            m.title.as_str(),
            m.units.as_str(),
            m.priority,
            m.labels.get(b"type")
        ),
        (
            "Streaming",
            "netdata.streaming_inbound",
            "Inbound Nodes",
            "nodes",
            130150,
            Some(&b"ephemeral"[..])
        )
    );

    let state = format!("netdata.streaming.in.state.{CHILD}");
    let one_hot = |on: &str| -> Vec<(String, i64)> {
        [
            "archived",
            "offline",
            "waiting",
            "waiting replication",
            "replicating",
            "running",
        ]
        .iter()
        .map(|d| (d.to_string(), i64::from(*d == on)))
        .collect()
    };
    assert_eq!(values(host, &state), one_hot("running"));
    let traffic = host
        .charts()
        .find(&format!("netdata.streaming.in.traffic.{CHILD}"))
        .unwrap();
    let m = traffic.meta();
    let labels: Vec<(Vec<u8>, Vec<u8>)> = m
        .labels
        .iter()
        .filter(|l| !l.name.starts_with(b"_collect"))
        .map(|l| (l.name.clone(), l.value.clone()))
        .collect();
    let hops = child.ingestion_hops().to_string().into_bytes();
    assert_eq!(
        labels,
        [
            (b"env".to_vec(), b"prod".to_vec()),
            (b"machine_guid".to_vec(), CHILD.as_bytes().to_vec()),
            (b"hostname".to_vec(), b"child".to_vec()),
            (
                b"node_id".to_vec(),
                b"abababab-abab-abab-abab-abababababab".to_vec()
            ),
            (b"hops".to_vec(), hops),
        ]
    );
    assert_eq!(
        (
            m.family.as_str(),
            m.context.as_str(),
            m.units.as_str(),
            m.priority,
            m.chart_type
        ),
        (
            "Streaming",
            "netdata.streaming.in.traffic",
            "bytes/s",
            130160,
            ChartType::Area
        )
    );
    assert_eq!(
        traffic
            .dims()
            .iter()
            .map(|d| (d.id().to_string(), d.meta().multiplier))
            .collect::<Vec<_>>(),
        [("in".to_string(), 1), ("out".to_string(), -1)]
    );

    // the next pass keeps the labels (their version did not move) and follows the state
    child.stream_bytes_received(100);
    child.pulse_status(RCV_OFFLINE);
    pulse.cycle();
    assert_eq!(values(host, &state), one_hot("offline"));
    assert_eq!(
        values(host, "netdata.netdata.streaming_inbound_permanent"),
        nodes(&["local", "stale disconnected"])
    );
}

/// A node that neither is a parent nor streams charts nothing about its hosts; one whose stream.conf enables an API
/// key but whose profile is not parent charts its children without the inbound nodes.
#[test]
fn the_parents_gates() {
    use netdata_agent_rrd::pulse::host_status::*;
    const CHILD: &str = "5a1e0000-0000-4000-8000-0000000000c1";
    let hosts = hosts();
    hosts.localhost().pulse_status(LOCAL);
    hosts
        .add_archived(CHILD, info("child"), |_| {})
        .pulse_status(RCV_RUNNING);
    // the charts of localhost after a cycle with these gates (they accumulate from one call to the next)
    let charted = |gates| {
        pulse(&hosts, gates).cycle();
        hosts.localhost().charts().all().len()
    };
    assert_eq!(charted(Gates::default()), 6);
    let api_key_only = Gates {
        is_parent: false,
        stream_is_parent: true,
        is_child: false,
    };
    assert_eq!(
        charted(api_key_only),
        6,
        "the walk runs for a parent or a child only"
    );
    let streaming_child = Gates {
        is_parent: false,
        stream_is_parent: true,
        is_child: true,
    };
    assert_eq!(charted(streaming_child), 6 + 4, "a child's own receivers");
}
