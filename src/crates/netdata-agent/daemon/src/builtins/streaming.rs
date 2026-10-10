//! The two streaming built-ins (`function-netdata-streaming.c`, `function-topology-streaming.c`).
//! `netdata-streaming` is C's table of every host's status. `topology:streaming` answers `info` as C does and
//! D176.3's placeholder to every other call: its topology is not ported.

use netdata_agent_nrpc::reply::{Payload, Reply};
use netdata_agent_pluginsd_proto::caps;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_rrd::status::{IngestStatus, IngestType, Status, StreamStatus};
use netdata_agent_rrd::upstream::Traffic;
use netdata_agent_streaming::reason::Reason;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};
use netdata_agent_text::rrdf::{self, Field, FieldType, Filter, Summary, Transform, Visual, opts, sort};

use super::{STREAMING_HELP, STREAMING_TOPOLOGY_HELP, json_reply, not_implemented};

/// `STREAMING_FUNCTION_UPDATE_EVERY`.
const UPDATE_EVERY: i64 = 10;

/// `function_netdata_streaming()`: one row per host, localhost first, then in creation order, each from its status at
/// one clock; then the columns with the maxima the rows raised. Its words are not read: any call is the table.
pub(super) fn render(hosts: &Hosts, reply: &mut Reply) -> u16 {
    let now = now_realtime_s();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("hostname", hosts.localhost().hostname());
    w.member_add_uint64("status", 200);
    w.member_add_string("type", "table");
    w.member_add_time_t("update_every", UPDATE_EVERY);
    w.member_add_boolean("has_history", false);
    w.member_add_string("help", STREAMING_HELP);
    w.member_add_array(Some(b"data"));
    let mut max = Maxima::default();
    for host in hosts.all() {
        row(&mut w, &host, &host.status(now), &mut max);
    }
    w.array_close();
    columns_and_charts(&mut w, &max);
    // a second read of the clock, as C's
    w.member_add_time_t("expires", now_realtime_s() + UPDATE_EVERY);
    json_reply(w, reply)
}

/// The members after the rows (`F.c:339-1008`): the columns with the maxima the rows raised, the default sort, the
/// charts and the groupings.
fn columns_and_charts(w: &mut JsonWriter, max: &Maxima) {
    w.member_add_object("columns");
    for (id, c) in COLUMNS.iter().enumerate() {
        rrdf::add_field(
            w,
            &Field {
                id,
                key: c.key.as_bytes(),
                name: c.name.as_bytes(),
                kind: c.kind,
                visual: c.visual,
                transform: c.transform,
                decimal_points: c.decimals,
                units: c.units.map(str::as_bytes),
                max: c.max.value(max),
                sort: c.sort,
                pointer_to: None,
                summary: c.summary,
                filter: c.filter,
                options: c.options,
                default_value: None,
            },
        );
    }
    w.object_close();
    w.member_add_string("default_sort_column", "Node");
    w.member_add_object("charts");
    for (key, name) in CHARTS {
        w.member_add_object(key);
        w.member_add_string("name", name);
        w.member_add_string("type", "stacked-bar");
        w.member_add_array(Some(b"columns"));
        w.add_array_item_string(key);
        w.array_close();
        w.object_close();
    }
    w.object_close();
    w.member_add_array(Some(b"default_charts"));
    for chart in ["InAge", "OutAge"] {
        w.add_array_item_array();
        w.add_array_item_string(chart);
        w.add_array_item_string("Node");
        w.array_close();
    }
    w.array_close();
    w.member_add_object("group_by");
    for (key, name) in GROUP_BY {
        w.member_add_object(key);
        w.member_add_string("name", name);
        w.member_add_array(Some(b"columns"));
        w.add_array_item_string(key);
        w.array_close();
        w.object_close();
    }
    w.object_close();
}

