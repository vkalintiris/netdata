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

fn settings(parents: Gates) -> Settings {
    Settings {
        gap_when_lost_iterations_above: 1,
        page_size: 4096,
        parents,
        out_of_memory_protection: 0,
        system_memory: || None,
    }
}

fn pulse(hosts: &Arc<Hosts>, parents: Gates) -> Pulse {
    Pulse::new(Arc::clone(hosts), settings(parents))
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
    let absolute = |dims: &[&'static str]| -> Vec<(&'static str, i32, i32, Algorithm)> {
        dims.iter().map(|&d| (d, 1, 1, A)).collect()
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
        shape(
            [
                "netdata.server_cpu",
                "CPU usage",
                "netdata.server_cpu",
                "Netdata CPU usage",
                "milliseconds/s",
            ],
            130000,
            ChartType::Stacked,
            &[("user", 1, 1000, I), ("system", 1, 1000, I)],
        ),
        shape(
            [
                "netdata.uptime",
                "Uptime",
                "netdata.uptime",
                "Netdata uptime",
                "seconds",
            ],
            130150,
            ChartType::Line,
            &[("uptime", 1, 1, A)],
        ),
        shape(
            [
                "netdata.memory",
                "Memory Usage",
                "netdata.memory",
                "Netdata Memory",
                "bytes",
            ],
            130100,
            ChartType::Stacked,
            &absolute(&[
                "dbengine",
                "rrd",
                "sqlite3",
                "metadata",
                "uuid",
                "labels",
                "ML",
                "strings",
                "streaming",
                "buffers",
                "workers",
                "aral",
                "judy",
                "slots",
                "other",
                "health log",
            ]),
        ),
        shape(
            [
                "netdata.memory_buffers",
                "Memory Usage",
                "netdata.memory_buffers",
                "Netdata Memory Buffers",
                "bytes",
            ],
            130102,
            ChartType::Stacked,
            &absolute(&[
                "queries",
                "collection",
                "aclk",
                "api",
                "functions",
                "sqlite",
                "exporters",
                "health",
                "streaming",
                "streaming cbuf",
                "replication",
                "web",
                "aral-by-size free",
                "aral-judy free",
                "uuid",
            ]),
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
    let mut child_info = info("child");
    child_info.system_info.hops = 2;
    let child = hosts.add_archived(CHILD, child_info, |_| {});
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
    // the daemon step, last in the cycle
    expected.extend(
        ["server_cpu", "uptime", "memory", "memory_buffers"].map(|id| format!("netdata.{id}")),
    );
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
            (b"hops".to_vec(), b"2".to_vec()),
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

    // the next pass keeps the labels (their version did not move), follows the state and the traffic, and revives
    // an obsolete dimension
    let label_version = traffic.meta().labels.version();
    let connections = host
        .charts()
        .find(&format!("netdata.streaming.in.reconnects.{CHILD}"))
        .unwrap()
        .dim("connections")
        .unwrap();
    connections.update_meta(|m| m.flags |= netdata_agent_rrd::chart::dim_flags::OBSOLETE);
    child.stream_bytes_received(100);
    child.pulse_status(RCV_OFFLINE);
    pulse.cycle();
    assert_eq!(values(host, &state), one_hot("offline"));
    assert_eq!(
        values(host, "netdata.netdata.streaming_inbound_permanent"),
        nodes(&["local", "stale disconnected"])
    );
    assert_eq!(traffic.meta().labels.version(), label_version);
    assert_eq!(
        values(host, &format!("netdata.streaming.in.traffic.{CHILD}"))[0],
        ("in".to_string(), 100)
    );
    assert_eq!(
        connections.meta().flags & netdata_agent_rrd::chart::dim_flags::OBSOLETE,
        0
    );

    // a new label of the child reaches its four charts on the next pass
    child.update_labels(|l| l.add(b"rack", b"r1", SRC_CONFIG));
    pulse.cycle();
    for id in per_child(CHILD) {
        let chart = host.charts().find(&id).unwrap();
        assert_eq!(chart.meta().labels.get(b"rack"), Some(&b"r1"[..]), "{id}");
    }
}

/// The parents module's gates: a node that neither is a parent nor streams walks no host; the children's charts need an
/// API key in stream.conf, the inbound nodes a parent's profile.
#[test]
fn the_parents_gates() {
    use netdata_agent_rrd::pulse::host_status::*;
    // localhost's charts after a cycle of fresh hosts, with a child running, under these gates
    let charted = |is_parent, stream_is_parent, is_child| {
        let hosts = hosts();
        hosts.localhost().pulse_status(LOCAL);
        hosts
            .add_archived(
                "5a1e0000-0000-4000-8000-0000000000c1",
                info("child"),
                |_| {},
            )
            .pulse_status(RCV_RUNNING);
        let gates = Gates {
            is_parent,
            stream_is_parent,
            is_child,
        };
        pulse(&hosts, gates).cycle();
        hosts.localhost().charts().all().len()
    };
    // the first cycle's charts without the parents module's
    const BASE: usize = 10;
    assert_eq!(charted(false, false, false), BASE);
    assert_eq!(
        charted(false, true, false),
        BASE,
        "the walk runs for a parent or a child only"
    );
    assert_eq!(
        charted(false, true, true),
        BASE + 4,
        "a child's own receivers"
    );
    assert_eq!(
        charted(true, false, false),
        BASE + 2,
        "the inbound nodes without the children's charts"
    );
    assert_eq!(charted(true, true, false), BASE + 4 + 2);
}

/// Hosts on a dbengine of `tiers` tiers, each with a 100 MiB quota and an hour of retention, at 1,800,000,000; the
/// localhost of `mode`.
fn dbengine_hosts(tiers: usize, mode: DbMode) -> (Vec<tempfile::TempDir>, Arc<Hosts>) {
    use netdata_agent_rrd::storage::StorageLayout;
    use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
    use netdata_agent_storage::dbengine::engine::mrg::Mrg;
    use netdata_agent_storage::dbengine::engine::query::{Dbengine, EngineConfig};
    let mrg = Mrg::new();
    let dirs: Vec<_> = (0..tiers).map(|_| tempfile::tempdir().unwrap()).collect();
    let tiers = dirs
        .iter()
        .enumerate()
        .map(|(tier, dir)| {
            let cfg = TierConfig {
                max_disk_space: 100 << 20,
                max_retention_s: 3600,
                ..TierConfig::new(tier, dir.path().to_path_buf())
            };
            load(cfg, &mrg, 1_800_000_000).unwrap()
        })
        .collect();
    let engine = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            main_cache_bytes: 1 << 20,
            extent_cache_bytes: 1 << 20,
            ..EngineConfig::new(|| 1_800_000_000)
        },
    );
    let storage = Arc::new(StorageLayout::new(Some(engine)));
    let localhost = Host::with_storage(
        "5a1e0000-0000-4000-8000-000000000001",
        true,
        HostInfo {
            db_mode: mode,
            ..info("parent")
        },
        &storage,
    );
    (dirs, Arc::new(Hosts::with_storage(localhost, storage)))
}

