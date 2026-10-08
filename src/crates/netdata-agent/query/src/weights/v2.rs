//! The answer of `/api/v2/weights` and `/api/v3/weights` (`src/web/api/queries/weights.c`): the members every
//! multinode answer starts with (`results_header_to_json_v2()`, the versions, the schema), then one of two
//! formats: every result as a row with the rollups of its instance, context and node, and the dictionaries the
//! rows index (`registered_results_to_json_multinode_no_group_by()`), or the results grouped by what the request
//! names (`registered_results_to_json_multinode_group_by()`).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use netdata_agent_rrd::host::Host;
use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_text::json::JsonWriter;
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::uuid_lower_text;

use super::Method;
use super::engine::Finished;
use super::results::Registered;
use super::v1::{db_counters, footer, writer};
use crate::jsonwrap::timings;
use crate::jsonwrap_v2::{Agent, agents_v2, node_add_v2, version_hashes_v2};
use crate::keys::Keys;
use crate::tables::{Aggregation, group_by, group_by_names, options, options_to_json_array};
use crate::target::Versions;

/// What the writers take from the agent: who answers, and how many tiers its storage has.
pub struct Answering<'a> {
    pub agent: Agent<'a>,
    pub storage_tiers: usize,
}

/// The two methods that compare with a baseline: their answers carry its window and its points.
fn has_baseline(method: Method) -> bool {
    matches!(method, Method::Ks2 | Method::Volume)
}

fn or_all(text: &Option<Vec<u8>>) -> &[u8] {
    text.as_deref().unwrap_or(b"*")
}

/// `results_header_to_json_v2()`: the request as the engine left it (the texts as given, `*` for one not given),
/// the view and what the queries read.
fn header(w: &mut JsonWriter, finished: &Finished, storage_tiers: usize, grouped: bool) {
    let req = &finished.request;
    let rfc3339 = req.options & options::RFC3339 != 0;
    let baseline = has_baseline(req.method);
    let texts = &req.texts;

    w.member_add_object("request");
    w.member_add_string("method", req.method.name());
    options_to_json_array(w, b"options", req.options);
    w.member_add_object("scope");
    w.member_add_string("scope_nodes", or_all(&texts.scope_nodes));
    w.member_add_string("scope_contexts", or_all(&texts.scope_contexts));
    w.member_add_string("scope_instances", or_all(&texts.scope_instances));
    w.member_add_string("scope_labels", or_all(&texts.scope_labels));
    w.object_close();
    w.member_add_object("selectors");
    w.member_add_string("nodes", or_all(&texts.nodes));
    w.member_add_string("contexts", or_all(&texts.contexts));
    w.member_add_string("instances", or_all(&texts.instances));
    w.member_add_string("dimensions", or_all(&texts.dimensions));
    w.member_add_string("labels", or_all(&texts.labels));
    w.member_add_string("alerts", or_all(&texts.alerts));
    w.object_close();
    w.member_add_object("window");
    w.member_add_time_t_formatted("after", req.after, rfc3339);
    w.member_add_time_t_formatted("before", req.before, rfc3339);
    w.member_add_uint64("points", req.points);
    if req.options & options::SELECTED_TIER != 0 {
        w.member_add_uint64("tier", req.tier);
    } else {
        w.member_add_null("tier");
    }
    w.object_close();
    if baseline {
        w.member_add_object("baseline");
        w.member_add_time_t_formatted("baseline_after", req.baseline_after, rfc3339);
        w.member_add_time_t_formatted("baseline_before", req.baseline_before, rfc3339);
        w.object_close();
    }
    w.member_add_object("aggregations");
    w.member_add_object("time");
    w.member_add_string("time_group", req.time_group.name());
    w.member_add_string_opt("time_group_options", req.time_group_options.as_deref());
    w.object_close();
    w.member_add_array(Some(b"metrics"));
    w.add_array_item_object();
    w.member_add_array(Some(b"group_by"));
    for name in group_by_names(req.group_by.group_by) {
        w.add_array_item_string(name);
    }
    w.array_close();
    w.member_add_string("aggregation", req.group_by.aggregation.name());
    w.object_close();
    w.array_close();
    w.object_close();
    // a `time_t` printed as unsigned; the engine left none below a second
    w.member_add_uint64("timeout", req.timeout_ms as u64);
    w.object_close();

    w.member_add_object("view");
    w.member_add_string("format", if grouped { "grouped" } else { "full" });
    w.member_add_string("time_group", req.time_group.name());
    w.member_add_object("window");
    w.member_add_time_t_formatted("after", req.after, rfc3339);
    w.member_add_time_t_formatted("before", req.before, rfc3339);
    w.member_add_time_t("duration", req.before - req.after);
    w.member_add_uint64("points", req.points);
    w.object_close();
    if baseline {
        w.member_add_object("baseline");
        w.member_add_time_t_formatted("after", req.baseline_after, rfc3339);
        w.member_add_time_t_formatted("before", req.baseline_before, rfc3339);
        w.member_add_time_t("duration", req.baseline_before - req.baseline_after);
        w.member_add_uint64("points", req.points.checked_shl(finished.shifts).unwrap_or(0));
        w.object_close();
    }
    w.object_close();

    w.member_add_object("db");
    db_counters(w, finished, storage_tiers);
    w.object_close();
}

/// What both formats write before their results: `api`, the header, the versions (the walk's sums, and the
/// host index's version as it is now) and the schema of a row or of a group's `v`.
fn begin(finished: &Finished, by: &Answering, grouped: bool) -> JsonWriter {
    let mut w = writer(finished);
    w.member_add_uint64("api", 2);
    header(&mut w, finished, by.storage_tiers, grouped);
    let nodes_hard_hash = (by.agent.nodes_hard_hash)();
    version_hashes_v2(&mut w, &Versions { nodes_hard_hash, ..finished.versions });
    let key = if grouped { "v_schema" } else { "schema" };
    schema(&mut w, key, has_baseline(finished.request.method), grouped);
    w
}

/// What both formats write after their results: the agent with the request's timings (C never marks the end of
/// a preparation, so `prep_ms` is 0 and the query runs from the start to the end of the engine's work), the two
/// counts and the limit's summary.
fn end(mut w: JsonWriter, finished: &Finished, by: &Answering, unit: &str, total: usize, returned: usize) -> Vec<u8> {
    let rfc3339 = finished.request.options & options::RFC3339 != 0;
    let received = finished.received;
    let executed = received.checked_add(Duration::from_micros(finished.duration_us)).unwrap_or(received);
    agents_v2(&mut w, by.agent, crate::now_s(), rfc3339, true, |w| {
        timings(w, "timings", received, received, executed, Instant::now());
    });
    footer(&mut w, finished, unit, total, returned);
    w.finalize();
    w.into_bytes()
}

