//! The v2 JSON wrapper around a result (`src/web/api/formatters/jsonwrap-v2.c`), with the v2 summaries
//! (`jsonwrap-summary-*.c`), the detailed tree (`jsonwrap-objects-tree.c`), `buffer_json_agents_v2()` and
//! `buffer_json_cloud_timings()`. Health is not ported, so every alert count is 0 and alert lists are empty.
//! Spec §9.8.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use netdata_agent_rrd::chart::ID_LENGTH_MAX;
use netdata_agent_rrd::host::Host;
use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::STORAGE_TIERS;
use crate::finalize::DVIEW_ANOMALY_COUNT_MULTIPLIER;
use crate::format::exposed;
use crate::groupby::{MAX_PASSES, aggregatable, has_percentage_units};
use crate::jsonwrap::query_timings;
use crate::keys::Keys;
use crate::rrdr::Rrdr;
use crate::tables::{group_by, group_by_names, options, options_to_json_array};
use crate::target::{Counts, QueryTarget, metric_status};
use crate::window::Window;

/// `MCP_QUERY_INFO_SUMMARY_SECTION`, `_DATABASE_SECTION`, `_VIEW_SECTION` (`src/web/mcp/mcp.h`).
const MCP_INFO_SUMMARY: &str = "The summary section breaks down the different sources that contribute data to the \
query. Use this to detect spikes, dives, anomalies (the % of anomalous samples vs the total samples) and evaluate the \
different groupings that may be beneficial for the task at hand.";
const MCP_INFO_DATABASE: &str = "The database section provides metadata about the underlying data storage, including \
retention periods and update frequencies, and data availability across different storage tiers.";
const MCP_INFO_VIEW: &str = "The view section provides summarized data for the visible time window. For each \
dimension returned, it contains the minimum, maximum, and average values, the anomaly rate (% of anomalous samples vs \
total samples) and contribution percentages, across all points.";

/// The agent answering (`localhost`): the `agents` member describes it.
#[derive(Debug, Clone, Copy)]
pub struct Agent<'a> {
    pub machine_guid: &'a str,
    pub node_id: [u8; 16],
    pub hostname: &'a str,
}

/// `struct summary_total_counts`.
#[derive(Debug, Default, Clone, Copy)]
struct Totals {
    selected: u64,
    excluded: u64,
    queried: u64,
    failed: u64,
}

impl Totals {
    /// `aggregate_into_summary_totals()`.
    fn add(&mut self, m: &Counts) {
        if m.selected > 0 {
            self.selected += 1;
            if m.queried > 0 {
                self.queried += 1;
            } else if m.failed > 0 {
                self.failed += 1;
            }
        } else {
            self.excluded += 1;
        }
    }
}

/// `aggregate_metrics_counts()` / `aggregate_instances_counts()`.
fn add_counts(dst: &mut Counts, src: &Counts) {
    dst.selected += src.selected;
    dst.excluded += src.excluded;
    dst.queried += src.queried;
    dst.failed += src.failed;
}

/// The wrapper's state: the query, its window and the key table.
struct Ctx<'a> {
    qt: &'a QueryTarget,
    window: &'a Window,
    k: Keys,
}

