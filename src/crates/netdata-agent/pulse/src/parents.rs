//! `pulse-parents.c`: the inbound nodes per state and ephemerality, the outbound nodes per state, and each child's
//! streaming charts, from one walk of the hosts. The extended reason charts are left out (D80.4).

use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartType, Dim, dim_flags};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_rrd::labels::SRC_AUTO;
use netdata_agent_rrd::pulse::host_status::*;
use netdata_agent_text::print::print_uuid_lower;

use crate::chart::{Def, Localhost, WithDims, dim, set};

/// `PULSE_INBOUND_STATE`, in C's order, with the inbound nodes chart's dimension of each.
const INBOUND: [(u32, &str); 9] = [
    (LOCAL, "local"),
    (VIRTUAL, "virtual"),
    (LOADING, "loading"),
    (ARCHIVED, "stale archived"),
    (RCV_OFFLINE, "stale disconnected"),
    (RCV_WAITING, "waiting"),
    (RCV_REPLICATION_WAIT, "waiting replication"),
    (RCV_REPLICATING, "replicating"),
    (RCV_RUNNING, "running"),
];

/// `PULSE_OUTBOUND_STATE`, in C's order (a host's state is the first of these it has), with the outbound nodes
/// chart's dimension of each, in the chart's order.
const OUTBOUND: [u32; 8] =
    [SND_OFFLINE, SND_CONNECTING, SND_PENDING, SND_WAITING, SND_REPLICATING, SND_RUNNING, SND_NO_DST, SND_NO_DST_FAILED];
const OUTBOUND_DIMS: [(&str, u32); 8] = [
    ("connecting", SND_CONNECTING),
    ("pending", SND_PENDING),
    ("offline", SND_OFFLINE),
    ("waiting", SND_WAITING),
    ("replicating", SND_REPLICATING),
    ("running", SND_RUNNING),
    ("no dst", SND_NO_DST),
    ("failed", SND_NO_DST_FAILED),
];

/// `pulse_outbound_state()`.
fn outbound_state(status: u32) -> Option<u32> {
    OUTBOUND.into_iter().find(|&bit| status & bit != 0)
}

/// The per-child state chart's dimensions, by inbound state (none for local, virtual and loading).
const CHILD_STATES: [Option<&str>; 9] = [
    None,
    None,
    None,
    Some("archived"),
    Some("offline"),
    Some("waiting"),
    Some("waiting replication"),
    Some("replicating"),
    Some("running"),
];

/// `pulse_inbound_state()`: the first inbound state of the host's pulse state, in C's order.
fn inbound_state(status: u32) -> Option<usize> {
    INBOUND.iter().position(|&(bit, _)| status & bit != 0)
}

/// Where the parents module runs.
#[derive(Debug, Clone, Copy, Default)]
pub struct Gates {
    /// `netdata_conf_is_parent()`: the node's profile is parent.
    pub is_parent: bool,
    /// `stream_conf_is_parent()`: stream.conf enables an API key.
    pub stream_is_parent: bool,
    /// `stream_conf_is_child()`: `[stream] enabled`.
    pub is_child: bool,
}

#[derive(Default)]
pub(crate) struct Charts {
    nodes: [Option<WithDims>; 2],
    outbound: Option<WithDims>,
}

impl Charts {
    /// `pulse_parents_do()`.
    pub fn update(&mut self, localhost: &Localhost<'_>, hosts: &Hosts, gates: Gates) {
        let mut inbound = [[0i64; INBOUND.len()]; 2];
        let mut outbound = [0i64; OUTBOUND.len()];
        if gates.is_parent || gates.is_child {
            for host in hosts.all() {
                let status = host.pulse_state();
                // not classified yet
                if status == 0 {
                    continue;
                }
                if let Some(state) = inbound_state(status) {
                    inbound[usize::from(status & EPHEMERAL != 0)][state] += 1;
                    if gates.stream_is_parent && !host.is_local() {
                        child_charts(localhost, &host, state);
                    }
                }
                if let Some(bit) = outbound_state(status) {
                    outbound[OUTBOUND.iter().position(|&b| b == bit).unwrap_or(0)] += 1;
                }
            }
        }
        if gates.is_parent {
            self.inbound(localhost, &inbound);
        }
        if gates.is_child {
            let (chart, dims) = self.outbound.get_or_insert_with(|| {
                let chart = localhost.create(&Def {
                    id: "streaming_outbound",
                    family: "Streaming",
                    context: Some("netdata.streaming_outbound"),
                    title: "Outbound Nodes",
                    units: "nodes",
                    module: "pulse",
                    priority: 130153,
                    chart_type: ChartType::Line,
                });
                let dims = OUTBOUND_DIMS.iter().map(|&(name, _)| dim(&chart, name, 1, 1, Algorithm::Absolute)).collect();
                (chart, dims)
            });
            for (rd, &(_, bit)) in dims.iter().zip(&OUTBOUND_DIMS) {
                set(rd, outbound[OUTBOUND.iter().position(|&b| b == bit).unwrap_or(0)]);
            }
            localhost.done(chart);
        }
    }