fn schema_item(w: &mut JsonWriter, name: &str, kind: &str, dictionary: Option<&str>) {
    w.add_array_item_object();
    w.member_add_string("name", name);
    w.member_add_string("type", kind);
    if let Some(dictionary) = dictionary {
        w.member_add_string("dictionary", dictionary);
    }
    w.object_close();
}

fn schema_labels(w: &mut JsonWriter, name: &str, labels: &[&str]) {
    w.member_add_string("name", name);
    w.member_add_string("type", "array");
    w.member_add_array(Some(b"labels"));
    for label in labels {
        w.add_array_item_string(label);
    }
    w.array_close();
}

fn schema_timeframe(w: &mut JsonWriter, name: &str) {
    w.add_array_item_object();
    schema_labels(w, name, &["min", "avg", "max", "sum", "count", "anomaly_count"]);
    w.member_add_object("calculations");
    w.member_add_string("anomaly rate", "anomaly_count * 100 / count");
    w.object_close();
    w.object_close();
}

/// `multinode_data_schema()`: what the items of a row are, or of a group's `v`.
fn schema(w: &mut JsonWriter, key: &str, baseline: bool, grouped: bool) {
    w.member_add_object(key);
    w.member_add_string("type", "array");
    w.member_add_array(Some(b"items"));
    if grouped {
        w.add_array_item_object();
        schema_labels(w, "weight", &["min", "avg", "max", "sum", "count"]);
        w.object_close();
    } else {
        w.add_array_item_object();
        w.member_add_string("name", "row_type");
        w.member_add_string("type", "integer");
        w.member_add_array(Some(b"value"));
        for name in ["dimension", "instance", "context", "node"] {
            w.add_array_item_string(name);
        }
        w.array_close();
        w.object_close();
        schema_item(w, "ni", "integer", Some("nodes"));
        schema_item(w, "ci", "integer", Some("contexts"));
        schema_item(w, "ii", "integer", Some("instances"));
        schema_item(w, "di", "integer", Some("dimensions"));
        schema_item(w, "weight", "number", None);
    }
    schema_timeframe(w, "timeframe");
    if baseline {
        schema_timeframe(w, "baseline timeframe");
    }
    w.array_close();
    w.object_close();
}

/// `struct aggregated_weight`: the weights of some results, with their points of the two windows merged.
#[derive(Debug, Clone, Copy)]
struct Aggregated {
    min: f64,
    max: f64,
    sum: f64,
    count: usize,
    highlighted: StoragePoint,
    baseline: StoragePoint,
}

impl Aggregated {
    /// `AGGREGATED_WEIGHT_EMPTY`.
    const EMPTY: Aggregated = Aggregated {
        min: f64::NAN,
        max: f64::NAN,
        sum: f64::NAN,
        count: 0,
        highlighted: StoragePoint::UNSET,
        baseline: StoragePoint::UNSET,
    };

    /// One result alone: a dimension's row, or the first of a group.
    fn of(t: &Registered) -> Self {
        Aggregated {
            min: t.value,
            max: t.value,
            sum: t.value,
            count: 1,
            highlighted: t.highlighted,
            baseline: t.baseline,
        }
    }

    /// `merge_into_aw()`: the first result is taken as it is (its baseline point only when the method has a
    /// baseline), the others are merged in.
    fn merge(&mut self, t: &Registered, baseline: bool) {
        if self.count == 0 {
            (self.min, self.max, self.sum, self.count) = (t.value, t.value, t.value, 1);
            self.highlighted = t.highlighted;
            if baseline {
                self.baseline = t.baseline;
            }
            return;
        }
        self.count += 1;
        self.sum += t.value;
        if t.value < self.min {
            self.min = t.value;
        }
        if t.value > self.max {
            self.max = t.value;
        }
        self.highlighted.merge_to(&t.highlighted);
        if baseline {
            self.baseline.merge_to(&t.baseline);
        }
    }

    fn average(&self) -> f64 {
        if self.count != 0 { self.sum / self.count as f64 } else { 0.0 }
    }

    /// `weights_group_score()`: what a limit ranks the groups by.
    fn score(&self, aggregation: Aggregation) -> f64 {
        match aggregation {
            Aggregation::Min => self.min,
            Aggregation::Max | Aggregation::Extremes => self.max,
            Aggregation::Sum | Aggregation::Percentage => self.sum,
            Aggregation::Average => self.average(),
        }
    }
}

fn point(w: &mut JsonWriter, p: &StoragePoint) {
    w.add_array_item_array();
    w.add_array_item_double(p.min);
    w.add_array_item_double(if p.count != 0 { p.sum / f64::from(p.count) } else { 0.0 });
    w.add_array_item_double(p.max);
    w.add_array_item_double(p.sum);
    w.add_array_item_uint64(u64::from(p.count));
    w.add_array_item_uint64(u64::from(p.anomaly_count));
    w.array_close();
}

/// The tail of `storage_point_to_json()`: the highlighted window's point and, for a method with a baseline,
/// the baseline's.
fn points(w: &mut JsonWriter, aw: &Aggregated, baseline: bool) {
    point(w, &aw.highlighted);
    if baseline {
        point(w, &aw.baseline);
    }
}

/// `WEIGHTS_POINT_TYPE` of the plain format's rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    Dimension = 0,
    Instance = 1,
    Context = 2,
    Node = 3,
}

/// The dictionary indexes a row carries; -1 where C has none yet.
#[derive(Debug, Clone, Copy)]
struct At {
    ni: i64,
    ci: i64,
    ii: i64,
    di: i64,
}

/// `storage_point_to_json()` for a row: its type, the indexes of what it is about (null below its level), its
/// weight and its points.
fn row(w: &mut JsonWriter, kind: Row, at: At, aw: &Aggregated, baseline: bool) {
    let levels = match kind {
        Row::Dimension => 3,
        Row::Instance => 2,
        Row::Context => 1,
        Row::Node => 0,
    };
    w.add_array_item_array();
    w.add_array_item_uint64(kind as u64);
    w.add_array_item_int64(at.ni);
    for (level, index) in [at.ci, at.ii, at.di].into_iter().enumerate() {
        if level < levels {
            w.add_array_item_int64(index);
        } else {
            w.add_array_item_null();
        }
    }
    w.add_array_item_double(aw.average());
    points(w, aw, baseline);
    w.array_close();
}

/// A dictionary of the plain format, or the groups of the grouped one: C's dictionary that keeps the value a key
/// was first given, read in the order the keys came. An entry's index is its place.
struct Unique<T> {
    index: HashMap<Vec<u8>, usize>,
    entries: Vec<Entry<T>>,
}