impl Ctx<'_> {
    fn minimal(&self) -> bool {
        self.window.options & options::MINIMAL_STATS != 0
    }

    /// `query_target_summary_cardinality_limit()`.
    fn summary_limit(&self) -> usize {
        if self.window.options & options::CARDINALITY_LIMIT_ALL != 0 {
            usize::try_from(self.qt.request.cardinality_limit).unwrap_or(usize::MAX)
        } else {
            0
        }
    }

    /// The share of the query's positive sum (`qt->query_points.sum`), 0 when that is not positive.
    fn contribution(&self, sum: f64) -> f64 {
        if self.qt.query_points.sum > 0.0 {
            sum * 100.0 / self.qt.query_points.sum
        } else {
            0.0
        }
    }

    /// `query_target_metric_counts()` (`key` = dimensions) and `query_target_instance_counts()` (instances):
    /// the non-zero members only, nothing when all are 0.
    fn counts(&self, w: &mut JsonWriter, key: &str, c: &Counts) {
        if c.selected == 0 && c.queried == 0 && c.failed == 0 && c.excluded == 0 {
            return;
        }
        let k = self.k;
        w.member_add_object(key);
        for (name, value) in [
            (k.selected(), c.selected),
            (k.excluded(), c.excluded),
            (k.queried(), c.queried),
            (k.failed(), c.failed),
        ] {
            if value > 0 {
                w.member_add_uint64(name, u64::from(value));
            }
        }
        w.object_close();
    }

    /// `query_target_total_counts()`.
    fn total_counts(&self, w: &mut JsonWriter, key: &str, t: &Totals) {
        if t.selected == 0 && t.queried == 0 && t.failed == 0 && t.excluded == 0 {
            return;
        }
        let k = self.k;
        w.member_add_object(key);
        for (name, value) in [
            (k.selected(), t.selected),
            (k.excluded(), t.excluded),
            (k.queried(), t.queried),
            (k.failed(), t.failed),
        ] {
            if value > 0 {
                w.member_add_uint64(name, value);
            }
        }
        w.object_close();
    }

    /// `query_target_points_statistics()`: nothing for an empty point.
    fn points_statistics(&self, w: &mut JsonWriter, sp: &StoragePoint) {
        if sp.count == 0 {
            return;
        }
        let k = self.k;
        w.member_add_object(k.statistics());
        w.member_add_double("min", sp.min);
        w.member_add_double("max", sp.max);
        if aggregatable(self.window.options) {
            w.member_add_uint64(k.count(), u64::from(sp.count));
            w.member_add_double("sum", sp.sum);
            w.member_add_double(k.volume(), sp.sum * self.window.view_update_every() as f64);
            w.member_add_uint64(k.anomaly_count(), u64::from(sp.anomaly_count));
        } else {
            let avg = if sp.count > 0 {
                sp.sum / f64::from(sp.count)
            } else {
                0.0
            };
            w.member_add_double("avg", avg);
            w.member_add_double(k.anomaly_rate(), sp.anomaly_rate());
            w.member_add_double(k.contribution(), self.contribution(sp.sum));
        }
        w.object_close();
    }

    fn node(&self, w: &mut JsonWriter, n: usize, show_status: bool) {
        let qn = &self.qt.nodes[n];
        node_add_v2(w, self.k, &qn.host, n, qn.duration_ut, show_status);
    }

    /// `query_target_summary_nodes_v2()`.
    fn summary_nodes(&self, w: &mut JsonWriter, totals: &mut Totals) {
        let (qt, k) = (self.qt, self.k);
        let show_status = qt.request.options & options::MINIMAL_STATS == 0;
        let limit = self.summary_limit();
        let node = |w: &mut JsonWriter, n: usize| {
            let qn = &qt.nodes[n];
            w.add_array_item_object();
            self.node(w, n, show_status);
            if !self.minimal() {
                self.counts(w, k.instances(), &qn.instances);
                self.counts(w, k.dimensions(), &qn.metrics);
            }
            self.points_statistics(w, &qn.query_points);
            w.object_close();
        };
        w.member_add_array(Some(b"nodes"));
        if limit > 0 && qt.nodes.len() > limit {
            let mut items: Vec<(usize, f64, String)> = (0..qt.nodes.len())
                .map(|n| {
                    (
                        n,
                        self.contribution(qt.nodes[n].query_points.sum),
                        qt.nodes[n].host.hostname(),
                    )
                })
                .collect();
            items.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| a.2.as_bytes().cmp(b.2.as_bytes()))
            });
            let (mut remaining, mut contribution) = (0, 0.0);
            let (mut metrics, mut instances) = (Counts::default(), Counts::default());
            let mut points = StoragePoint::UNSET;
            for (i, &(n, share, _)) in items.iter().enumerate() {
                let qn = &qt.nodes[n];
                if i < limit - 1 {
                    node(w, n);
                } else {
                    contribution += share;
                    remaining += 1;
                    add_counts(&mut metrics, &qn.metrics);
                    add_counts(&mut instances, &qn.instances);
                    points.merge_to(&qn.query_points);
                }
                totals.add(&qn.metrics);
            }
            if remaining > 0 {
                w.add_array_item_object();
                w.member_add_string("id", "__remaining_nodes__");
                w.member_add_string(k.hostname(), format!("remaining {remaining} nodes"));
                w.member_add_double(k.contribution(), contribution);
                if !self.minimal() {
                    self.counts(w, k.instances(), &instances);
                    self.counts(w, k.dimensions(), &metrics);
                }
                self.points_statistics(w, &points);
                w.object_close();
            }
        } else {
            for n in 0..qt.nodes.len() {
                node(w, n);
                totals.add(&qt.nodes[n].metrics);
            }
        }
        w.array_close();
    }

    /// `query_target_summary_contexts_v2()`: one entry per context id, first seen first; returns their count.
    fn summary_contexts(&self, w: &mut JsonWriter, totals: &mut Totals) -> usize {
        let k = self.k;
        let mut order: Vec<&str> = Vec::new();
        let mut entries: HashMap<&str, (Counts, Counts, StoragePoint)> = HashMap::new();
        for qc in &self.qt.contexts {
            let id = qc.rc.id();
            let e = entries.entry(id).or_insert_with(|| {
                order.push(id);
                (
                    Counts::default(),
                    Counts::default(),
                    StoragePoint::default(),
                )
            });
            add_counts(&mut e.0, &qc.instances);
            add_counts(&mut e.1, &qc.metrics);
            e.2.merge_to(&qc.query_points);
        }
        w.member_add_array(Some(b"contexts"));
        for id in &order {
            let (instances, metrics, points) = &entries[id];
            w.add_array_item_object();
            w.member_add_string("id", id);
            if !self.minimal() {
                self.counts(w, k.instances(), instances);
                self.counts(w, k.dimensions(), metrics);
            }
            self.points_statistics(w, points);
            w.object_close();
            totals.add(metrics);
        }
        w.array_close();
        order.len()
    }

    /// `query_target_summary_instances_v2()`.
    fn summary_instances(&self, w: &mut JsonWriter, totals: &mut Totals) {
        let (qt, k) = (self.qt, self.k);
        let limit = self.summary_limit();
        let instance = |w: &mut JsonWriter, i: usize| {
            let qi = &qt.instances[i];
            let (id, name) = (qi.ri.id(), qi.ri.state().name);
            w.add_array_item_object();
            w.member_add_string("id", id);
            if id != name {
                w.member_add_string(k.name(), &name);
            }
            w.member_add_uint64(k.node_index(), qt.contexts[qi.context].node as u64);
            let share = self.contribution(qi.query_points.sum);
            if share > 0.0 {
                w.member_add_double(k.contribution(), share);
            }
            if !self.minimal() {
                self.counts(w, k.dimensions(), &qi.metrics);
            }
            self.points_statistics(w, &qi.query_points);
            w.object_close();
        };
        w.member_add_array(Some(b"instances"));
        if limit > 0 && qt.instances.len() > limit {
            let mut items: Vec<(usize, f64)> = (0..qt.instances.len())
                .map(|i| (i, self.contribution(qt.instances[i].query_points.sum)))
                .collect();
            items.sort_by(|a, b| {
                b.1.partial_cmp(&a.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        qt.instances[a.0]
                            .ri
                            .id()
                            .as_bytes()
                            .cmp(qt.instances[b.0].ri.id().as_bytes())
                    })
            });
            let (mut remaining, mut contribution) = (0, 0.0);
            let mut metrics = Counts::default();
            let mut points = StoragePoint::UNSET;
            for (n, &(i, share)) in items.iter().enumerate() {
                let qi = &qt.instances[i];
                if n < limit - 1 {
                    instance(w, i);
                } else {
                    contribution += share;
                    remaining += 1;
                    add_counts(&mut metrics, &qi.metrics);
                    points.merge_to(&qi.query_points);
                }
                totals.add(&qi.metrics);
            }
            if remaining > 0 {
                w.add_array_item_object();
                w.member_add_string("id", "__remaining_instances__");
                w.member_add_string(k.name(), format!("remaining {remaining} instances"));
                if contribution > 0.0 {
                    w.member_add_double(k.contribution(), contribution);
                }
                if !self.minimal() {
                    self.counts(w, k.dimensions(), &metrics);
                }
                self.points_statistics(w, &points);
                w.object_close();
            }
        } else {
            for i in 0..qt.instances.len() {
                instance(w, i);
                totals.add(&qt.instances[i].metrics);
            }
        }
        w.array_close();
    }

    /// `query_target_summary_dimensions_v12(v2 = true)`: one entry per metric name (else id), sorted by priority
    /// then name, or by sum when the summary limit applies.
    fn summary_dimensions(&self, w: &mut JsonWriter, totals: &mut Totals) {
        let (qt, k) = (self.qt, self.k);
        struct Entry {
            key: String,
            points: StoragePoint,
            metrics: Counts,
            priority: u32,
        }
        let mut entries: Vec<Entry> = Vec::new();
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut q = 0;
        for qd in &qt.dimensions {
            let mut qm = None;
            while q < qt.query.len()
                && Arc::ptr_eq(&qt.dimensions[qt.query[q].dimension].rm, &qd.rm)
            {
                qm = Some(&qt.query[q]);
                q += 1;
            }
            let name = qd.rm.state().name;
            let key = if name.is_empty() {
                qd.rm.id().to_string()
            } else {
                name
            };
            if key.is_empty() {
                continue;
            }
            let priority = qd.priority as u32;
            let at = *index.entry(key.clone()).or_insert_with(|| {
                entries.push(Entry {
                    key,
                    points: StoragePoint::default(),
                    metrics: Counts::default(),
                    priority,
                });
                entries.len() - 1
            });
            let e = &mut entries[at];
            if priority < e.priority {
                e.priority = priority;
            }
            match qm {
                Some(qm) => {
                    e.metrics.selected += u32::from(qm.status & metric_status::SELECTED != 0);
                    e.metrics.failed += u32::from(qm.status & metric_status::FAILED != 0);
                    if qm.status & metric_status::QUERIED != 0 {
                        e.metrics.queried += 1;
                        e.points.merge_to(&qm.query_points);
                    }
                }
                None => e.metrics.excluded += 1,
            }
        }
        let limit = self.summary_limit();
        if limit > 0 && entries.len() > limit {
            entries.sort_by(|a, b| {
                b.points
                    .sum
                    .partial_cmp(&a.points.sum)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(a.priority.cmp(&b.priority))
                    .then_with(|| a.key.as_bytes().cmp(b.key.as_bytes()))
            });
        } else {
            entries.sort_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then_with(|| a.key.as_bytes().cmp(b.key.as_bytes()))
            });
        }
        w.member_add_array(Some(b"dimensions"));
        let (mut remaining, mut contribution) = (0, 0.0);
        let mut metrics = Counts::default();
        let mut points = StoragePoint::UNSET;
        for (n, e) in entries.iter().enumerate() {
            totals.add(&e.metrics);
            if limit > 0 && n + 1 > limit - 1 {
                remaining += 1;
                if qt.query_points.sum > 0.0 {
                    contribution += self.contribution(e.points.sum);
                }
                add_counts(&mut metrics, &e.metrics);
                points.merge_to(&e.points);
                continue;
            }
            w.add_array_item_object();
            w.member_add_string("id", &e.key);
            if !self.minimal() {
                self.counts(w, k.dimensions(), &e.metrics);
            }
            self.points_statistics(w, &e.points);
            w.member_add_uint64(k.priority(), u64::from(e.priority));
            w.object_close();
        }
        if remaining > 0 {
            w.add_array_item_object();
            w.member_add_string("id", "__remaining_dimensions__");
            w.member_add_string(k.name(), format!("remaining {remaining} dimensions"));
            w.member_add_double(k.contribution(), contribution);
            if !self.minimal() {
                self.counts(w, k.dimensions(), &metrics);
            }
            self.points_statistics(w, &points);
            w.object_close();
        }
        w.array_close();
    }

    /// `query_target_summary_labels_v12(v2 = true)`: per label key, its values with the counts and points of the
    /// instances carrying them.
    fn summary_labels(
        &self,
        w: &mut JsonWriter,
        key_totals: &mut Totals,
        value_totals: &mut Totals,
    ) {
        let (qt, k) = (self.qt, self.k);
        struct Value {
            value: Vec<u8>,
            points: StoragePoint,
            metrics: Counts,
        }
        struct Key {
            name: Vec<u8>,
            values: Vec<Value>,
            seen: HashMap<Vec<u8>, usize>,
            points: StoragePoint,
            metrics: Counts,
        }
        let mut keys: Vec<Key> = Vec::new();
        for qi in &qt.instances {
            for label in qi.ri.labels().iter() {
                let at = match keys.iter().position(|key| key.name == label.name) {
                    Some(at) => at,
                    None => {
                        keys.push(Key {
                            name: label.name.clone(),
                            values: Vec::new(),
                            seen: HashMap::new(),
                            points: StoragePoint::default(),
                            metrics: Counts::default(),
                        });
                        keys.len() - 1
                    }
                };
                let key = &mut keys[at];
                let mut pair = label.name.clone();
                pair.push(b':');
                pair.extend_from_slice(&label.value);
                pair.truncate(ID_LENGTH_MAX * 2 - 1);
                let v = *key.seen.entry(pair).or_insert_with(|| {
                    key.values.push(Value {
                        value: label.value.clone(),
                        points: StoragePoint::default(),
                        metrics: Counts::default(),
                    });
                    key.values.len() - 1
                });
                add_counts(&mut key.values[v].metrics, &qi.metrics);
                add_counts(&mut key.metrics, &qi.metrics);
                key.values[v].points.merge_to(&qi.query_points);
                key.points.merge_to(&qi.query_points);
            }
        }
        let limit = self.summary_limit();
        w.member_add_array(Some(b"labels"));
        for key in &mut keys {
            w.add_array_item_object();
            w.member_add_string("id", &key.name);
            if !self.minimal() {
                self.counts(w, k.dimensions(), &key.metrics);
            }
            self.points_statistics(w, &key.points);
            key_totals.add(&key.metrics);
            w.member_add_array(Some(k.label_values().as_bytes()));
            let limited = limit > 0 && key.values.len() > limit;
            if limited {
                key.values.sort_by(|a, b| {
                    b.points
                        .sum
                        .partial_cmp(&a.points.sum)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| {
                            let pa = [&key.name[..], b":", &a.value].concat();
                            let pb = [&key.name[..], b":", &b.value].concat();
                            pa.cmp(&pb)
                        })
                });
            }
            let (mut remaining, mut metrics, mut points) =
                (0, Counts::default(), StoragePoint::UNSET);
            for (n, v) in key.values.iter().enumerate() {
                value_totals.add(&v.metrics);
                if limited && n + 1 > limit - 1 {
                    remaining += 1;
                    add_counts(&mut metrics, &v.metrics);
                    points.merge_to(&v.points);
                    continue;
                }
                self.label_value(w, &v.value, None, &v.metrics, &v.points);
            }
            if remaining > 0 {
                let name = format!("remaining {remaining} values");
                self.label_value(
                    w,
                    b"__remaining_values__",
                    Some(name.as_bytes()),
                    &metrics,
                    &points,
                );
            }
            w.array_close();
            w.object_close();
        }
        w.array_close();
    }

    /// `output_label_value()`.
    fn label_value(
        &self,
        w: &mut JsonWriter,
        id: &[u8],
        name: Option<&[u8]>,
        metrics: &Counts,
        points: &StoragePoint,
    ) {
        w.add_array_item_object();
        w.member_add_string("id", id);
        if let Some(name) = name {
            w.member_add_string(self.k.name(), name);
        }
        if !self.minimal() {
            self.counts(w, self.k.dimensions(), metrics);
        }
        self.points_statistics(w, points);
        w.object_close();
    }

    /// `query_target_detailed_objects_tree()`: node → context → instance → dimension, the queried dimensions
    /// only unless `all-dimensions`.
    fn detailed(&self, w: &mut JsonWriter, now_s: i64) {
        let (qt, k, opts) = (self.qt, self.k, self.window.options);
        let rfc3339 = opts & options::RFC3339 != 0;
        w.member_add_object(b"nodes");
        let (mut last_node, mut last_context, mut last_instance) = (None, None, None);
        let mut q = 0;
        for qd in &qt.dimensions {
            let mut qm = None;
            let mut queried = false;
            while q < qt.query.len()
                && Arc::ptr_eq(&qt.dimensions[qt.query[q].dimension].rm, &qd.rm)
            {
                queried = qt.query[q].status & metric_status::QUERIED != 0;
                qm = Some(&qt.query[q]);
                q += 1;
            }
            if !queried && opts & options::ALL_DIMENSIONS == 0 {
                continue;
            }
            let i = qd.instance;
            let c = qt.instances[i].context;
            let n = qt.contexts[c].node;
            if last_node != Some(n) {
                if last_node.is_some() {
                    if last_context.is_some() {
                        if last_instance.take().is_some() {
                            w.object_close();
                            w.object_close();
                        }
                        w.object_close();
                        w.object_close();
                        last_context = None;
                    }
                    w.object_close();
                    w.object_close();
                }
                let qn = &qt.nodes[n];
                w.member_add_object(qn.host.machine_guid());
                if let Some(node_id) = &qn.node_id {
                    w.member_add_string(k.node_id(), node_id);
                }
                w.member_add_uint64(k.node_index(), n as u64);
                w.member_add_string(k.hostname(), qn.host.hostname());
                w.member_add_object(b"contexts");
                last_node = Some(n);
            }
            if last_context != Some(c) {
                if last_context.is_some() {
                    if last_instance.take().is_some() {
                        w.object_close();
                        w.object_close();
                    }
                    w.object_close();
                    w.object_close();
                }
                w.member_add_object(qt.contexts[c].rc.id());
                w.member_add_object(b"instances");
                last_context = Some(c);
            }
            if last_instance != Some(i) {
                if last_instance.is_some() {
                    w.object_close();
                    w.object_close();
                }
                let ri = &qt.instances[i].ri;
                let state = ri.state();
                w.member_add_object(ri.id());
                w.member_add_string(k.name(), &state.name);
                w.member_add_time_t(k.update_every(), i64::from(state.update_every_s));
                w.member_add_object(b"labels");
                ri.labels().to_json_members(w);
                w.object_close();
                w.member_add_object(b"dimensions");
                last_instance = Some(i);
            }
            let state = qd.rm.state();
            w.member_add_object(qd.rm.id());
            w.member_add_string(k.name(), &state.name);
            w.member_add_uint64(k.queried(), u64::from(queried));
            w.member_add_time_t_formatted(k.first_entry(), state.first_time_s, rfc3339);
            // rrdmetric_acquired_last_entry(): 0 while collected, which prints as now.
            let last_entry = if qd.rm.flags.is_collected() {
                0
            } else {
                state.last_time_s
            };
            let last = if last_entry != 0 { last_entry } else { now_s };
            w.member_add_time_t_formatted(k.last_entry(), last, rfc3339);
            if let Some(qm) = qm {
                if qm.status & metric_status::GROUPED != 0 {
                    w.member_add_string("as", &qm.grouped_as.name);
                }
                self.points_statistics(w, &qm.query_points);
                if opts & options::DEBUG != 0 {
                    crate::jsonwrap::query_metric_plan(w, qm, opts);
                }
            }
            w.object_close();
        }
        if last_node.is_some() {
            if last_context.is_some() {
                if last_instance.is_some() {
                    w.object_close();
                    w.object_close();
                }
                w.object_close();
                w.object_close();
            }
            w.object_close();
            w.object_close();
        }
        w.object_close();
    }

    /// `query_target_combined_units_v2()`.
    fn combined_units(&self, w: &mut JsonWriter, contexts: usize, ignore_percentage: bool) {
        let qt = self.qt;
        if !ignore_percentage && has_percentage_units(qt, self.window.options) {
            w.member_add_string("units", "%");
        } else if contexts == 1 {
            w.member_add_string("units", qt.contexts[0].rc.state().units);
        } else if contexts > 1 {
            let mut units: Vec<String> = Vec::new();
            for qc in &qt.contexts {
                let u = qc.rc.state().units;
                if !units.contains(&u) {
                    units.push(u);
                }
            }
            if units.len() == 1 {
                w.member_add_string("units", qt.contexts[0].rc.state().units);
            } else {
                w.member_add_array(Some(b"units"));
                for u in &units {
                    w.add_array_item_string(u);
                }
                w.array_close();
            }
        }
    }

    /// `rrdr_dimension_query_points_statistics()` over `dqp` (db) or `dview` (view).
    fn column_statistics(&self, w: &mut JsonWriter, r: &Rrdr, dview: bool) {
        let points = if dview { &r.dview } else { &r.dqp };
        if points.is_empty() {
            return;
        }
        let (k, opts) = (self.k, self.window.options);
        let multiplier = if dview {
            DVIEW_ANOMALY_COUNT_MULTIPLIER
        } else {
            1.0
        };
        let shown: Vec<&StoragePoint> = (0..r.columns)
            .filter(|&c| exposed(r.od[c], opts))
            .map(|c| &points[c])
            .collect();
        let array = |w: &mut JsonWriter, key: &str, f: &dyn Fn(&mut JsonWriter, &StoragePoint)| {
            w.member_add_array(Some(key.as_bytes()));
            for sp in &shown {
                f(w, sp);
            }
            w.array_close();
        };
        w.member_add_object(k.statistics());
        array(w, "min", &|w, sp| w.add_array_item_double(sp.min));
        array(w, "max", &|w, sp| w.add_array_item_double(sp.max));
        if opts & options::RETURN_RAW != 0 {
            array(w, "sum", &|w, sp| w.add_array_item_double(sp.sum));
            array(w, k.count(), &|w, sp| {
                w.add_array_item_uint64(u64::from(sp.count))
            });
            array(w, k.anomaly_count(), &|w, sp| {
                // C passes the double to a uint64 parameter.
                w.add_array_item_uint64(
                    (sp.anomaly_rate() / multiplier / 100.0 * f64::from(sp.count)) as u64,
                )
            });
        } else {
            let mut sum = 0.0;
            for sp in &shown {
                sum += sp.sum.abs();
            }
            array(w, "avg", &|w, sp| {
                w.add_array_item_double(sp.average_value())
            });
            array(w, k.anomaly_rate(), &|w, sp| {
                w.add_array_item_double(sp.anomaly_rate() / multiplier)
            });
            array(w, k.contribution(), &|w, sp| {
                w.add_array_item_double(if sum > 0.0 {
                    sp.sum.abs() * 100.0 / sum
                } else {
                    0.0
                })
            });
        }
        w.object_close();
    }

    /// A per-column array of the exposed columns.
    fn column_array(
        &self,
        w: &mut JsonWriter,
        key: &str,
        r: &Rrdr,
        item: &dyn Fn(&mut JsonWriter, usize),
    ) {
        w.member_add_array(Some(key.as_bytes()));
        for c in 0..r.columns {
            if exposed(r.od[c], self.window.options) {
                item(w, c);
            }
        }
        w.array_close();
    }

    /// `rrdr_grouped_by_array_v2()`: the deepest pass's groupings.
    fn grouped_by(&self, w: &mut JsonWriter) {
        let qt = self.qt;
        let mut g = 0;
        while g < MAX_PASSES && qt.request.group_by[g].group_by != group_by::NONE {
            g += 1;
        }
        let g = g.saturating_sub(1);
        let gb = qt.request.group_by[g].group_by;
        w.member_add_array(Some(b"grouped_by"));
        if gb & group_by::SELECTED != 0 {
            w.add_array_item_string("selected");
        } else if gb & group_by::PERCENTAGE_OF_INSTANCE != 0 {
            w.add_array_item_string("percentage-of-instance");
        } else {
            if gb & group_by::DIMENSION != 0 {
                w.add_array_item_string("dimension");
            }
            if gb & group_by::INSTANCE != 0 {
                w.add_array_item_string("instance");
            }
            if gb & group_by::LABEL != 0 {
                for key in &qt.group_by_label_keys[g] {
                    w.add_array_item_string([&b"label:"[..], key].concat());
                }
            }
            if gb & group_by::NODE != 0 {
                w.add_array_item_string("node");
            }
            if gb & group_by::CONTEXT != 0 {
                w.add_array_item_string("context");
            }
            if gb & group_by::UNITS != 0 {
                w.add_array_item_string("units");
            }
        }
        w.array_close();
    }

    /// `rrdr_dimension_units_array_v2()`.
    fn column_units(&self, w: &mut JsonWriter, r: &Rrdr, ignore_percentage: bool) {
        if r.du.is_empty() {
            return;
        }
        let percentage = !ignore_percentage && has_percentage_units(self.qt, self.window.options);
        self.column_array(w, "units", r, &|w, c| {
            w.add_array_item_string(if percentage { "%" } else { r.du[c].as_str() })
        });
    }

    /// `rrdr_json_group_by_labels()`.
    fn group_by_labels(&self, w: &mut JsonWriter, r: &Rrdr) {
        let (Some(keys), Some(dl)) = (r.label_keys.as_ref(), r.dl.as_ref()) else {
            return;
        };
        w.member_add_object(b"labels");
        for key in keys {
            w.member_add_array(Some(key));
            for (labels, &od) in dl.iter().zip(&r.od) {
                if !exposed(od, self.window.options) {
                    continue;
                }
                match labels.iter().find(|(k, _)| k == key) {
                    Some((_, values)) => {
                        w.add_array_item_array();
                        for v in values {
                            w.add_array_item_string(v);
                        }
                        w.array_close();
                    }
                    None => w.add_array_item_null(),
                }
            }
            w.array_close();
        }
        w.object_close();
    }
}