    /// The inbound nodes charts, per ephemerality.
    fn inbound(&mut self, localhost: &Localhost<'_>, inbound: &[[i64; INBOUND.len()]; 2]) {
        for (idx, (kind, id)) in [
            ("permanent", "netdata.streaming_inbound_permanent"),
            ("ephemeral", "netdata.streaming_inbound_ephemeral"),
        ]
        .into_iter()
        .enumerate()
        {
            let (chart, dims) = self.nodes[idx].get_or_insert_with(|| {
                let chart = localhost.create(&Def {
                    id,
                    family: "Streaming",
                    context: Some("netdata.streaming_inbound"),
                    title: "Inbound Nodes",
                    units: "nodes",
                    module: "pulse",
                    priority: 130150,
                    chart_type: ChartType::Line,
                });
                chart.update_meta(|m| m.labels.add(b"type", kind.as_bytes(), SRC_AUTO));
                let dims = INBOUND
                    .iter()
                    .map(|&(_, name)| dim(&chart, name, 1, 1, Algorithm::Absolute))
                    .collect();
                (chart, dims)
            });
            for (rd, nodes) in dims.iter().zip(inbound[idx]) {
                set(rd, nodes);
            }
            localhost.done(chart);
        }
    }
}

/// `pulse_child_dim()`: the dimension if it is there and not obsolete, else `rrddim_add()`, which revives it.
fn child_dim(chart: &Chart, id: &str, multiplier: i32, algorithm: Algorithm) -> Arc<Dim> {
    match chart.dim(id) {
        Some(rd) if rd.meta().flags & dim_flags::OBSOLETE == 0 => rd,
        _ => dim(chart, id, multiplier, 1, algorithm),
    }
}

/// `pulse_child_chart_labels()`: the child's labels, then its identity over them. C also asks health, which is not
/// ported, to recheck the chart's labels.
fn child_labels(chart: &Chart, host: &Host) {
    let labels = host.labels();
    let hostname = host.hostname();
    let node_id = host.node_id();
    let hops = host.ingestion_hops().to_string();
    chart.update_meta(|m| {
        m.labels.copy_from(&labels);
        m.labels
            .add(b"machine_guid", host.machine_guid().as_bytes(), SRC_AUTO);
        m.labels.add(b"hostname", hostname.as_bytes(), SRC_AUTO);
        if node_id != [0; 16] {
            let mut text = Vec::new();
            print_uuid_lower(&mut text, &node_id);
            m.labels.add(b"node_id", &text, SRC_AUTO);
        }
        m.labels.add(b"hops", hops.as_bytes(), SRC_AUTO);
    });
}

/// `pulse_child_charts_update()`: found or created by id on every pass, the labels applied only when the child's
/// label version moved.
fn child_charts(localhost: &Localhost<'_>, host: &Host, state: usize) {
    let guid = host.machine_guid();
    let refresh_labels = host.pulse_labels_refresh(host.labels_version());
    let chart = |kind: &str, context, title, units, priority, chart_type| {
        let chart = localhost.create(&Def {
            id: &format!("streaming.in.{kind}.{guid}"),
            family: "Streaming",
            context: Some(context),
            title,
            units,
            module: "pulse",
            priority,
            chart_type,
        });
        if refresh_labels {
            child_labels(&chart, host);
        }
        chart
    };

    let traffic = chart(
        "traffic",
        "netdata.streaming.in.traffic",
        "Inbound Streaming Traffic",
        "bytes/s",
        130160,
        ChartType::Area,
    );
    let (bytes_in, bytes_out) = host.stream_bytes();
    set(
        &child_dim(&traffic, "in", 1, Algorithm::Incremental),
        bytes_in as i64,
    );
    set(
        &child_dim(&traffic, "out", -1, Algorithm::Incremental),
        bytes_out as i64,
    );
    localhost.done(&traffic);

    let st = chart(
        "state",
        "netdata.streaming.in.state",
        "Inbound Streaming State",
        "state",
        130161,
        ChartType::Line,
    );
    for (i, name) in CHILD_STATES.iter().enumerate() {
        if let Some(name) = name {
            set(
                &child_dim(&st, name, 1, Algorithm::Absolute),
                i64::from(state == i),
            );
        }
    }
    localhost.done(&st);

    let reconnects = chart(
        "reconnects",
        "netdata.streaming.in.reconnects",
        "Inbound Streaming Reconnects",
        "connects/s",
        130162,
        ChartType::Line,
    );
    set(
        &child_dim(&reconnects, "connections", 1, Algorithm::Incremental),
        i64::from(host.receiver_connections()),
    );
    localhost.done(&reconnects);

    let age = chart(
        "age",
        "netdata.streaming.in.age",
        "Inbound Streaming State Age",
        "seconds",
        130163,
        ChartType::Line,
    );
    let changed = host.state_changed_s();
    let now_s = netdata_agent_rrd::collection::now_realtime_timeval().0;
    set(
        &child_dim(&age, "age", 1, Algorithm::Absolute),
        if changed != 0 && now_s > changed {
            now_s - changed
        } else {
            0
        },
    );
    localhost.done(&age);
}