struct Entry<T> {
    value: T,
    /// Printed in the dictionaries.
    exposed: bool,
}

impl<T> Unique<T> {
    fn new() -> Self {
        Unique { index: HashMap::new(), entries: Vec::new() }
    }

    /// The index of `key`'s entry, made by `make` when the key is new, and whether it is.
    fn add(&mut self, key: Vec<u8>, make: impl FnOnce() -> T) -> (usize, bool) {
        let next = self.entries.len();
        let i = *self.index.entry(key).or_insert(next);
        if i == next {
            self.entries.push(Entry { value: make(), exposed: false });
        }
        (i, i == next)
    }

    fn expose(&mut self, i: usize) {
        self.entries[i].exposed = true;
    }

    fn exposed(&self) -> impl Iterator<Item = (usize, &T)> {
        self.entries.iter().enumerate().filter(|(_, e)| e.exposed).map(|(i, e)| (i, &e.value))
    }
}

/// `dict_unique_id_name_add()`'s key: `id:name`, in a buffer that holds 1023 bytes of it.
fn id_name_key(id: &str, name: &str) -> Vec<u8> {
    let mut key = [id.as_bytes(), b":", name.as_bytes()].concat();
    key.truncate(1023);
    key
}

struct Node {
    host: Arc<Host>,
    /// The time its results' queries took, together.
    duration_us: u64,
}

struct Named {
    id: String,
    /// A context's units; an instance's or a dimension's name.
    other: String,
}

fn named_to_json(w: &mut JsonWriter, array: &[u8], index_key: &str, entries: &Unique<Named>) {
    w.member_add_array(Some(array));
    for (i, named) in entries.exposed() {
        w.add_array_item_object();
        w.member_add_string("id", &named.id);
        if named.id != named.other {
            w.member_add_string("nm", &named.other);
        }
        w.member_add_int64(index_key, i as i64);
        w.object_close();
    }
    w.array_close();
}

/// `registered_results_to_json_multinode_no_group_by()`. Returns the body and how many dimensions it holds.
/// - `result`: a row per result, and at each change of instance, of context and of host (by identity, in the
///   order the results were registered) the rollup of what ended: every result of it counts, printed or not.
/// - With a limit only the selected results are printed, and the rollups of an instance, a context or a node
///   only when it holds one; the index of what is not shown is -1 and no row prints it.
/// - `dictionaries`: what the rows index, in first-seen order. Contexts are keyed by id, instances and
///   dimensions by id and name, so the same chart on two hosts is one entry; nodes by machine GUID, and every
///   node with a result is listed, selected or not, with the time its queries took.
/// - The node entries have the short keys: C reads its thread's key table here without setting it.
pub fn plain(finished: &Finished, by: &Answering) -> (Vec<u8>, usize) {
    let mut w = begin(finished, by, false);
    let baseline = has_baseline(finished.request.method);
    let limit = finished.request.cardinality_limit != 0;

    let mut nodes: Unique<Node> = Unique::new();
    let (mut contexts, mut instances, mut dimensions) = (Unique::new(), Unique::new(), Unique::new());
    let (mut node_aw, mut context_aw, mut instance_aw) = (Aggregated::EMPTY, Aggregated::EMPTY, Aggregated::EMPTY);
    let (mut last_host, mut last_context, mut last_instance) = (None, None, None);
    let (mut node, mut context, mut instance) = (0, None, None);
    let mut at = At { ni: -1, ci: -1, ii: -1, di: -1 };
    let mut node_selected = false;
    let mut printed = 0;

    w.member_add_array(Some(b"result"));
    for t in &finished.results {
        if last_instance.is_some_and(|last| !Arc::ptr_eq(last, &t.instance)) {
            if let Some(i) = instance {
                row(&mut w, Row::Instance, at, &instance_aw, baseline);
                instances.expose(i);
            }
            last_instance = None;
            instance_aw = Aggregated::EMPTY;
        }
        if last_context.is_some_and(|last| !Arc::ptr_eq(last, &t.context)) {
            if let Some(i) = context {
                row(&mut w, Row::Context, at, &context_aw, baseline);
                contexts.expose(i);
            }
            last_context = None;
            context_aw = Aggregated::EMPTY;
        }
        if last_host.is_some_and(|last| !Arc::ptr_eq(last, &t.host)) {
            if node_selected {
                row(&mut w, Row::Node, at, &node_aw, baseline);
            }
            last_host = None;
            node_aw = Aggregated::EMPTY;
        }

        if last_host.is_none() {
            last_host = Some(&t.host);
            let key = t.host.machine_guid().as_bytes().to_vec();
            (node, _) = nodes.add(key, || Node { host: Arc::clone(&t.host), duration_us: 0 });
            at.ni = node as i64;
            node_selected = !limit || t.node_selected;
            nodes.expose(node);
        }
        if last_context.is_none() {
            last_context = Some(&t.context);
            context = (!limit || t.context_selected).then(|| {
                let id = t.context.id();
                contexts.add(id.as_bytes().to_vec(), || Named { id: id.to_owned(), other: t.context.state().units }).0
            });
            at.ci = context.map_or(-1, |i| i as i64);
        }
        if last_instance.is_none() {
            last_instance = Some(&t.instance);
            instance = (!limit || t.instance_selected).then(|| {
                let (id, name) = (t.instance.id(), t.instance.state().name);
                instances.add(id_name_key(id, &name), || Named { id: id.to_owned(), other: name }).0
            });
            at.ii = instance.map_or(-1, |i| i as i64);
        }

        if !limit || t.selected {
            let (id, name) = (t.metric.id(), t.metric.state().name);
            let (i, _) = dimensions.add(id_name_key(id, &name), || Named { id: id.to_owned(), other: name });
            at.di = i as i64;
            row(&mut w, Row::Dimension, at, &Aggregated::of(t), baseline);
            if let Some(i) = context {
                contexts.expose(i);
            }
            if let Some(i) = instance {
                instances.expose(i);
            }
            dimensions.expose(i);
            printed += 1;
        }

        instance_aw.merge(t, baseline);
        context_aw.merge(t, baseline);
        node_aw.merge(t, baseline);
        let entry = &mut nodes.entries[node].value;
        entry.duration_us = entry.duration_us.wrapping_add(t.duration_us);
    }
    if let Some(i) = instance {
        row(&mut w, Row::Instance, at, &instance_aw, baseline);
        instances.expose(i);
    }
    if let Some(i) = context {
        row(&mut w, Row::Context, at, &context_aw, baseline);
        contexts.expose(i);
    }
    if node_selected {
        row(&mut w, Row::Node, at, &node_aw, baseline);
    }
    w.array_close();

    w.member_add_object("dictionaries");
    w.member_add_array(Some(b"nodes"));
    for (i, entry) in nodes.exposed() {
        w.add_array_item_object();
        node_add_v2(&mut w, Keys::with_long(false), &entry.host, i, entry.duration_us, true);
        w.object_close();
    }
    w.array_close();
    w.member_add_array(Some(b"contexts"));
    for (i, named) in contexts.exposed() {
        w.add_array_item_object();
        w.member_add_string("id", &named.id);
        w.member_add_string("units", &named.other);
        w.member_add_int64("ci", i as i64);
        w.object_close();
    }
    w.array_close();
    named_to_json(&mut w, b"instances", "ii", &instances);
    named_to_json(&mut w, b"dimensions", "di", &dimensions);
    w.object_close();

    (end(w, finished, by, "dimensions", finished.results.len(), printed), printed)
}