/// `rrdr_json_wrapper_begin2()`: a new writer holding every member before `result`, and the number of unique
/// contexts (`r->internal.contexts`) the end needs.
pub fn begin_v2(qt: &QueryTarget, window: &Window) -> (JsonWriter, usize) {
    let opts = window.options;
    let (kq, sq): (&[u8], &[u8]) = if opts & options::GOOGLE_JSON != 0 {
        (b"", b"'")
    } else {
        (b"\"", b"\"")
    };
    let json_options = if opts & options::MINIFY != 0 {
        JsonOptions::MINIFY
    } else {
        JsonOptions::DEFAULT
    };
    let mut w = JsonWriter::with_quotes(kq, sq, 0, true, json_options);
    let x = Ctx {
        qt,
        window,
        k: Keys::new(opts),
    };
    let req = &qt.request;
    w.member_add_uint64("api", u64::from(req.version));
    if opts & options::DEBUG != 0 {
        let rfc3339 = req.options & options::RFC3339 != 0;
        w.member_add_string("id", &qt.id);
        w.member_add_object(b"request");
        w.member_add_string("format", req.format.name());
        options_to_json_array(&mut w, b"options", req.options);
        w.member_add_object(b"scope");
        w.member_add_string_opt("scope_nodes", req.scope_nodes.as_deref());
        w.member_add_string_opt("scope_contexts", req.scope_contexts.as_deref());
        w.object_close();
        w.member_add_object(b"selectors");
        w.member_add_string_opt("nodes", req.nodes.as_deref());
        w.member_add_string_opt("contexts", req.contexts.as_deref());
        w.member_add_string_opt("instances", req.instances.as_deref());
        w.member_add_string_opt("dimensions", req.dimensions.as_deref());
        w.member_add_string_opt("labels", req.labels.as_deref());
        w.member_add_string_opt("alerts", req.alerts.as_deref());
        w.object_close();
        w.member_add_object(b"window");
        w.member_add_time_t_formatted("after", req.after, rfc3339);
        w.member_add_time_t_formatted("before", req.before, rfc3339);
        w.member_add_uint64("points", req.points);
        if req.options & options::SELECTED_TIER != 0 {
            w.member_add_uint64("tier", req.tier);
        } else {
            w.member_add_null("tier");
        }
        w.object_close();
        w.member_add_object(b"aggregations");
        w.member_add_object(b"time");
        w.member_add_string("time_group", req.time_group.name());
        w.member_add_string_opt("time_group_options", req.time_group_options.as_deref());
        if req.resampling_time > 0 {
            w.member_add_time_t("time_resampling", req.resampling_time);
        } else {
            w.member_add_null("time_resampling");
        }
        w.object_close();
        w.member_add_array(Some(b"metrics"));
        for g in 0..MAX_PASSES {
            let pass = &req.group_by[g];
            if pass.group_by == group_by::NONE {
                break;
            }
            w.add_array_item_object();
            w.member_add_array(Some(b"group_by"));
            for name in group_by_names(pass.group_by) {
                w.add_array_item_string(name);
            }
            w.array_close();
            w.member_add_array(Some(b"group_by_label"));
            for key in &qt.group_by_label_keys[g] {
                w.add_array_item_string(key);
            }
            w.array_close();
            w.member_add_string("aggregation", pass.aggregation.name());
            w.object_close();
        }
        w.array_close();
        w.object_close();
        // C prints the int through a uint64 parameter.
        w.member_add_uint64("timeout", i64::from(req.timeout_ms) as u64);
        w.object_close();
    }
    if opts & options::MINIMAL_STATS == 0 {
        w.member_add_object(b"versions");
        w.member_add_uint64("routing_hard_hash", 1);
        let v = &qt.versions;
        w.member_add_uint64("nodes_hard_hash", v.nodes_hard_hash);
        w.member_add_uint64("contexts_hard_hash", v.contexts_hard_hash);
        w.member_add_uint64("contexts_soft_hash", v.contexts_soft_hash);
        w.member_add_uint64("alerts_hard_hash", 0);
        w.member_add_uint64("alerts_soft_hash", 0);
        w.object_close();
    }
    let mut totals: [Totals; 6] = Default::default();
    w.member_add_object(b"summary");
    if opts & options::MCP_INFO != 0 {
        w.member_add_string("info", MCP_INFO_SUMMARY);
    }
    x.summary_nodes(&mut w, &mut totals[0]);
    let contexts = x.summary_contexts(&mut w, &mut totals[1]);
    x.summary_instances(&mut w, &mut totals[2]);
    x.summary_dimensions(&mut w, &mut totals[3]);
    let (keys, values) = totals.split_at_mut(5);
    x.summary_labels(&mut w, &mut keys[4], &mut values[0]);
    if opts & options::MINIMAL_STATS == 0 {
        // Health is not ported: no alert is linked to any instance.
        w.member_add_array(Some(b"alerts"));
        w.array_close();
    }
    if aggregatable(opts) {
        w.member_add_object(b"globals");
        x.points_statistics(&mut w, &qt.query_points);
        w.object_close();
    }
    w.object_close();
    if opts & options::MINIMAL_STATS == 0 {
        w.member_add_object(b"totals");
        for (key, t) in [
            "nodes",
            "contexts",
            "instances",
            "dimensions",
            "label_keys",
            "label_key_values",
        ]
        .iter()
        .zip(totals.iter())
        {
            x.total_counts(&mut w, key, t);
        }
        w.object_close();
    }
    if opts & options::SHOW_DETAILS != 0 {
        w.member_add_object(b"detailed");
        x.detailed(&mut w, crate::now_s());
        w.object_close();
    }
    if opts & options::MINIMAL_STATS == 0 {
        w.member_add_array(Some(b"functions"));
        w.array_close();
    }
    (w, contexts)
}