/// A host's row, its cells in the columns' order (`F.c:73-333`), raising the maxima its cells pass.
fn row(w: &mut JsonWriter, host: &Host, s: &Status, max: &mut Maxima) {
    let info = host.info();
    let ephemeral = host.is_ephemeral();
    raise(&mut max.db_metrics, s.db.metrics);
    raise(&mut max.db_instances, s.db.instances);
    raise(&mut max.db_contexts, s.db.contexts);
    raise(&mut max.in_repl_instances, s.ingest.replication.instances);
    raise(&mut max.out_repl_instances, s.stream.replication.instances);
    for (m, &sent) in max.sent.iter_mut().zip(&s.stream.sent_bytes) {
        raise(m, sent);
    }
    w.add_array_item_array();
    w.add_array_item_string(&info.hostname);
    w.add_array_item_object();
    w.member_add_string("severity", severity(ephemeral, s));
    w.object_close();
    w.add_array_item_string(if ephemeral { "ephemeral" } else { "permanent" });
    w.add_array_item_string(&info.program_name);
    w.add_array_item_string(&info.program_version);
    info.system_info.to_streaming_function_array(w);

    // retention, in milliseconds; its duration only when both ends are set and apart
    w.add_array_item_uint64((s.db.first_time_s as u64).wrapping_mul(1000));
    raise(&mut max.db_from, s.db.first_time_s);
    w.add_array_item_uint64((s.db.last_time_s as u64).wrapping_mul(1000));
    raise(&mut max.db_to, s.db.last_time_s);
    if s.db.first_time_s != 0 && s.db.last_time_s != 0 && s.db.last_time_s > s.db.first_time_s {
        let duration = s.db.last_time_s - s.db.first_time_s;
        w.add_array_item_uint64(duration as u64);
        raise(&mut max.db_duration, duration);
    } else {
        w.add_array_item_null();
    }
    w.add_array_item_uint64(s.db.metrics);
    w.add_array_item_uint64(s.db.instances);
    w.add_array_item_uint64(s.db.contexts);
    w.add_array_item_string(s.ingest.status.name());
    w.add_array_item_string(s.stream.status.name());
    w.add_array_item_string(s.ml.status.name());

    // collection
    w.add_array_item_uint64(u64::from(s.ingest.id));
    raise(&mut max.in_connections, s.ingest.id);
    since_and_age(w, s.now, s.ingest.since_s, &mut max.in_since, &mut max.in_age);
    w.add_array_item_string(match s.ingest.kind {
        IngestType::Localhost => "LOCALHOST",
        IngestType::Virtual => "VIRTUAL NODE",
        _ => Reason(s.ingest.reason).text(),
    });
    w.add_array_item_int64(i64::from(s.ingest.hops));
    raise(&mut max.in_hops, s.ingest.hops);
    w.add_array_item_double(s.ingest.replication.completion);
    w.add_array_item_uint64(s.ingest.replication.instances);
    let local = matches!(s.ingest.kind, IngestType::Localhost | IngestType::Virtual);
    w.add_array_item_string(if local { "localhost" } else { &s.ingest.peers.local_ip });
    w.add_array_item_uint64(u64::from(s.ingest.peers.local_port));
    raise(&mut max.in_local_port, s.ingest.peers.local_port);
    w.add_array_item_string(&s.ingest.peers.peer_ip);
    w.add_array_item_uint64(u64::from(s.ingest.peers.peer_port));
    raise(&mut max.in_remote_port, s.ingest.peers.peer_port);
    w.add_array_item_string(if s.ingest.tls { "SSL" } else { "PLAIN" });
    caps::to_json_array(w, s.ingest.capabilities, None);
    w.add_array_item_uint64(s.ingest.metrics);
    w.add_array_item_uint64(s.ingest.instances);
    w.add_array_item_uint64(s.ingest.contexts);

    // streaming
    w.add_array_item_uint64(u64::from(s.stream.id));
    raise(&mut max.out_connections, s.stream.id);
    since_and_age(w, s.now, s.stream.since_s, &mut max.out_since, &mut max.out_age);
    w.add_array_item_string(Reason(s.stream.reason).text());
    w.add_array_item_int64(i64::from(s.stream.hops));
    raise(&mut max.out_hops, s.stream.hops);
    w.add_array_item_double(s.stream.replication.completion);
    w.add_array_item_uint64(s.stream.replication.instances);
    // C raises a maximum of the local port too and never prints it: the column passes NaN (`F.c:775`)
    w.add_array_item_string(&s.stream.peers.local_ip);
    w.add_array_item_uint64(u64::from(s.stream.peers.local_port));
    w.add_array_item_string(&s.stream.peers.peer_ip);
    w.add_array_item_uint64(u64::from(s.stream.peers.peer_port));
    raise(&mut max.out_remote_port, s.stream.peers.peer_port);
    w.add_array_item_string(if s.stream.tls { "SSL" } else { "PLAIN" });
    w.add_array_item_string(if s.stream.compression { "COMPRESSED" } else { "UNCOMPRESSED" });
    caps::to_json_array(w, s.stream.capabilities, None);
    // not the instance's order: data, metadata, replication, functions (`F.c:266-269`)
    for traffic in [Traffic::Data, Traffic::Metadata, Traffic::Replication, Traffic::Functions] {
        w.add_array_item_uint64(s.stream.sent_bytes[traffic as usize] as u64);
    }
    // stream_parent_handshake_error_to_json(): each parent's last handshake, and the newest attempt among them
    w.add_array_item_array();
    let mut last_attempt_ut = 0;
    for parent in &s.stream.parents {
        last_attempt_ut = last_attempt_ut.max(parent.since_ut);
        w.add_array_item_string(Reason(parent.reason).text());
    }
    w.array_close();
    if last_attempt_ut == 0 {
        w.add_array_item_null();
        w.add_array_item_null();
    } else {
        let since_ms = last_attempt_ut / 1000;
        w.add_array_item_uint64(since_ms);
        raise(&mut max.out_attempt_since, since_ms);
        let age = s.now - (last_attempt_ut / 1_000_000) as i64;
        w.add_array_item_time_t(age);
        raise(&mut max.out_attempt_age, age);
    }

    // ML: C prints the five counts of a host whose models run; no host's run here (`Status::ml`), so the cells are
    // C's other branch
    for _ in 0..5 {
        w.add_array_item_null();
    }
    w.add_array_item_string(host.machine_guid());
    w.add_array_item_uuid(Some(&host.node_id()));
    w.array_close();
}

/// A since and its age (`F.c:182-195`, `:233-246`): the since in milliseconds and its age at the status's clock
/// (signed), both null when the since is 0.
fn since_and_age(w: &mut JsonWriter, now: i64, since_s: i64, max_since: &mut u64, max_age: &mut i64) {
    if since_s == 0 {
        w.add_array_item_null();
        w.add_array_item_null();
        return;
    }
    let since_ms = (since_s as u64).wrapping_mul(1000);
    w.add_array_item_uint64(since_ms);
    raise(max_since, since_ms);
    let age = now - since_s;
    w.add_array_item_time_t(age);
    raise(max_age, age);
}

/// The row's severity (`F.c:107-136`): an ephemeral host is always normal; any other is critical while its collection
/// is offline or archived, else warning while its stream is offline for a reason other than having no parent.
fn severity(ephemeral: bool, s: &Status) -> &'static str {
    if ephemeral {
        "normal"
    } else if matches!(s.ingest.status, IngestStatus::Offline | IngestStatus::Archived) {
        "critical"
    } else if s.stream.status == StreamStatus::Offline && Reason(s.stream.reason) != Reason::SP_NO_DESTINATION {
        "warning"
    } else {
        "normal"
    }
}

/// `if(cell > max) max = cell`.
fn raise<T: PartialOrd>(max: &mut T, cell: T) {
    if cell > *max {
        *max = cell;
    }
}

