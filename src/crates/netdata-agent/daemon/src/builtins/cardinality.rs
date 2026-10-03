//! `netdata-metrics-cardinality` (`src/web/api/functions/function-metrics-cardinality.c`): the instances and
//! time-series of every context of every host, counted collected or archived, grouped by context or by hostname.

use indexmap::IndexMap;
use netdata_agent_nrpc::reply::Reply;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::Hosts;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};
use netdata_agent_text::rrdf::{self, Field, FieldType, Filter, Summary, Transform, Visual, opts, sort};

use super::{METRICS_CARDINALITY_HELP, json_reply};

/// `struct counts`.
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    nodes: u64,
    instances: u64,
    metrics: u64,
    online_nodes: u64,
    online_instances: u64,
    online_metrics: u64,
    offline_nodes: u64,
    offline_instances: u64,
    offline_metrics: u64,
}

impl Counts {
    fn add(&mut self, c: &Counts) {
        self.nodes += c.nodes;
        self.online_nodes += c.online_nodes;
        self.offline_nodes += c.offline_nodes;
        self.instances += c.instances;
        self.online_instances += c.online_instances;
        self.offline_instances += c.offline_instances;
        self.metrics += c.metrics;
        self.online_metrics += c.online_metrics;
        self.offline_metrics += c.offline_metrics;
    }

    fn max(&mut self, c: &Counts) {
        self.nodes = self.nodes.max(c.nodes);
        self.instances = self.instances.max(c.instances);
        self.metrics = self.metrics.max(c.metrics);
        self.online_nodes = self.online_nodes.max(c.online_nodes);
        self.online_instances = self.online_instances.max(c.online_instances);
        self.online_metrics = self.online_metrics.max(c.online_metrics);
        self.offline_nodes = self.offline_nodes.max(c.offline_nodes);
        self.offline_instances = self.offline_instances.max(c.offline_instances);
        self.offline_metrics = self.offline_metrics.max(c.offline_metrics);
    }
}

/// A share in percent, 0 for an empty whole.
fn share(part: u64, whole: u64) -> f64 {
    if whole > 0 { part as f64 * 100.0 / whole as f64 } else { 0.0 }
}

/// The header: the members, the accepted parameter and the select that sets it.
fn header(w: &mut JsonWriter, hostname: &str) {
    w.member_add_string("hostname", hostname);
    w.member_add_uint64("status", 200);
    w.member_add_string("type", "table");
    w.member_add_time_t("update_every", 10);
    w.member_add_boolean("has_history", false);
    w.member_add_string("help", METRICS_CARDINALITY_HELP);
    w.member_add_array(Some(b"accepted_params"));
    w.add_array_item_string("group");
    w.array_close();
    w.member_add_array(Some(b"required_params"));
    w.add_array_item_object();
    w.member_add_string("id", "group");
    w.member_add_string("name", "Grouping");
    w.member_add_string("help", "Select how to group the metrics");
    w.member_add_boolean("unique_view", true);
    w.member_add_string("type", "select");
    w.member_add_array(Some(b"options"));
    for (id, name) in [("by-context", "Group by Context"), ("by-node", "Group by Node")] {
        w.add_array_item_object();
        w.member_add_string("id", id);
        w.member_add_string("name", name);
        w.object_close();
    }
    w.array_close();
    w.object_close();
    w.array_close();
}