/// `dbengine_retention_statistics()`: a chart per tier localhost keeps in the dbengine, every ten seconds, labelled
/// by its tier, with the space and time retention.
#[test]
fn the_dbengine_tiers_retention() {
    let (_dirs, hosts) = dbengine_hosts(2, DbMode::Dbengine);
    pulse(&hosts, Gates::default()).cycle();
    let host = hosts.localhost();
    for tier in 0..2 {
        let id = format!("netdata.dbengine_retention_tier{tier}");
        let chart = host.charts().find(&id).unwrap();
        let m = chart.meta();
        assert_eq!(
            (
                m.family.as_str(),
                m.context.as_str(),
                m.title.as_str(),
                m.units.as_str(),
                m.module.as_str(),
                m.priority,
                m.update_every,
                m.chart_type,
                m.labels.get(b"tier")
            ),
            (
                "dbengine retention",
                "netdata.dbengine_tier_retention",
                "dbengine space and time retention",
                "%",
                "stats",
                134900,
                10,
                ChartType::Line,
                Some(tier.to_string().as_bytes())
            )
        );
        // rrdeng_get_used_disk_space(): the files, a file's target, less what the last file holds
        let td = &host.storage().dbengine().unwrap().tiers[tier];
        let last = td.filenos().into_iter().max().unwrap();
        let used = td.current_disk_space() + td.config.target_datafile_size()
            - td.file(last).unwrap().pos();
        assert_eq!(
            values(host, &id),
            [
                ("space".to_string(), (used * 100 / (100 << 20)) as i64),
                ("time".to_string(), 0)
            ]
        );
        assert_ne!(
            m.flags & netdata_agent_rrd::chart::flags::METADATA_UPDATE,
            0,
            "the metadata writer stores it"
        );
    }
    // an alloc localhost keeps tier 0 out of the dbengine
    let (_dirs, hosts) = dbengine_hosts(2, DbMode::Alloc);
    pulse(&hosts, Gates::default()).cycle();
    let host = hosts.localhost();
    assert!(
        host.charts()
            .find("netdata.dbengine_retention_tier0")
            .is_none()
    );
    assert!(
        host.charts()
            .find("netdata.dbengine_retention_tier1")
            .is_some()
    );
}