/// C's maxima over the rows, in its types: each starts at 0 but the hops' (-1), and only a cell that is printed
/// raises one.
struct Maxima {
    db_from: i64,
    db_to: i64,
    db_duration: i64,
    db_metrics: u64,
    db_instances: u64,
    db_contexts: u64,
    in_connections: u32,
    in_since: u64,
    in_age: i64,
    in_hops: i16,
    in_repl_instances: u64,
    in_local_port: u16,
    in_remote_port: u16,
    out_connections: u32,
    out_since: u64,
    out_age: i64,
    out_hops: i16,
    out_repl_instances: u64,
    out_remote_port: u16,
    sent: [usize; 4],
    out_attempt_since: u64,
    out_attempt_age: i64,
}

impl Default for Maxima {
    fn default() -> Self {
        Maxima {
            db_from: 0,
            db_to: 0,
            db_duration: 0,
            db_metrics: 0,
            db_instances: 0,
            db_contexts: 0,
            in_connections: 0,
            in_since: 0,
            in_age: 0,
            in_hops: -1,
            in_repl_instances: 0,
            in_local_port: 0,
            in_remote_port: 0,
            out_connections: 0,
            out_since: 0,
            out_age: 0,
            out_hops: -1,
            out_repl_instances: 0,
            out_remote_port: 0,
            sent: [0; 4],
            out_attempt_since: 0,
            out_attempt_age: 0,
        }
    }
}

/// A maximum the rows raise, as a column prints it.
#[derive(Debug, Clone, Copy)]
enum Stat {
    /// The two retention ends, in milliseconds.
    DbFrom,
    DbTo,
    DbDuration,
    DbMetrics,
    DbInstances,
    DbContexts,
    InConnections,
    InSince,
    InAge,
    InHops,
    InReplInstances,
    InLocalPort,
    InRemotePort,
    OutConnections,
    OutSince,
    OutAge,
    OutHops,
    OutReplInstances,
    OutRemotePort,
    Sent(Traffic),
    OutAttemptSince,
    OutAttemptAge,
}

/// A column's `max`: none (NaN, left out), the completions' constant 100, the ML counts' (0: no host's models run),
/// or one the rows raised.
#[derive(Debug, Clone, Copy)]
enum Max {
    None,
    Percent,
    Ml,
    Of(Stat),
}

impl Max {
    fn value(self, m: &Maxima) -> f64 {
        match self {
            Max::None => f64::NAN,
            Max::Percent => 100.0,
            Max::Ml => 0.0,
            Max::Of(stat) => match stat {
                Stat::DbFrom => m.db_from as f64 * 1000.0,
                Stat::DbTo => m.db_to as f64 * 1000.0,
                Stat::DbDuration => m.db_duration as f64,
                Stat::DbMetrics => m.db_metrics as f64,
                Stat::DbInstances => m.db_instances as f64,
                Stat::DbContexts => m.db_contexts as f64,
                Stat::InConnections => f64::from(m.in_connections),
                Stat::InSince => m.in_since as f64,
                Stat::InAge => m.in_age as f64,
                Stat::InHops => f64::from(m.in_hops),
                Stat::InReplInstances => m.in_repl_instances as f64,
                Stat::InLocalPort => f64::from(m.in_local_port),
                Stat::InRemotePort => f64::from(m.in_remote_port),
                Stat::OutConnections => f64::from(m.out_connections),
                Stat::OutSince => m.out_since as f64,
                Stat::OutAge => m.out_age as f64,
                Stat::OutHops => f64::from(m.out_hops),
                Stat::OutReplInstances => m.out_repl_instances as f64,
                Stat::OutRemotePort => f64::from(m.out_remote_port),
                Stat::Sent(traffic) => m.sent[traffic as usize] as f64,
                Stat::OutAttemptSince => m.out_attempt_since as f64,
                Stat::OutAttemptAge => m.out_attempt_age as f64,
            },
        }
    }
}

/// A column: `buffer_rrdf_table_add_field()`'s arguments but its index, which is its place in [`COLUMNS`], its
/// `pointer_to` and default value, NULL in every column, and its maximum, which the rows raise.
struct Column {
    key: &'static str,
    name: &'static str,
    kind: FieldType,
    visual: Visual,
    transform: Transform,
    decimals: usize,
    units: Option<&'static str>,
    max: Max,
    sort: u8,
    summary: Summary,
    filter: Filter,
    options: u8,
}

/// A column of text with C's commonest arguments: a value without units or maximum, ascending, counted, filtered by
/// a multiselect.
const fn text(key: &'static str, name: &'static str, options: u8) -> Column {
    col(
        key, name, FieldType::String, Visual::Value, Transform::None, 0, None, Max::None, sort::ASCENDING,
        Summary::Count, Filter::MultiSelect, options,
    )
}

#[allow(clippy::too_many_arguments)]
const fn col(
    key: &'static str,
    name: &'static str,
    kind: FieldType,
    visual: Visual,
    transform: Transform,
    decimals: usize,
    units: Option<&'static str>,
    max: Max,
    sort: u8,
    summary: Summary,
    filter: Filter,
    options: u8,
) -> Column {
    Column { key, name, kind, visual, transform, decimals, units, max, sort, summary, filter, options }
}