/// The counts of every context, keyed by context or hostname, in first-seen order, and their totals.
fn count(hosts: &Hosts, by_node: bool) -> (IndexMap<String, Counts>, Counts) {
    let mut rows: IndexMap<String, Counts> = IndexMap::new();
    let mut all = Counts::default();
    for host in hosts.all() {
        let online = host.is_online();
        let hostname = host.hostname();
        for rc in host.contexts().all() {
            let mut c = Counts { nodes: 1, ..Counts::default() };
            if online {
                c.online_nodes += 1;
            } else {
                c.offline_nodes += 1;
            }
            for ri in rc.instances() {
                c.instances += 1;
                if ri.flags.is_collected() {
                    c.online_instances += 1;
                    for rm in ri.metrics() {
                        c.metrics += 1;
                        if rm.flags.is_collected() {
                            c.online_metrics += 1;
                        } else {
                            c.offline_metrics += 1;
                        }
                    }
                } else {
                    c.offline_instances += 1;
                    let metrics = ri.metrics().len() as u64;
                    c.metrics += metrics;
                    c.offline_metrics += metrics;
                }
            }
            let key = if by_node { hostname.clone() } else { rc.id().to_string() };
            rows.entry(key).or_default().add(&c);
            all.add(&c);
        }
    }
    (rows, all)
}

/// A count column.
fn count_column<'a>(id: usize, key: &'a str, name: &'a str, units: &'a [u8], max: u64, visible: bool) -> Field<'a> {
    Field {
        id,
        key: key.as_bytes(),
        name: name.as_bytes(),
        kind: FieldType::Integer,
        visual: Visual::Value,
        transform: Transform::Number,
        decimal_points: 0,
        units: Some(units),
        max: max as f64,
        sort: sort::DESCENDING,
        pointer_to: None,
        summary: Summary::Sum,
        filter: Filter::Range,
        options: if visible { opts::VISIBLE } else { opts::NONE },
        default_value: None,
    }
}

/// A percentage column.
fn share_column<'a>(id: usize, key: &'a str, name: &'a str, visible: bool) -> Field<'a> {
    Field {
        visual: Visual::Bar,
        decimal_points: 2,
        max: 100.0,
        summary: Summary::Max,
        ..count_column(id, key, name, b"%", 0, visible)
    }
}