/// The id a group's key names a node by: its node id, or its host id (the machine GUID) when it has none.
fn node_uuid(host: &Host) -> Vec<u8> {
    let node_id = host.node_id();
    let id = if node_id == [0; 16] {
        uuid_parse_flexi(host.machine_guid().as_bytes()).unwrap_or_default()
    } else {
        node_id
    };
    let (text, len) = uuid_lower_text(&id, false);
    text[..len].to_vec()
}

/// A result's group: its key, and the name shown for it. Each part the request groups by adds to both, after a
/// comma when the key holds something: the dimension's name; the instance's id (in the name, its name), with
/// `@` and the node unless the request also groups by node; the node's id (in the name, the host's name as the
/// walk took it); the context's id; the context's units.
fn group_key(t: &Registered, bits: u32) -> (Vec<u8>, Vec<u8>) {
    let (mut key, mut name): (Vec<u8>, Vec<u8>) = (Vec::new(), Vec::new());
    let mut part = |to_key: &[u8], to_name: &[u8]| {
        if !key.is_empty() {
            key.push(b',');
            name.push(b',');
        }
        key.extend_from_slice(to_key);
        name.extend_from_slice(to_name);
    };
    if bits & group_by::DIMENSION != 0 {
        let dimension = t.metric.state().name;
        part(dimension.as_bytes(), dimension.as_bytes());
    }
    if bits & group_by::INSTANCE != 0 {
        let mut instance_key = t.instance.id().as_bytes().to_vec();
        let mut instance_name = t.instance.state().name.into_bytes();
        if bits & group_by::NODE == 0 {
            instance_key.push(b'@');
            instance_key.extend_from_slice(&node_uuid(&t.host));
            instance_name.push(b'@');
            instance_name.extend_from_slice(t.hostname.as_bytes());
        }
        part(&instance_key, &instance_name);
    }
    if bits & group_by::NODE != 0 {
        part(&node_uuid(&t.host), t.hostname.as_bytes());
    }
    if bits & group_by::CONTEXT != 0 {
        part(t.context.id().as_bytes(), t.context.id().as_bytes());
    }
    if bits & group_by::UNITS != 0 {
        let units = t.context.state().units;
        part(units.as_bytes(), units.as_bytes());
    }
    (key, name)
}

struct Group {
    id: Vec<u8>,
    name: Vec<u8>,
    aw: Aggregated,
    selected: bool,
}

/// `weights_group_compare()`: by the aggregation's score, the smaller first when the weights were spread
/// (`normalized`: the strongest is 0), the larger first otherwise; then by id.
fn group_compare(a: &Group, b: &Group, aggregation: Aggregation, normalized: bool) -> Ordering {
    let (av, bv) = (a.aw.score(aggregation), b.aw.score(aggregation));
    if av != bv {
        return if (av < bv) == normalized { Ordering::Less } else { Ordering::Greater };
    }
    a.id.cmp(&b.id)
}

/// `registered_results_to_json_multinode_group_by()`. Returns the body and how many results it grouped.
/// - `result`: a group per key in first-seen order, with its id, its name when it is another text, and `v`: the
///   minimum, average, maximum, sum and count of its weights, then its points merged.
/// - A limit below the number of groups keeps the best of them by the request's aggregation; the others are
///   left out, the counts still say how many there were.
/// - C gives its dictionary the key as it is: results that group under an empty key (by units alone, in a
///   context without units) end it at a null pointer. They are a group here.
pub fn grouped(finished: &Finished, by: &Answering) -> (Vec<u8>, usize) {
    let mut w = begin(finished, by, true);
    let req = &finished.request;
    let baseline = has_baseline(req.method);

    let mut groups: Unique<Group> = Unique::new();
    for t in &finished.results {
        let (key, name) = group_key(t, req.group_by.group_by);
        let new = || Group { id: key.clone(), name, aw: Aggregated::of(t), selected: false };
        let (i, is_new) = groups.add(key.clone(), new);
        if !is_new {
            groups.entries[i].value.aw.merge(t, baseline);
        }
    }

    let total = groups.entries.len();
    let limit = usize::try_from(req.cardinality_limit).unwrap_or(usize::MAX);
    let limited = limit != 0 && limit < total;
    if limited {
        let normalized = req.options & options::RETURN_RAW == 0 && req.method != Method::Value;
        let aggregation = req.group_by.aggregation;
        let mut order: Vec<usize> = (0..total).collect();
        let group = |i: usize| &groups.entries[i].value;
        order.sort_by(|&a, &b| group_compare(group(a), group(b), aggregation, normalized));
        for &i in order.iter().take(limit) {
            groups.entries[i].value.selected = true;
        }
    }

    let mut returned = 0;
    w.member_add_array(Some(b"result"));
    for group in groups.entries.iter().map(|e| &e.value) {
        if limited && !group.selected {
            continue;
        }
        w.add_array_item_object();
        w.member_add_string("id", &group.id);
        if group.id != group.name {
            w.member_add_string("nm", &group.name);
        }
        w.member_add_array(Some(b"v"));
        w.add_array_item_array();
        w.add_array_item_double(group.aw.min);
        w.add_array_item_double(group.aw.average());
        w.add_array_item_double(group.aw.max);
        w.add_array_item_double(group.aw.sum);
        w.add_array_item_uint64(group.aw.count as u64);
        w.array_close();
        points(&mut w, &group.aw, baseline);
        w.array_close();
        w.object_close();
        returned += 1;
    }
    w.array_close();

    (end(w, finished, by, "groups", total, returned), finished.results.len())
}

/// The multinode answer of a finished request: grouped when the request groups by something the weights group
/// by, else every result with its rollups.
pub fn multinode(finished: &Finished, by: &Answering) -> (Vec<u8>, usize) {
    if finished.request.group_by.group_by == group_by::NONE { plain(finished, by) } else { grouped(finished, by) }
}