/// The retention percentages as C computes them.
#[test]
fn retention_percentages_as_c() {
    use crate::retention::{space_percent, time_percent};
    assert_eq!(space_percent(25, 100, 7), 25, "of the quota");
    assert_eq!(
        space_percent(25, 0, 75),
        25,
        "of the free and used space without one"
    );
    assert_eq!(space_percent(250, 100, 0), 250, "not clamped");
    assert_eq!(space_percent(0, 0, 0), 0);
    assert_eq!(time_percent(1000, 1900, 3600), 25);
    assert_eq!(time_percent(1000, 9000, 3600), 100, "clamped at 100");
    assert_eq!(time_percent(0, 9000, 3600), 0, "no oldest point");
    assert_eq!(time_percent(1000, 1900, 0), 0, "no time limit");
    assert_eq!(
        time_percent(2000, 1100, 3600),
        -25,
        "an oldest point ahead of the clock"
    );
    assert_eq!(
        space_percent(u64::MAX / 50, 100, 0),
        ((u64::MAX / 50).wrapping_mul(100) / 100) as i64,
        "the product wraps as C's"
    );
}

/// `pulse_daemon_memory_do()`'s out of memory protection: charted only while the system's memory is known and the
/// dbengine keeps some free.
#[test]
fn the_out_of_memory_protection() {
    let charted = |protection, system_memory: fn() -> Option<u64>| {
        let hosts = hosts();
        let mut pulse = Pulse::new(
            Arc::clone(&hosts),
            Settings {
                out_of_memory_protection: protection,
                system_memory,
                ..settings(Gates::default())
            },
        );
        pulse.cycle();
        hosts
            .localhost()
            .charts()
            .find("netdata.out_of_memory_protection")
            .map(|chart| {
                let m = chart.meta();
                assert_eq!(
                    (
                        m.family.as_str(),
                        m.context.as_str(),
                        m.title.as_str(),
                        m.units.as_str(),
                        m.module.as_str(),
                        m.priority,
                        m.update_every,
                        m.chart_type
                    ),
                    (
                        "Memory Usage",
                        "netdata.out_of_memory_protection",
                        "Out of Memory Protection",
                        "bytes",
                        "pulse",
                        130103,
                        1,
                        ChartType::Area
                    )
                );
                values(hosts.localhost(), "netdata.out_of_memory_protection")
            })
    };
    assert_eq!(
        charted(1 << 30, || Some(123_456)),
        Some(vec![("available".to_string(), 123_456)])
    );
    assert_eq!(charted(0, || Some(123_456)), None, "no protection");
    assert_eq!(charted(1 << 30, || None), None, "the memory unknown");

    // the gate is read every cycle: memory unknown at the first, known at the second
    static KNOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let hosts = hosts();
    let mut pulse = Pulse::new(
        Arc::clone(&hosts),
        Settings {
            out_of_memory_protection: 1 << 30,
            system_memory: || {
                KNOWN
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .then_some(42)
            },
            ..settings(Gates::default())
        },
    );
    let oom = || {
        hosts
            .localhost()
            .charts()
            .find("netdata.out_of_memory_protection")
            .is_some()
    };
    pulse.cycle();
    assert!(!oom());
    KNOWN.store(true, std::sync::atomic::Ordering::Relaxed);
    pulse.cycle();
    assert!(oom());
}

/// `pulse_daemon_do()`'s values: the uptime counts from the first cycle; the memory by owner is Rust's own for the
/// dbengine caches, the ram rings and SQLite, 0 for the rest, and the buffers 0 (D82).
#[test]
fn the_daemon_charts_values() {
    let hosts = hosts();
    let host = hosts.localhost();
    let mut charts = pulse(&hosts, Gates::default());
    charts.cycle();
    assert_eq!(values(host, "netdata.uptime"), [("uptime".to_string(), 0)]);
    let memory = values(host, "netdata.memory");
    assert_eq!(memory[0], ("dbengine".to_string(), 0), "no engine");
    assert!(memory[1].1 > 0, "the pulse charts' own rings: {memory:?}");
    assert!(memory[3..].iter().all(|(_, v)| *v == 0), "{memory:?}");
    assert!(
        values(host, "netdata.memory_buffers")
            .iter()
            .all(|(_, v)| *v == 0)
    );
    // the rings counted are the ones the charts hold; the first reading, as C's, came after the memory chart's own
    // were made and before the buffers chart's
    let rings = |ids: &[&str]| -> i64 {
        host.charts()
            .all()
            .iter()
            .filter(|c| ids.is_empty() || ids.contains(&c.id()))
            .flat_map(|c| c.dims())
            .filter_map(|d| d.ring().map(|r| r.memsize() as i64))
            .sum()
    };
    assert_eq!(host.storage().pulse().rrd_memory.read(), rings(&[]));
    assert_eq!(memory[1].1, rings(&[]) - rings(&["netdata.memory_buffers"]));

    // a dbengine localhost: the caches hold its charts' pages once they stored (a point per second of the clock)
    let (_dirs, hosts) = dbengine_hosts(1, DbMode::Dbengine);
    let mut charts = pulse(&hosts, Gates::default());
    charts.cycle();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    charts.cycle();
    let memory = values(hosts.localhost(), "netdata.memory");
    assert!(memory[0].1 > 0, "{memory:?}");
    assert_eq!(memory[1], ("rrd".to_string(), 0), "no ram rings");
}