/// `rrdr_json_wrapper_end2()`.
pub fn end_v2(
    w: &mut JsonWriter,
    r: &Rrdr,
    qt: &QueryTarget,
    window: &Window,
    contexts: usize,
    received: Instant,
    agent: &Agent<'_>,
) {
    let opts = window.options;
    let x = Ctx {
        qt,
        window,
        k: Keys::new(opts),
    };
    let rfc3339 = opts & options::RFC3339 != 0;
    w.member_add_object(b"db");
    if opts & options::MCP_INFO != 0 {
        w.member_add_string("info", MCP_INFO_DATABASE);
    }
    w.member_add_uint64("tiers", STORAGE_TIERS);
    w.member_add_time_t("update_every", qt.db.minimum_latest_update_every_s);
    w.member_add_time_t_formatted("first_entry", qt.db.first_time_s, rfc3339);
    w.member_add_time_t_formatted("last_entry", qt.db.last_time_s, rfc3339);
    x.combined_units(w, contexts, true);
    w.member_add_object(b"dimensions");
    x.column_array(w, "ids", r, &|w, c| w.add_array_item_string(&r.di[c]));
    x.column_units(w, r, true);
    x.column_statistics(w, r, false);
    w.object_close();
    w.member_add_array(Some(b"per_tier"));
    w.add_array_item_object();
    w.member_add_uint64("tier", 0);
    w.member_add_uint64("queries", qt.db.tier0_queries as u64);
    w.member_add_uint64("points", qt.db.tier0_points as u64);
    w.member_add_time_t("update_every", qt.db.tier0_update_every);
    w.member_add_time_t_formatted("first_entry", qt.db.tier0_first, rfc3339);
    w.member_add_time_t_formatted("last_entry", qt.db.tier0_last, rfc3339);
    w.object_close();
    w.array_close();
    w.object_close();

    w.member_add_object(b"view");
    if opts & options::MCP_INFO != 0 {
        w.member_add_string("info", MCP_INFO_VIEW);
    }
    if contexts == 1 {
        w.member_add_string("title", qt.contexts[0].rc.state().title);
    } else if contexts > 1 {
        let mut title = b"Chart for contexts: ".to_vec();
        let mut seen: Vec<&str> = Vec::new();
        for qc in &qt.contexts {
            let id = qc.rc.id();
            if !seen.contains(&id) {
                if !seen.is_empty() {
                    title.extend_from_slice(b", ");
                }
                title.extend_from_slice(id.as_bytes());
                seen.push(id);
            }
        }
        w.member_add_string("title", title);
    }
    w.member_add_time_t("update_every", r.view.update_every);
    w.member_add_time_t_formatted("after", r.view.after, rfc3339);
    w.member_add_time_t_formatted("before", r.view.before, rfc3339);
    if opts & options::DEBUG != 0 {
        w.member_add_string("format", qt.request.format.name());
        options_to_json_array(w, b"options", opts);
        w.member_add_string("time_group", qt.request.time_group.name());
    }
    if opts & (options::DEBUG | options::RETURN_RAW) != 0 {
        w.member_add_object(b"partial_data_trimming");
        w.member_add_time_t("max_update_every", r.trimming.max_update_every);
        w.member_add_time_t_formatted("expected_after", r.trimming.expected_after, rfc3339);
        w.member_add_time_t_formatted("trimmed_after", r.trimming.trimmed_after, rfc3339);
        w.object_close();
    }
    if r.cardinality_folded != 0 {
        w.member_add_object(b"cardinality");
        w.member_add_uint64("folded", r.cardinality_folded as u64);
        w.member_add_double("cut", r.cardinality_cut);
        w.object_close();
    }
    if opts & options::RETURN_RAW != 0 {
        w.member_add_uint64("points", r.rows as u64);
    }
    x.combined_units(w, contexts, false);
    if contexts >= 1 {
        w.member_add_string("chart_type", qt.contexts[0].rc.state().chart_type.name());
    }
    w.member_add_object(b"dimensions");
    x.grouped_by(w);
    x.column_array(w, "ids", r, &|w, c| w.add_array_item_string(&r.di[c]));
    x.column_array(w, "names", r, &|w, c| w.add_array_item_string(&r.dn[c]));
    x.column_units(w, r, false);
    if !r.dp.is_empty() {
        x.column_array(w, "priorities", r, &|w, c| {
            w.add_array_item_uint64(r.dp[c] as u64)
        });
    }
    if !r.dgbc.is_empty() {
        x.column_array(w, "aggregated", r, &|w, c| {
            w.add_array_item_uint64(u64::from(r.dgbc[c]))
        });
    }
    x.column_statistics(w, r, true);
    x.group_by_labels(w, r);
    w.object_close();
    w.member_add_double("min", r.view.min);
    w.member_add_double("max", r.view.max);
    w.object_close();

    if opts & options::MINIMAL_STATS == 0 {
        let finished = Instant::now();
        w.member_add_array(Some(b"agents"));
        w.add_array_item_object();
        w.member_add_string("mg", agent.machine_guid);
        w.member_add_uuid("nd", &agent.node_id);
        w.member_add_string("nm", agent.hostname);
        w.member_add_time_t_formatted("now", crate::now_s(), rfc3339);
        w.member_add_uint64("ai", 0);
        query_timings(w, "timings", received, finished, qt);
        w.object_close();
        w.array_close();
        cloud_timings(w, "timings", received, finished);
    }
    w.finalize();
}