#[cfg(test)]
mod tests {
    use super::super::methods::Stats;
    use super::super::parse::{Format, parse};
    use super::super::results::{Found, Of, register, select};
    use super::super::timeout_ms;
    use super::*;
    use crate::testing::{weights_host_as, weights_named_host};

    const ONE: &str = "11111111-1111-1111-1111-111111111111";
    const TWO: &str = "22222222-2222-2222-2222-222222222222";
    const NAMED: &str = "33333333-3333-3333-3333-333333333333";

    fn sp(min: f64, max: f64, sum: f64, count: u32, anomaly_count: u32) -> StoragePoint {
        StoragePoint { min, max, sum, count, anomaly_count, ..StoragePoint::default() }
    }

    /// The fixture's hosts: `one` and `two` with chart `t.w` of `ctx.w` (units `u`), and `named` with chart `t.n`
    /// named `t.named` and its dimension `d` named `dee`, in `ctx.n` (units `things`).
    fn hosts() -> [Arc<Host>; 3] {
        [weights_host_as(ONE, "one"), weights_host_as(TWO, "two"), weights_named_host(NAMED, "named")]
    }

    /// A finished version-2 request of `method` with a result for each of `found`: (host, metric id, what the
    /// method found), the hosts being [`hosts`].
    fn finished(method: Method, query: &str, found: &[(usize, &str, Found)]) -> Finished {
        let hosts = hosts();
        let (mut results, mut stats): (Vec<Registered>, Stats) = (Vec::new(), Stats::default());
        for &(host, dimension, found) in found {
            let h = &hosts[host];
            let contexts = h.contexts();
            let rc = contexts.get("ctx.w").or_else(|| contexts.get("ctx.n")).expect("the fixture's context");
            let ri = rc.instances().into_iter().next().expect("its instance");
            let rm = ri.metric(dimension).expect("the metric");
            let hostname = h.hostname();
            let of = Of { host: h, hostname: &hostname, context: &rc, instance: &ri, metric: &rm };
            register(&mut results, &mut stats, true, &of, found);
        }
        let mut request = parse(query.as_bytes(), 2, method, Format::Multinode, 1).expect("a request");
        // what the engine leaves: the windows, the timeout, the group-by it knows, and no option but the query's
        (request.after, request.before, request.baseline_after, request.baseline_before) = (1000, 1060, 760, 1000);
        request.timeout_ms = timeout_ms(request.timeout_ms);
        request.options &= options::MINIFY | options::RFC3339 | options::SELECTED_TIER | options::RETURN_RAW;
        let normalized = request.options & options::RETURN_RAW == 0 && method != Method::Value;
        if request.cardinality_limit != 0 && request.group_by.group_by == group_by::NONE {
            select(&mut results, usize::try_from(request.cardinality_limit).unwrap(), normalized);
        }
        stats = Stats { db_points: 484, result_points: 4, db_queries: 4, ..Stats::default() };
        stats.db_points_per_tier[0] = 484;
        Finished {
            request,
            shifts: 2,
            results,
            stats,
            examined: 10,
            received: Instant::now(),
            duration_us: 2500,
            versions: Versions { contexts_hard_hash: 3, ..Versions::default() },
        }
    }

    fn value(value: f64) -> Found {
        Found { value, flags: 0, highlighted: None, baseline: None, duration_us: 0 }
    }

    /// The body of `write`, as the agent `agent` of GUID `guid-agent` whose host index is at version 7.
    fn body(write: fn(&Finished, &Answering) -> (Vec<u8>, usize), finished: &Finished) -> (String, usize) {
        let nodes_hard_hash = || 7;
        let (machine_guid, hostname) = ("guid-agent", "agent");
        let agent = Agent { machine_guid, node_id: [0; 16], hostname, nodes_hard_hash: &nodes_hard_hash };
        let (body, count) = write(finished, &Answering { agent, storage_tiers: 1 });
        (String::from_utf8(body).unwrap(), count)
    }