/// `function_metrics_cardinality()`: `group:by-node` or `group:by-context` (the last wins) after the name; `info`
/// answers the header alone.
pub(super) fn render(hosts: &Hosts, reply: &mut Reply, function: &[u8]) -> u16 {
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    header(&mut w, &hosts.localhost().hostname());
    let mut by_node = false;
    for word in quoted_strings_splitter(function, 1024, Separators::Whitespace).iter().skip(1) {
        match word.as_slice() {
            b"group:by-node" => by_node = true,
            b"group:by-context" => by_node = false,
            b"info" => return json_reply(w, reply),
            _ => {}
        }
    }
    let (rows, all) = count(hosts, by_node);
    let mut max = Counts::default();
    w.member_add_array(Some(b"data"));
    for (key, c) in &rows {
        w.add_array_item_array();
        w.add_array_item_string(key);
        if !by_node {
            w.add_array_item_uint64(c.nodes);
            w.add_array_item_uint64(c.online_nodes);
            w.add_array_item_uint64(c.offline_nodes);
        }
        for v in [c.instances, c.online_instances, c.offline_instances, c.metrics, c.online_metrics, c.offline_metrics] {
            w.add_array_item_uint64(v);
        }
        for v in [
            share(c.offline_instances, c.instances),
            share(c.offline_metrics, c.metrics),
            share(c.instances, all.instances),
            share(c.online_instances, all.online_instances),
            share(c.offline_instances, all.offline_instances),
            share(c.metrics, all.metrics),
            share(c.online_metrics, all.online_metrics),
            share(c.offline_metrics, all.offline_metrics),
        ] {
            w.add_array_item_double(v);
        }
        w.array_close();
        max.max(c);
    }
    w.array_close();
    w.member_add_object("columns");
    let first = Field {
        id: 0,
        key: if by_node { b"Hostname" } else { b"Context" },
        name: if by_node { b"Hostname" } else { b"Context Name" },
        kind: FieldType::String,
        visual: Visual::Value,
        transform: Transform::None,
        decimal_points: 0,
        units: None,
        max: f64::NAN,
        sort: sort::ASCENDING,
        pointer_to: None,
        summary: Summary::Count,
        filter: Filter::None,
        options: opts::FULL_WIDTH | opts::UNIQUE_KEY | opts::VISIBLE,
        default_value: None,
    };
    let mut columns = vec![first];
    if !by_node {
        columns.push(count_column(0, "All Nodes", "Number of Nodes", b"nodes", max.nodes, false));
        columns.push(count_column(0, "Curr. Nodes", "Number of Online Nodes", b"nodes", max.online_nodes, true));
        columns.push(count_column(0, "Old Nodes", "Number of Offline Nodes", b"nodes", max.offline_nodes, true));
    }
    columns.extend([
        count_column(0, "All Instances", "Total Number of Instances", b"instances", max.instances, false),
        count_column(
            0,
            "Curr. Instances",
            "Total Number of Currently Collected Instances",
            b"instances",
            max.online_instances,
            true,
        ),
        count_column(0, "Old Instances", "Total Number of Archived Instances", b"instances", max.offline_instances, true),
        count_column(0, "All Dimensions", "Total Number of Time-Series", b"metrics", max.metrics, true),
        count_column(
            0,
            "Curr. Dimensions",
            "Total Number of Currently Collected Time-Series",
            b"metrics",
            max.online_metrics,
            false,
        ),
        count_column(0, "Old Dimensions", "Total Number of Archived Time-Series", b"metrics", max.offline_metrics, false),
        share_column(0, "Ephemeral Instances", "Percentage of Archived Instances vs All Instances of the row", true),
        share_column(
            0,
            "Ephemeral Dimensions",
            "Percentage of Archived Time-Series vs All Time-Series of the row",
            false,
        ),
        share_column(
            0,
            "All Instances %",
            "Percentage of All Instances of row vs the sum of All Instances across all rows",
            true,
        ),
        share_column(
            0,
            "Curr. Instances %",
            "Percentage of Currently Collected Instances of row vs the sum of Currently Collected Instances across all \
             rows",
            false,
        ),
        share_column(
            0,
            "Old Instances %",
            "Percentage of Old Instances of row vs the sum of Old Instances across all rows",
            true,
        ),
        share_column(
            0,
            "All Dimensions %",
            "Percentage of All Time-Series of row vs the sum of All Time-Series across all rows",
            false,
        ),
        share_column(
            0,
            "Curr. Dimensions %",
            "Percentage of Currently Collected Time-Series of row vs the sum of Currently Collected Time-Series across \
             all rows",
            false,
        ),
        share_column(
            0,
            "Old Dimensions %",
            "Percentage of Archived Time-Series of row vs the sum of Archived Time-Series across all rows",
            false,
        ),
    ]);
    for (id, field) in columns.iter_mut().enumerate() {
        field.id = id;
        rrdf::add_field(&mut w, field);
    }
    w.object_close();
    w.member_add_string("default_sort_column", "Old Instances");
    w.member_add_object("charts");
    for (name, kind, cols) in [
        ("Instances Ephemerality", "doughnut", &["Curr. Instances", "Old Instances"][..]),
        ("Dimensions Ephemerality", "doughnut", &["Curr. Dimensions", "Old Dimensions"][..]),
        ("Instances Total", "value", &["All Instances"][..]),
        ("Dimensions Total", "value", &["All Dimensions"][..]),
    ] {
        w.member_add_object(name);
        w.member_add_array(Some(b"columns"));
        for col in cols {
            w.add_array_item_string(col);
        }
        w.array_close();
        w.member_add_string("name", name);
        w.member_add_string("type", kind);
        w.member_add_string("groupBy", "all");
        w.member_add_string("aggregation", "sum");
        w.object_close();
    }
    w.object_close();
    w.member_add_array(Some(b"default_charts"));
    for name in ["Instances Ephemerality", "Dimensions Ephemerality"] {
        w.add_array_item_array();
        w.add_array_item_string(name);
        w.array_close();
    }
    w.array_close();
    w.member_add_time_t("expires", now_realtime_s() + 1);
    json_reply(w, reply)
}