/// `buffer_json_node_add_v2()` with its status (`buffer_json_agent_status_id()`): a node's identity in v2 answers.
pub fn node_add_v2(
    w: &mut JsonWriter,
    k: Keys,
    host: &Host,
    ni: usize,
    duration_ut: u64,
    show_status: bool,
) {
    w.member_add_string(k.machine_guid(), host.machine_guid());
    let node_id = host.node_id();
    if node_id != [0; 16] {
        w.member_add_uuid(k.node_id(), &node_id);
    }
    w.member_add_string(k.hostname(), host.hostname());
    w.member_add_uint64(k.node_index(), ni as u64);
    if show_status {
        w.member_add_object(k.status());
        w.member_add_uint64(k.agent_index(), 0);
        w.member_add_uint64("code", 200);
        w.member_add_string("msg", "");
        if duration_ut != 0 {
            w.member_add_double("ms", duration_ut as f64 / 1000.0);
        }
        w.object_close();
    }
}

/// `buffer_json_cloud_timings()`: no routing here, only the total.
pub fn cloud_timings(w: &mut JsonWriter, key: &str, received: Instant, finished: Instant) {
    w.member_add_object(key);
    w.member_add_double("routing_ms", 0.0);
    w.member_add_double("node_max_ms", 0.0);
    w.member_add_double(
        "total_ms",
        finished.saturating_duration_since(received).as_micros() as f64 / 1000.0,
    );
    w.object_close();
}