    /// The text between `from` and `to`, both included; the first of each.
    fn between<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
        let start = text.find(from).unwrap_or_else(|| panic!("no {from} in {text}"));
        let end = text[start..].find(to).unwrap_or_else(|| panic!("no {to} after {from} in {text}"));
        &text[start..start + end + to.len()]
    }

    const VALUES: [(usize, &str, f64); 3] = [(0, "a", 0.5), (0, "b", 0.25), (1, "a", 0.75)];

    fn values(values: &[(usize, &'static str, f64)]) -> Vec<(usize, &'static str, Found)> {
        values.iter().map(|&(host, dimension, v)| (host, dimension, value(v))).collect()
    }

    const DB: &str = concat!(
        r#""db":{"db_queries":4,"query_result_points":4,"binary_searches":0,"db_points_read":484,"#,
        r#""db_points_per_tier":[484]},"#
    );
    const VERSIONS: &str = concat!(
        r#""versions":{"routing_hard_hash":1,"nodes_hard_hash":7,"contexts_hard_hash":3,"contexts_soft_hash":0,"#,
        r#""alerts_hard_hash":0,"alerts_soft_hash":0},"#
    );
    const TIMEFRAME: &str = concat!(
        r#"{"name":"timeframe","type":"array","labels":["min","avg","max","sum","count","anomaly_count"],"#,
        r#""calculations":{"anomaly rate":"anomaly_count * 100 / count"}}"#
    );
    const BASELINE_TIMEFRAME: &str = concat!(
        r#"{"name":"baseline timeframe","type":"array","labels":["min","avg","max","sum","count","anomaly_count"],"#,
        r#""calculations":{"anomaly rate":"anomaly_count * 100 / count"}}"#
    );

    /// What comes before the results of a request without a baseline: the request with `*` for every text not
    /// given, a null tier and null grouping options, the timeout the engine settled; the view; the counters; the
    /// versions with the host index's as it is at print; the schema of a row.
    #[test]
    fn the_answer_starts_with_the_request_the_view_the_counters_the_versions_and_the_schema() {
        let (text, _) = body(plain, &finished(Method::Value, "options=minify&points=7", &values(&VALUES)));
        let head = [
            r#"{"api":2,"request":{"method":"value","options":["minify"],"#,
            r#""scope":{"scope_nodes":"*","scope_contexts":"*","scope_instances":"*","scope_labels":"*"},"#,
            r#""selectors":{"nodes":"*","contexts":"*","instances":"*","dimensions":"*","labels":"*","alerts":"*"},"#,
            r#""window":{"after":1000,"before":1060,"points":7,"tier":null},"#,
            r#""aggregations":{"time":{"time_group":"average","time_group_options":null},"#,
            r#""metrics":[{"group_by":["none"],"aggregation":"average"}]},"timeout":300000},"#,
            r#""view":{"format":"full","time_group":"average","#,
            r#""window":{"after":1000,"before":1060,"duration":60,"points":7}},"#,
            DB,
            VERSIONS,
            r#""schema":{"type":"array","items":[{"name":"row_type","type":"integer","#,
            r#""value":["dimension","instance","context","node"]},"#,
            r#"{"name":"ni","type":"integer","dictionary":"nodes"},"#,
            r#"{"name":"ci","type":"integer","dictionary":"contexts"},"#,
            r#"{"name":"ii","type":"integer","dictionary":"instances"},"#,
            r#"{"name":"di","type":"integer","dictionary":"dimensions"},"#,
            r#"{"name":"weight","type":"number"},"#,
            TIMEFRAME,
            r#"]},"result":["#,
        ]
        .concat();
        assert!(text.starts_with(&head), "{text}");
    }

    /// The texts as given, the tier when one was selected, the grouping and its options, a timeout below a
    /// second raised to one; for a method with a baseline its window in the request (two members of their own
    /// names) and in the view (with its duration and the points shifted), and a second timeframe in the schema.
    /// A grouped answer says so and calls its schema `v_schema`, of a group's values.
    #[test]
    fn the_request_echoes_what_was_given_and_a_baseline_adds_its_members() {
        let query = concat!(
            "options=minify&points=20&method=ks2&scope_nodes=sn&scope_contexts=sc&scope_instances=si",
            "&scope_labels=sl&scope_dimensions=sd&nodes=n&contexts=c&instances=i&dimensions=d&labels=l&alerts=al",
            "&tier=0&time_group=max&time_group_options=o&timeout=5&group_by=dimension,node&aggregation=sum"
        );
        let (text, _) = body(grouped, &finished(Method::Value, query, &values(&VALUES)));
        let request = [
            r#""request":{"method":"ks2","options":["selected-tier","minify"],"#,
            r#""scope":{"scope_nodes":"sn","scope_contexts":"sc","scope_instances":"si","scope_labels":"sl"},"#,
            r#""selectors":{"nodes":"n","contexts":"c","instances":"i","dimensions":"d","labels":"l","alerts":"al"},"#,
            r#""window":{"after":1000,"before":1060,"points":20,"tier":0},"#,
            r#""baseline":{"baseline_after":760,"baseline_before":1000},"#,
            r#""aggregations":{"time":{"time_group":"max","time_group_options":"o"},"#,
            r#""metrics":[{"group_by":["dimension","node"],"aggregation":"sum"}]},"timeout":1000},"#,
            r#""view":{"format":"grouped","time_group":"max","#,
            r#""window":{"after":1000,"before":1060,"duration":60,"points":20},"#,
            r#""baseline":{"after":760,"before":1000,"duration":240,"points":80}},"#,
        ]
        .concat();
        assert_eq!(between(&text, r#""request":"#, r#""points":80}},"#), request, "{text}");
        let schema = [
            r#""v_schema":{"type":"array","items":["#,
            r#"{"name":"weight","type":"array","labels":["min","avg","max","sum","count"]},"#,
            TIMEFRAME,
            ",",
            BASELINE_TIMEFRAME,
            r#"]},"result":["#,
        ]
        .concat();
        assert!(text.contains(&schema), "{text}");
        assert!(!text.contains(r#""schema""#), "{text}");
        // not minified without the option
        assert!(body(plain, &finished(Method::Value, "", &values(&VALUES))).0.contains('\n'));
    }

    const ZEROS: &str = "[0,0,0,0,0,0]";

    /// Every result is a row, and the rollups come at each change of instance, context and host: here all three
    /// at once, the second host's chart being another object of the same ids. The same context, chart and
    /// dimension on two hosts are one dictionary entry each; the nodes are two. A name equal to its id is not
    /// printed. The agent's block follows with the timings (no preparation, the engine's time as the query's),
    /// then the two counts; without a limit no summary of one.
    #[test]
    fn the_plain_format_has_a_row_per_result_and_rollups_at_each_boundary() {
        let (text, printed) = body(plain, &finished(Method::Value, "options=minify", &values(&VALUES)));
        assert_eq!(printed, 3);
        let result = [
            r#""result":["#,
            &format!("[0,0,0,0,0,0.5,{ZEROS}],[0,0,0,0,1,0.25,{ZEROS}],"),
            &format!("[1,0,0,0,null,0.375,{ZEROS}],[2,0,0,null,null,0.375,{ZEROS}],"),
            &format!("[3,0,null,null,null,0.375,{ZEROS}],"),
            &format!("[0,1,0,0,0,0.75,{ZEROS}],"),
            &format!("[1,1,0,0,null,0.75,{ZEROS}],[2,1,0,null,null,0.75,{ZEROS}],[3,1,null,null,null,0.75,{ZEROS}]"),
            r#"],"dictionaries":{"nodes":["#,
            &format!(r#"{{"mg":"{ONE}","nm":"one","ni":0,"st":{{"ai":0,"code":200,"msg":""}}}},"#),
            &format!(r#"{{"mg":"{TWO}","nm":"two","ni":1,"st":{{"ai":0,"code":200,"msg":""}}}}],"#),
            r#""contexts":[{"id":"ctx.w","units":"u","ci":0}],"#,
            r#""instances":[{"id":"t.w","ii":0}],"#,
            r#""dimensions":[{"id":"a","di":0},{"id":"b","di":1}]},"#,
            r#""agents":[{"mg":"guid-agent","#,
        ]
        .concat();
        assert_eq!(between(&text, r#""result":"#, r#""agents":[{"mg":"guid-agent","#), result, "{text}");
        let agents = between(&text, r#""agents":["#, "}}],");
        assert!(agents.contains(r#""nm":"agent","now":"#), "{text}");
        assert!(agents.contains(r#","ai":0,"timings":{"prep_ms":0,"query_ms":2.5,"output_ms":"#), "{text}");
        assert!(text.ends_with(r#"}}],"correlated_dimensions":3,"total_dimensions_count":10}"#), "{text}");
    }

    /// A chart and a dimension whose names are not their ids print them as `nm`, and are keyed by both; a
    /// context's entry has its units. A node's entry has the time its results' queries took, when there is any.
    #[test]
    fn the_dictionaries_have_the_names_the_units_and_the_time_of_a_node() {
        let slow = |v: f64, duration_us: u64| Found { duration_us, ..value(v) };
        let found = [(0, "a", slow(0.5, 1500)), (0, "b", slow(0.25, 500)), (2, "d", value(0.125))];
        let (text, printed) = body(plain, &finished(Method::Value, "options=minify", &found));
        assert_eq!(printed, 3);
        let dictionaries = [
            r#""dictionaries":{"nodes":["#,
            &format!(r#"{{"mg":"{ONE}","nm":"one","ni":0,"st":{{"ai":0,"code":200,"msg":"","ms":2}}}},"#),
            &format!(r#"{{"mg":"{NAMED}","nm":"named","ni":1,"st":{{"ai":0,"code":200,"msg":""}}}}],"#),
            r#""contexts":[{"id":"ctx.w","units":"u","ci":0},{"id":"ctx.n","units":"things","ci":1}],"#,
            r#""instances":[{"id":"t.w","ii":0},{"id":"t.n","nm":"t.named","ii":1}],"#,
            r#""dimensions":[{"id":"a","di":0},{"id":"b","di":1},{"id":"d","nm":"dee","di":2}]},"#,
        ]
        .concat();
        assert_eq!(between(&text, r#""dictionaries":"#, r#""di":2}]},"#), dictionaries, "{text}");
        assert!(text.contains(&format!("[0,1,1,1,2,0.125,{ZEROS}],[1,1,1,1,null,0.125,{ZEROS}]")), "{text}");
    }

    /// With a limit only the selected result is a row, and only what holds it has a rollup and a dictionary
    /// entry: the other host's instance, context and node print no row. Both nodes are listed all the same. The
    /// summary of the limit counts dimensions.
    #[test]
    fn a_limit_prints_the_selected_results_and_lists_every_node() {
        let (text, printed) = body(plain, &finished(Method::Value, "options=minify&limit=1", &values(&VALUES)));
        assert_eq!(printed, 1);
        let result = [
            r#""result":["#,
            &format!("[0,1,0,0,0,0.75,{ZEROS}],"),
            &format!("[1,1,0,0,null,0.75,{ZEROS}],[2,1,0,null,null,0.75,{ZEROS}],[3,1,null,null,null,0.75,{ZEROS}]"),
            r#"],"dictionaries":{"nodes":["#,
            &format!(r#"{{"mg":"{ONE}","nm":"one","ni":0,"st":{{"ai":0,"code":200,"msg":""}}}},"#),
            &format!(r#"{{"mg":"{TWO}","nm":"two","ni":1,"st":{{"ai":0,"code":200,"msg":""}}}}],"#),
            r#""contexts":[{"id":"ctx.w","units":"u","ci":0}],"#,
            r#""instances":[{"id":"t.w","ii":0}],"#,
            r#""dimensions":[{"id":"a","di":0}]},"#,
        ]
        .concat();
        assert_eq!(between(&text, r#""result":"#, r#""di":0}]},"#), result, "{text}");
        let tail = concat!(
            r#"}}],"correlated_dimensions":3,"total_dimensions_count":10,"result_limit":{"limit":1,"total":3,"#,
            r#""returned":1,"unit":"dimensions","truncated":true,"summary_scope":"all"}}"#
        );
        assert!(text.ends_with(tail), "{text}");
        // the rollups of what is shown still count every result of it, selected or not
        let both = [(0, "a", value(0.5)), (0, "b", value(0.25))];
        let (text, printed) = body(plain, &finished(Method::Value, "options=minify&limit=1", &both));
        assert_eq!(printed, 1);
        let rows = [
            format!("[0,0,0,0,0,0.5,{ZEROS}],[1,0,0,0,null,0.375,{ZEROS}],"),
            format!("[2,0,0,null,null,0.375,{ZEROS}],[3,0,null,null,null,0.375,{ZEROS}]]"),
        ]
        .concat();
        assert!(text.contains(&rows), "{text}");
    }

    /// A method with a baseline: each row has the highlighted window's point and the baseline's; a rollup has
    /// them merged (the smallest minimum, the largest maximum, the sums and counts added), the average being
    /// the sum over the count.
    #[test]
    fn rows_carry_the_points_of_both_windows_and_rollups_merge_them() {
        let with = |v: f64, highlighted: StoragePoint, baseline: StoragePoint| Found {
            highlighted: Some(highlighted),
            baseline: Some(baseline),
            ..value(v)
        };
        let found = [
            (0, "a", with(0.5, sp(1.0, 3.0, 4.0, 2, 1), sp(2.0, 5.0, 7.0, 2, 0))),
            (0, "b", with(0.25, sp(0.5, 2.0, 5.0, 4, 2), sp(1.0, 8.0, 9.0, 2, 1))),
        ];
        let (text, _) = body(plain, &finished(Method::Ks2, "options=minify|raw", &found));
        let rows = concat!(
            r#""result":[[0,0,0,0,0,0.5,[1,2,3,4,2,1],[2,3.5,5,7,2,0]],"#,
            "[0,0,0,0,1,0.25,[0.5,1.25,2,5,4,2],[1,4.5,8,9,2,1]],",
            "[1,0,0,0,null,0.375,[0.5,1.5,3,9,6,3],[1,4,8,16,4,1]],",
            "[2,0,0,null,null,0.375,[0.5,1.5,3,9,6,3],[1,4,8,16,4,1]],",
            "[3,0,null,null,null,0.375,[0.5,1.5,3,9,6,3],[1,4,8,16,4,1]]],"
        );
        assert!(text.contains(rows), "{text}");
        // a method without a baseline prints one point, whatever the results hold
        let (text, _) = body(plain, &finished(Method::Value, "options=minify", &found));
        assert!(text.contains(r#""result":[[0,0,0,0,0,0.5,[1,2,3,4,2,1]],"#), "{text}");
    }

    /// A group's key and name, part by part: the dimension's name; the instance's id, with the node unless the
    /// request groups by node too (in the name: the instance's name and the host's); the node's id, which is
    /// its host id while it has no node id; the context; the units. Parts are joined by commas.
    #[test]
    fn a_group_is_keyed_by_what_the_request_names() {
        use group_by::{CONTEXT, DIMENSION, INSTANCE, NODE, UNITS};
        let f = finished(Method::Value, "", &[(0, "a", value(0.5)), (2, "d", value(0.25))]);
        let key = |i: usize, bits: u32| {
            let (key, name) = group_key(&f.results[i], bits);
            (String::from_utf8(key).unwrap(), String::from_utf8(name).unwrap())
        };
        let pair = |key: &str, name: &str| (key.to_owned(), name.to_owned());
        assert_eq!(key(0, DIMENSION), pair("a", "a"));
        assert_eq!(key(1, DIMENSION), pair("dee", "dee"));
        assert_eq!(key(0, INSTANCE), pair(&format!("t.w@{ONE}"), "t.w@one"));
        assert_eq!(key(0, INSTANCE | NODE), pair(&format!("t.w,{ONE}"), "t.w,one"));
        assert_eq!(key(0, NODE), pair(ONE, "one"));
        assert_eq!(key(0, CONTEXT), pair("ctx.w", "ctx.w"));
        assert_eq!(key(1, UNITS), pair("things", "things"));
        assert_eq!(key(1, DIMENSION | CONTEXT | UNITS), pair("dee,ctx.n,things", "dee,ctx.n,things"));
        assert_eq!(key(1, INSTANCE), pair(&format!("t.n@{NAMED}"), "t.named@named"));
        // a node id, once the host has one, names the node
        let node_id = [0xab; 16];
        f.results[0].host.set_node_id(node_id);
        assert_eq!(key(0, NODE), pair("abababab-abab-abab-abab-abababababab", "one"));
    }

    /// The grouped format: the groups in first-seen order, each with the minimum, average, maximum, sum and
    /// count of its weights and its points; `nm` only for a name that is not the id. Every result counts in
    /// `correlated_dimensions`.
    #[test]
    fn the_grouped_format_aggregates_the_weights_of_each_group() {
        let query = "options=minify&group_by=dimension";
        let (text, grouped_results) = body(multinode, &finished(Method::Value, query, &values(&VALUES)));
        assert_eq!(grouped_results, 3);
        let result = [
            format!(r#""result":[{{"id":"a","v":[[0.5,0.625,0.75,1.25,2],{ZEROS}]}},"#),
            format!(r#"{{"id":"b","v":[[0.25,0.25,0.25,0.25,1],{ZEROS}]}}],"agents":["#),
        ]
        .concat();
        assert!(text.contains(&result), "{text}");
        assert!(text.ends_with(r#"}}],"correlated_dimensions":3,"total_dimensions_count":10}"#), "{text}");
        assert!(!text.contains("dictionaries"), "{text}");
        let (text, _) = body(multinode, &finished(Method::Value, "options=minify&group_by=node", &values(&VALUES)));
        let result = [
            format!(r#""result":[{{"id":"{ONE}","nm":"one","v":[[0.25,0.375,0.5,0.75,2],{ZEROS}]}},"#),
            format!(r#"{{"id":"{TWO}","nm":"two","v":[[0.75,0.75,0.75,0.75,1],{ZEROS}]}}],"#),
        ]
        .concat();
        assert!(text.contains(&result), "{text}");
        // without a group-by the plain format answers
        let (text, _) = body(multinode, &finished(Method::Value, "options=minify", &values(&VALUES)));
        assert!(text.contains(r#""view":{"format":"full","#) && text.contains("dictionaries"), "{text}");
    }

    /// A limit below the number of groups keeps the best by the aggregation's score: the largest of raw weights
    /// (the method `value`, or `raw`), the smallest of spread ones; the summary counts groups. A limit that
    /// covers the groups leaves all of them, with a summary that is not truncated.
    #[test]
    fn a_limit_keeps_the_best_groups_by_the_aggregation() {
        // a: 0.5 and 0.125 (average 0.3125, min 0.125, max 0.5, sum 0.625); b: 0.25
        let found = values(&[(0, "a", 0.5), (0, "b", 0.25), (1, "a", 0.125)]);
        let kept = |method: Method, query: &str| {
            let (text, _) = body(grouped, &finished(method, query, &found));
            let ids: Vec<&str> = text.match_indices(r#"{"id":""#).map(|(at, _)| &text[at + 7..at + 8]).collect();
            (ids.concat(), text)
        };
        let base = "options=minify&group_by=dimension&limit=1";
        assert_eq!(kept(Method::Value, base).0, "a");
        assert_eq!(kept(Method::Value, &format!("{base}&aggregation=min")).0, "b");
        assert_eq!(kept(Method::Value, &format!("{base}&aggregation=max")).0, "a");
        assert_eq!(kept(Method::Value, &format!("{base}&aggregation=sum")).0, "a");
        assert_eq!(kept(Method::Value, &format!("{base}&aggregation=extremes")).0, "a");
        assert_eq!(kept(Method::Value, &format!("{base}&aggregation=percentage")).0, "a");
        // spread weights: the smallest score is the best
        assert_eq!(kept(Method::Ks2, base).0, "b");
        assert_eq!(kept(Method::Ks2, &format!("{base}&aggregation=min")).0, "a");
        assert_eq!(kept(Method::Ks2, "options=minify|raw&group_by=dimension&limit=1").0, "a");
        let (_, text) = kept(Method::Value, base);
        let tail = concat!(
            r#"}}],"correlated_dimensions":3,"total_dimensions_count":10,"result_limit":{"limit":1,"total":2,"#,
            r#""returned":1,"unit":"groups","truncated":true,"summary_scope":"all"}}"#
        );
        assert!(text.ends_with(tail), "{text}");
        let (ids, text) = kept(Method::Value, "options=minify&group_by=dimension&limit=2");
        assert_eq!(ids, "ab");
        assert!(text.ends_with(r#""returned":2,"unit":"groups","truncated":false,"summary_scope":"all"}}"#), "{text}");
    }

    /// Each aggregation ranks the groups by its own score. Of a (0.9 and 0.1), b (0.4 alone) and z (0.95 and
    /// 0.01) the largest average and the largest sum are a's, the largest minimum b's, the largest maximum z's;
    /// with b at 0.6 alone its average passes a's, whose sum stays the largest.
    #[test]
    fn each_aggregation_ranks_the_groups_by_its_own_score() {
        let kept = |found: &[(usize, &'static str, f64)], aggregation: &str| {
            let query = format!("options=minify&group_by=dimension&limit=1&aggregation={aggregation}");
            let (text, _) = body(grouped, &finished(Method::Value, &query, &values(found)));
            let head = r#""result":[{"id":""#;
            let at = text.find(head).expect("a group") + head.len();
            text[at..at + 1].to_owned()
        };
        let three = [(0, "a", 0.9), (1, "a", 0.1), (0, "b", 0.4), (0, "z", 0.95), (1, "z", 0.01)];
        let aggregations = ["average", "min", "max", "sum", "extremes", "percentage"];
        let winners = aggregations.map(|aggregation| kept(&three, aggregation));
        assert_eq!(winners, ["a", "b", "z", "a", "z", "a"]);
        let two = [(0, "a", 0.9), (1, "a", 0.1), (0, "b", 0.6)];
        assert_eq!((kept(&two, "average"), kept(&two, "sum")), ("b".to_owned(), "a".to_owned()));
    }

    /// Equal scores are ranked by id.
    #[test]
    fn groups_of_equal_score_are_ranked_by_id() {
        let found = values(&[(0, "b", 0.5), (0, "a", 0.5)]);
        let (text, _) = body(grouped, &finished(Method::Value, "options=minify&group_by=dimension&limit=1", &found));
        assert!(text.contains(r#""result":[{"id":"a","#) && !text.contains(r#"{"id":"b""#), "{text}");
    }
}