/// The 85 columns, in C's order (`F.c:343-915`; `.local/scratch-m10c12/gencols.py` makes this table from C's calls
/// and checks it). The three Collected columns print the database's maxima, as C's.
const COLUMNS: [Column; 85] = [
    text("Node", "Node's Hostname", opts::VISIBLE | opts::UNIQUE_KEY | opts::STICKY),
    col(
        "rowOptions", "rowOptions", FieldType::None, Visual::RowOptions, Transform::None, 0, None, Max::None,
        sort::FIXED, Summary::Count, Filter::None, opts::DUMMY,
    ),
    text("Ephemerality", "The type of ephemerality for the node", opts::VISIBLE),
    text("AgentName", "The name of the Netdata agent", opts::NONE),
    text("AgentVersion", "The version of the Netdata agent", opts::NONE),
    text("OSName", "The name of the host's operating system", opts::NONE),
    text("OSId", "The identifier of the host's operating system", opts::NONE),
    text("OSIdLike", "The ID-like string for the host's OS", opts::NONE),
    text("OSVersion", "The version of the host's operating system", opts::NONE),
    text("OSVersionId", "The version identifier of the host's OS", opts::NONE),
    text("OSDetection", "Details about host OS detection", opts::NONE),
    text("CPUCores", "The number of CPU cores in the host", opts::NONE),
    col(
        "DiskSpace", "The total disk space available on the host", FieldType::String, Visual::Value, Transform::None, 0,
        None, Max::None, sort::ASCENDING, Summary::Count, Filter::None, opts::NONE,
    ),
    col(
        "CPUFreq", "The CPU frequency of the host", FieldType::String, Visual::Value, Transform::None, 0, None,
        Max::None, sort::ASCENDING, Summary::Count, Filter::None, opts::NONE,
    ),
    col(
        "RAMTotal", "The total RAM available on the host", FieldType::String, Visual::Value, Transform::None, 0, None,
        Max::None, sort::ASCENDING, Summary::Count, Filter::None, opts::NONE,
    ),
    text("ContainerOSName", "The name of the container's operating system", opts::NONE),
    text("ContainerOSId", "The identifier of the container's operating system", opts::NONE),
    text("ContainerOSIdLike", "The ID-like string for the container's OS", opts::NONE),
    text("ContainerOSVersion", "The version of the container's OS", opts::NONE),
    text("ContainerOSVersionId", "The version identifier of the container's OS", opts::NONE),
    text("ContainerOSDetection", "Details about container OS detection", opts::NONE),
    text("IsK8sNode", "Whether this node is part of a Kubernetes cluster", opts::NONE),
    text("KernelName", "The kernel name", opts::NONE),
    text("KernelVersion", "The kernel version", opts::NONE),
    text("Architecture", "The system architecture", opts::NONE),
    text("Virtualization", "The virtualization technology in use", opts::NONE),
    text("VirtDetection", "Details about virtualization detection", opts::NONE),
    text("Container", "Container type information", opts::NONE),
    text("ContainerDetection", "Details about container detection", opts::NONE),
    text("CloudProviderType", "The type of cloud provider", opts::NONE),
    text("CloudInstanceType", "The type of cloud instance", opts::NONE),
    text("CloudInstanceRegion", "The region of the cloud instance", opts::NONE),
    col(
        "dbFrom", "DB Data Retention From", FieldType::Timestamp, Visual::Value, Transform::DatetimeMs, 0, None,
        Max::Of(Stat::DbFrom), sort::ASCENDING, Summary::Min, Filter::None, opts::NONE,
    ),
    col(
        "dbTo", "DB Data Retention To", FieldType::Timestamp, Visual::Value, Transform::DatetimeMs, 0, None,
        Max::Of(Stat::DbTo), sort::ASCENDING, Summary::Max, Filter::None, opts::NONE,
    ),
    col(
        "dbDuration", "DB Data Retention Duration", FieldType::Duration, Visual::Value, Transform::DurationS, 0, None,
        Max::Of(Stat::DbDuration), sort::ASCENDING, Summary::Max, Filter::None, opts::VISIBLE,
    ),
    col(
        "dbMetrics", "Time-series Metrics in the DB", FieldType::Integer, Visual::Value, Transform::Number, 0, None,
        Max::Of(Stat::DbMetrics), sort::DESCENDING, Summary::Sum, Filter::Range, opts::VISIBLE,
    ),
    col(
        "dbInstances", "Instances in the DB", FieldType::Integer, Visual::Value, Transform::Number, 0, None,
        Max::Of(Stat::DbInstances), sort::DESCENDING, Summary::Sum, Filter::Range, opts::VISIBLE,
    ),
    col(
        "dbContexts", "Contexts in the DB", FieldType::Integer, Visual::Value, Transform::Number, 0, None,
        Max::Of(Stat::DbContexts), sort::DESCENDING, Summary::Sum, Filter::Range, opts::VISIBLE,
    ),
    text("InStatus", "Data Collection Online Status", opts::VISIBLE),
    text("OutStatus", "Streaming Online Status", opts::VISIBLE),
    text("MlStatus", "ML Status", opts::VISIBLE),
    col(
        "InConnections", "Number of times this child connected", FieldType::Integer, Visual::Value, Transform::None, 0,
        None, Max::Of(Stat::InConnections), sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "InSince", "Last Data Collection Status Change", FieldType::Timestamp, Visual::Value, Transform::DatetimeMs, 0,
        None, Max::Of(Stat::InSince), sort::DESCENDING, Summary::Min, Filter::None, opts::NONE,
    ),
    col(
        "InAge", "Last Data Collection Online Status Change Age", FieldType::Duration, Visual::Value,
        Transform::DurationS, 0, None, Max::Of(Stat::InAge), sort::ASCENDING, Summary::Max, Filter::Range,
        opts::VISIBLE,
    ),
    text("InReason", "Data Collection Online Status Reason", opts::VISIBLE),
    col(
        "InHops", "Data Collection Distance Hops from Origin Node", FieldType::Integer, Visual::Value, Transform::None,
        0, None, Max::Of(Stat::InHops), sort::ASCENDING, Summary::Min, Filter::Range, opts::VISIBLE,
    ),
    col(
        "InReplCompletion", "Inbound Replication Completion", FieldType::Integer, Visual::Bar, Transform::Number, 1,
        Some("%"), Max::Percent, sort::DESCENDING, Summary::Min, Filter::Range, opts::VISIBLE,
    ),
    col(
        "InReplInstances", "Inbound Replicating Instances", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("instances"), Max::Of(Stat::InReplInstances), sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    text("InLocalIP", "Inbound Local IP", opts::NONE),
    col(
        "InLocalPort", "Inbound Local Port", FieldType::Integer, Visual::Value, Transform::Number, 0, None,
        Max::Of(Stat::InLocalPort), sort::ASCENDING, Summary::Count, Filter::Range, opts::NONE,
    ),
    text("InRemoteIP", "Inbound Remote IP", opts::NONE),
    col(
        "InRemotePort", "Inbound Remote Port", FieldType::Integer, Visual::Value, Transform::Number, 0, None,
        Max::Of(Stat::InRemotePort), sort::ASCENDING, Summary::Count, Filter::Range, opts::NONE,
    ),
    text("InSSL", "Inbound SSL Connection", opts::NONE),
    col(
        "InCapabilities", "Inbound Connection Capabilities", FieldType::Array, Visual::Pill, Transform::None, 0, None,
        Max::None, sort::ASCENDING, Summary::Count, Filter::MultiSelect, opts::NONE,
    ),
    col(
        "CollectedMetrics", "Time-series Metrics Currently Collected", FieldType::Integer, Visual::Value,
        Transform::Number, 0, None, Max::Of(Stat::DbMetrics), sort::DESCENDING, Summary::Sum, Filter::Range,
        opts::VISIBLE,
    ),
    col(
        "CollectedInstances", "Instances Currently Collected", FieldType::Integer, Visual::Value, Transform::Number, 0,
        None, Max::Of(Stat::DbInstances), sort::DESCENDING, Summary::Sum, Filter::Range, opts::VISIBLE,
    ),
    col(
        "CollectedContexts", "Contexts Currently Collected", FieldType::Integer, Visual::Value, Transform::Number, 0,
        None, Max::Of(Stat::DbContexts), sort::DESCENDING, Summary::Sum, Filter::Range, opts::VISIBLE,
    ),
    col(
        "OutConnections", "Number of times connected to a parent", FieldType::Integer, Visual::Value, Transform::None,
        0, None, Max::Of(Stat::OutConnections), sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "OutSince", "Last Streaming Status Change", FieldType::Timestamp, Visual::Value, Transform::DatetimeMs, 0, None,
        Max::Of(Stat::OutSince), sort::DESCENDING, Summary::Max, Filter::None, opts::NONE,
    ),
    col(
        "OutAge", "Last Streaming Status Change Age", FieldType::Duration, Visual::Value, Transform::DurationS, 0, None,
        Max::Of(Stat::OutAge), sort::ASCENDING, Summary::Min, Filter::Range, opts::VISIBLE,
    ),
    text("OutReason", "Streaming Status Reason", opts::VISIBLE),
    col(
        "OutHops", "Streaming Distance Hops from Origin Node", FieldType::Integer, Visual::Value, Transform::None, 0,
        None, Max::Of(Stat::OutHops), sort::ASCENDING, Summary::Min, Filter::Range, opts::VISIBLE,
    ),
    col(
        "OutReplCompletion", "Outbound Replication Completion", FieldType::Integer, Visual::Bar, Transform::Number, 1,
        Some("%"), Max::Percent, sort::DESCENDING, Summary::Min, Filter::Range, opts::VISIBLE,
    ),
    col(
        "OutReplInstances", "Outbound Replicating Instances", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("instances"), Max::Of(Stat::OutReplInstances), sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    text("OutLocalIP", "Outbound Local IP", opts::NONE),
    col(
        "OutLocalPort", "Outbound Local Port", FieldType::Integer, Visual::Value, Transform::Number, 0, None, Max::None,
        sort::ASCENDING, Summary::Count, Filter::None, opts::NONE,
    ),
    text("OutRemoteIP", "Outbound Remote IP", opts::NONE),
    col(
        "OutRemotePort", "Outbound Remote Port", FieldType::Integer, Visual::Value, Transform::Number, 0, None,
        Max::Of(Stat::OutRemotePort), sort::ASCENDING, Summary::Count, Filter::Range, opts::NONE,
    ),
    text("OutSSL", "Outbound SSL Connection", opts::NONE),
    text("OutCompression", "Outbound Compressed Connection", opts::NONE),
    col(
        "OutCapabilities", "Outbound Connection Capabilities", FieldType::Array, Visual::Pill, Transform::None, 0, None,
        Max::None, sort::ASCENDING, Summary::Count, Filter::MultiSelect, opts::NONE,
    ),
    col(
        "OutTrafficData", "Outbound Metric Data Traffic", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("bytes"), Max::Of(Stat::Sent(Traffic::Data)), sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "OutTrafficMetadata", "Outbound Metric Metadata Traffic", FieldType::Integer, Visual::Value, Transform::Number,
        0, Some("bytes"), Max::Of(Stat::Sent(Traffic::Metadata)), sort::DESCENDING, Summary::Sum, Filter::Range,
        opts::NONE,
    ),
    col(
        "OutTrafficReplication", "Outbound Metric Replication Traffic", FieldType::Integer, Visual::Value,
        Transform::Number, 0, Some("bytes"), Max::Of(Stat::Sent(Traffic::Replication)), sort::DESCENDING, Summary::Sum,
        Filter::Range, opts::NONE,
    ),
    col(
        "OutTrafficFunctions", "Outbound Metric Functions Traffic", FieldType::Integer, Visual::Value,
        Transform::Number, 0, Some("bytes"), Max::Of(Stat::Sent(Traffic::Functions)), sort::DESCENDING, Summary::Sum,
        Filter::Range, opts::NONE,
    ),
    col(
        "OutAttemptHandshake", "Outbound Connection Attempt Handshake Status", FieldType::Array, Visual::Pill,
        Transform::None, 0, None, Max::None, sort::ASCENDING, Summary::Count, Filter::MultiSelect, opts::NONE,
    ),
    col(
        "OutAttemptSince", "Last Outbound Connection Attempt Status Change Time", FieldType::Timestamp, Visual::Value,
        Transform::DatetimeMs, 0, None, Max::Of(Stat::OutAttemptSince), sort::DESCENDING, Summary::Max, Filter::None,
        opts::NONE,
    ),
    col(
        "OutAttemptAge", "Last Outbound Connection Attempt Status Change Age", FieldType::Duration, Visual::Value,
        Transform::DurationS, 0, None, Max::Of(Stat::OutAttemptAge), sort::ASCENDING, Summary::Min, Filter::Range,
        opts::VISIBLE,
    ),
    col(
        "MlAnomalous", "Number of Anomalous Metrics", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("metrics"), Max::Ml, sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "MlNormal", "Number of Not Anomalous Metrics", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("metrics"), Max::Ml, sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "MlTrained", "Number of Trained Metrics", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("metrics"), Max::Ml, sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "MlPending", "Number of Pending Metrics", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("metrics"), Max::Ml, sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    col(
        "MlSilenced", "Number of Silenced Metrics", FieldType::Integer, Visual::Value, Transform::Number, 0,
        Some("metrics"), Max::Ml, sort::DESCENDING, Summary::Sum, Filter::Range, opts::NONE,
    ),
    text("MachineGUID", "Machine GUID", opts::NONE),
    text("NodeID", "Cloud Node ID", opts::NONE),
];

/// `charts`: a column and its title each, each chart a stacked bar of its one column (`F.c:918-959`).
const CHARTS: [(&str, &str); 3] =
    [("InAge", "Data Collection Age"), ("OutAge", "Streaming Age"), ("dbDuration", "Retention Duration")];

/// `group_by`: a column and its title each (`F.c:975-1008`).
const GROUP_BY: [(&str, &str); 29] = [
    ("OSName", "O/S Name"),
    ("OSId", "O/S ID"),
    ("OSIdLike", "O/S ID Like"),
    ("OSVersion", "O/S Version"),
    ("OSVersionId", "O/S Version ID"),
    ("OSDetection", "O/S Detection"),
    ("CPUCores", "CPU Cores"),
    ("ContainerOSName", "Container O/S Name"),
    ("ContainerOSId", "Container O/S ID"),
    ("ContainerOSIdLike", "Container O/S ID Like"),
    ("ContainerOSVersion", "Container O/S Version"),
    ("ContainerOSVersionId", "Container O/S Version ID"),
    ("ContainerOSDetection", "Container O/S Detection"),
    ("IsK8sNode", "Kubernetes Nodes"),
    ("KernelName", "Kernel Name"),
    ("KernelVersion", "Kernel Version"),
    ("Architecture", "Architecture"),
    ("Virtualization", "Virtualization Technology"),
    ("VirtDetection", "Virtualization Detection"),
    ("Container", "Container"),
    ("ContainerDetection", "Container Detection"),
    ("CloudProviderType", "Cloud Provider Type"),
    ("CloudInstanceType", "Cloud Instance Type"),
    ("CloudInstanceRegion", "Cloud Instance Region"),
    ("InStatus", "Collection Status"),
    ("OutStatus", "Streaming Status"),
    ("MlStatus", "ML Status"),
    ("InRemoteIP", "Inbound IP"),
    ("OutRemoteIP", "Outbound IP"),
];

/// `function_streaming_topology()`: `info` as any word after the name answers the metadata at once; the topology
/// itself is not ported.
pub(super) fn topology(reply: &mut Reply, function: &[u8], _: Option<&Payload>, _: &[u8]) -> u16 {
    let info = quoted_strings_splitter(function, 1024, Separators::Whitespace).iter().skip(1).any(|w| w == b"info");
    if !info {
        return not_implemented(reply);
    }
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_uint64("status", 200);
    w.member_add_string("type", "topology");
    w.member_add_time_t("update_every", UPDATE_EVERY);
    w.member_add_boolean("has_history", false);
    w.member_add_string("help", STREAMING_TOPOLOGY_HELP);
    w.member_add_array(Some(b"accepted_params"));
    w.add_array_item_string("info");
    w.array_close();
    w.member_add_array(Some(b"required_params"));
    w.array_close();
    w.member_add_time_t("expires", now_realtime_s() + UPDATE_EVERY);
    json_reply(w, reply)
}

#[cfg(test)]
mod tests {
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_rrd::mode::DbMode;
    use netdata_agent_rrd::status::{
        Db, DbLiveness, DbStatus, DyncfgStatus, Ingest, Ml, MlStatus, MlType, ParentStatus, Replication, SocketPeers,
        Stream,
    };

    use super::*;

    const GUID: &str = "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e";

    fn host(hostname: &str) -> Host {
        Host::new(
            GUID,
            true,
            HostInfo {
                hostname: hostname.into(),
                registry_hostname: hostname.into(),
                os: "linux".into(),
                timezone: "UTC".into(),
                abbrev_timezone: "UTC".into(),
                utc_offset: 0,
                program_name: "netdata".into(),
                program_version: "v0".into(),
                update_every: 1,
                db_mode: DbMode::Ram,
                history_entries: 4096,
                health_enabled: false,
                system_info: Default::default(),
                replication_enabled: false,
                replication_period: 0,
                replication_step: 0,
                stream_send: None,
                cache_dir: None,
            },
        )
    }

    /// Localhost of an agent that collects nothing and streams nowhere, asked at 1791312192, started at 1791312184.
    fn fresh() -> Status {
        Status {
            now: 1_791_312_192,
            db: Db {
                status: DbStatus::Initializing,
                liveness: DbLiveness::Stale,
                mode: DbMode::Dbengine,
                first_time_s: 0,
                last_time_s: 1_791_312_192,
                metrics: 0,
                instances: 0,
                contexts: 0,
            },
            ingest: Ingest {
                id: 0,
                hops: 0,
                kind: IngestType::Localhost,
                status: IngestStatus::Initializing,
                since_s: 1_791_312_184,
                reason: 0,
                metrics: 0,
                instances: 0,
                contexts: 0,
                replication: Replication::default(),
                capabilities: 0,
                peers: SocketPeers::default(),
                tls: false,
            },
            stream: Stream {
                id: 0,
                hops: 1,
                status: StreamStatus::Disabled,
                since_s: 1_791_312_184,
                reason: 0,
                replication: Replication::default(),
                capabilities: 0,
                peers: SocketPeers::default(),
                tls: false,
                compression: false,
                sent_bytes: [0; 4],
                parents: Vec::new(),
            },
            ml: Ml { status: MlStatus::Disabled, kind: MlType::Disabled },
            dyncfg: DyncfgStatus::Available,
        }
    }

    /// The rows of `rows`, each a host and its status, as the table's `data`, with the maxima they raised.
    fn rendered(rows: &[(&Host, &Status)]) -> (String, Maxima) {
        let mut w = JsonWriter::new(JsonOptions::MINIFY);
        let mut max = Maxima::default();
        w.member_add_array(Some(b"data"));
        for (host, s) in rows {
            row(&mut w, host, s, &mut max);
        }
        w.array_close();
        (String::from_utf8_lossy(w.as_bytes()).into_owned(), max)
    }

    /// One row's cells by column key.
    fn cells(host: &Host, s: &Status) -> impl Fn(&str) -> serde_json::Value + use<> {
        let (text, _) = rendered(&[(host, s)]);
        let doc: serde_json::Value = serde_json::from_str(&format!("{text}}}")).unwrap();
        let row = doc["data"][0].as_array().unwrap().clone();
        assert_eq!(row.len(), COLUMNS.len());
        move |key| row[COLUMNS.iter().position(|c| c.key == key).unwrap()].clone()
    }

    /// The cells of keys, as one JSON array.
    fn pick(cell: &impl Fn(&str) -> serde_json::Value, keys: &[&str]) -> serde_json::Value {
        serde_json::Value::Array(keys.iter().map(|key| cell(key)).collect())
    }

    /// `F.c:73-333` on a fresh agent's localhost: the row the plan of commit 12 read off C (section 1.7, "What a fresh
    /// agent prints"), the 27 system-info texts empty, both statuses' ages at the status's clock, the parents' and
    /// ML's cells null, the node id null while it is zero.
    #[test]
    fn a_fresh_agent_s_row_is_cs() {
        let (text, _) = rendered(&[(&host("box"), &fresh())]);
        let info = vec![r#""""#; 27].join(",");
        assert_eq!(
            text,
            format!(concat!(
                r#"{{"data":[["box",{{"severity":"normal"}},"permanent","netdata","v0",{info},"#,
                r#"0,1791312192000,null,0,0,0,"initializing","disabled","disabled","#,
                r#"0,1791312184000,8,"LOCALHOST",0,0,0,"localhost",0,"",0,"#,
                r#""PLAIN",[],0,0,0,0,1791312184000,8,"NEVER CONNECTED",1,0,0,"",0,"",0,"PLAIN","UNCOMPRESSED",[],"#,
                r#"0,0,0,0,[],null,null,null,null,null,null,null,"{GUID}",null]]"#
            ), info = info, GUID = GUID)
        );
    }

    /// The columns, the default sort, the charts and the groupings byte for byte as C printed them in the
    /// streaming check's table of three hosts (`TestFnNetdataStreaming`, 2026-10-08), with the maxima C's rows raised
    /// there: every column's arguments, `max` left out for NaN, the completions' 100 and the ML counts' 0 over null
    /// cells, the Collected columns at the database's maxima.
    #[test]
    fn the_columns_and_the_charts_are_cs() {
        let max = Maxima {
            db_from: 1_791_438_074,
            db_to: 1_791_438_087,
            db_duration: 26,
            db_metrics: 73,
            db_instances: 14,
            db_contexts: 13,
            in_connections: 1,
            in_since: 1_791_438_083_000,
            in_age: 35,
            in_hops: 2,
            in_local_port: 38231,
            in_remote_port: 36916,
            out_since: 1_791_438_052_000,
            out_age: 35,
            out_hops: 3,
            ..Maxima::default()
        };
        let mut w = JsonWriter::new(JsonOptions::DEFAULT);
        w.member_add_array(Some(b"data"));
        w.array_close();
        columns_and_charts(&mut w, &max);
        w.member_add_time_t("expires", 0);
        let text = w.as_bytes();
        let start = text.windows(10).position(|b| b == b"\"columns\":").unwrap();
        let end = text.windows(9).position(|b| b == b"\"expires\"").unwrap();
        let golden = include_bytes!("../../tests/golden/netdata-streaming-columns-master-prod.txt");
        assert_eq!(String::from_utf8_lossy(&text[start..end]), String::from_utf8_lossy(golden));
    }

    /// The maxima as C raises them: the hops from -1, so a table without a row prints -1; only a cell that is printed
    /// raises one (a since of 0, a duration of a host without retention); the largest of two rows.
    #[test]
    fn the_maxima_follow_c() {
        let none = Maxima::default();
        assert_eq!((Max::Of(Stat::InHops).value(&none), Max::Of(Stat::OutHops).value(&none)), (-1.0, -1.0));
        let empty = Status {
            db: Db { first_time_s: 0, last_time_s: 0, ..fresh().db },
            ingest: Ingest { since_s: 0, ..fresh().ingest },
            ..fresh()
        };
        let mut big = fresh();
        big.db.first_time_s = 1_791_312_100;
        big.db.metrics = 7;
        big.ingest.hops = 2;
        big.stream.hops = -3;
        big.stream.peers = SocketPeers { local_ip: "a".into(), local_port: 9, peer_ip: "b".into(), peer_port: 19999 };
        let (text, max) = rendered(&[(&host("a"), &empty), (&host("b"), &big)]);
        assert!(text.contains(r#""a",{"severity":"normal"}"#) && text.contains(",0,0,null,0,0,0,"), "{text}");
        assert_eq!(
            (max.db_from, max.db_to, max.db_duration, max.db_metrics, max.in_since, max.in_age),
            (1_791_312_100, 1_791_312_192, 92, 7, 1_791_312_184_000, 8)
        );
        assert_eq!((max.in_hops, max.out_hops, max.out_remote_port), (2, 1, 19999));
        assert_eq!(Max::Of(Stat::DbFrom).value(&max), 1_791_312_100_000.0);
    }

    /// The cells that depend on the host's kind and its sender (`F.c:101-329`): a vnode's local reason and address; a
    /// connected child's socket, TLS and capabilities; an offline child's stored reason and empty ends; a sender's
    /// cells, its traffic as data, metadata, replication, functions, its parents' last handshakes and the newest
    /// attempt; the severities, the stream's NO PARENT TO SEND TO exception and an ephemeral host's normal.
    #[test]
    fn the_cells_follow_the_host_s_kind() {
        let vnode = Status { ingest: Ingest { kind: IngestType::Virtual, ..fresh().ingest }, ..fresh() };
        let cell = cells(&host("v"), &vnode);
        assert_eq!(pick(&cell, &["InReason", "InLocalIP"]), serde_json::json!(["VIRTUAL NODE", "localhost"]));

        let child = Status {
            ingest: Ingest {
                id: 2,
                hops: 1,
                kind: IngestType::Child,
                status: IngestStatus::Online,
                reason: 1,
                replication: Replication { in_progress: false, completion: 87.5, instances: 3 },
                capabilities: caps::VCAPS | caps::HLABELS,
                peers: SocketPeers {
                    local_ip: "10.0.0.1".into(),
                    local_port: 19999,
                    peer_ip: "10.0.0.2".into(),
                    peer_port: 40000,
                },
                tls: true,
                ..fresh().ingest
            },
            ..fresh()
        };
        let cell = cells(&host("c"), &child);
        assert_eq!(
            pick(
                &cell,
                &[
                    "InConnections", "InReason", "InHops", "InReplCompletion", "InReplInstances", "InLocalIP",
                    "InLocalPort",
                ]
            ),
            serde_json::json!([2, "CONNECTED", 1, 87.5, 3, "10.0.0.1", 19999])
        );
        assert_eq!(
            pick(&cell, &["InRemoteIP", "InRemotePort", "InSSL", "InCapabilities"]),
            serde_json::json!(["10.0.0.2", 40000, "SSL", ["VCAPS", "HLABELS"]])
        );
        assert_eq!(cell("rowOptions"), serde_json::json!({"severity": "normal"}));

        let offline = Status {
            ingest: Ingest {
                kind: IngestType::Child,
                status: IngestStatus::Offline,
                reason: Reason::DISCONNECT_SOCKET_WRITE_FAILED.0,
                ..fresh().ingest
            },
            ..fresh()
        };
        let cell = cells(&host("o"), &offline);
        assert_eq!(
            pick(&cell, &["InReason", "InLocalIP", "InLocalPort", "rowOptions"]),
            serde_json::json!([
                "DISCONNECTED SOCKET WRITE FAILED",
                "",
                0,
                {"severity": "critical"}
            ])
        );
        let ephemeral = host("e");
        ephemeral.set_ephemeral(true);
        let cell = cells(&ephemeral, &offline);
        assert_eq!(
            pick(&cell, &["Ephemerality", "rowOptions"]),
            serde_json::json!(["ephemeral", {"severity": "normal"}])
        );

        let parent = |reason: Reason, since_ut| ParentStatus { reason: reason.0, since_ut, ..Default::default() };
        let sender = Status {
            stream: Stream {
                id: 4,
                status: StreamStatus::Offline,
                since_s: 1_791_312_180,
                reason: Reason::SP_CONNECTION_REFUSED.0,
                peers: SocketPeers {
                    local_ip: "not connected".into(),
                    local_port: 0,
                    peer_ip: "not connected".into(),
                    peer_port: 0,
                },
                compression: true,
                tls: true,
                capabilities: caps::VCAPS,
                sent_bytes: [1, 2, 3, 4],
                parents: vec![
                    parent(Reason::SP_CONNECTION_REFUSED, 1_791_312_190_500_000),
                    parent(Reason::NEVER, 1_791_312_180_000_000),
                ],
                ..fresh().stream
            },
            ..fresh()
        };
        let cell = cells(&host("s"), &sender);
        assert_eq!(
            pick(
                &cell,
                &[
                    "OutConnections", "OutSince", "OutAge", "OutReason", "OutLocalIP", "OutSSL", "OutCompression",
                ]
            ),
            serde_json::json!([
                4,
                1_791_312_180_000u64,
                12,
                "CONNECTION REFUSED",
                "not connected",
                "SSL",
                "COMPRESSED"
            ])
        );
        assert_eq!(
            pick(&cell, &["OutTrafficData", "OutTrafficMetadata", "OutTrafficReplication", "OutTrafficFunctions"]),
            serde_json::json!([4, 3, 1, 2])
        );
        assert_eq!(
            pick(&cell, &["OutCapabilities", "OutAttemptHandshake", "OutAttemptSince", "OutAttemptAge", "rowOptions"]),
            serde_json::json!([
                ["VCAPS"],
                ["CONNECTION REFUSED", "NEVER CONNECTED"],
                1_791_312_190_500u64,
                2,
                {"severity": "warning"}
            ])
        );
        let alone = Status {
            stream: Stream { reason: Reason::SP_NO_DESTINATION.0, ..sender.stream.clone() },
            ..fresh()
        };
        assert_eq!(cells(&host("n"), &alone)("rowOptions"), serde_json::json!({"severity": "normal"}));
    }
}
