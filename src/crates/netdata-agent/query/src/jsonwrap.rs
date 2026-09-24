//! The v1 JSON wrapper around a result (`src/web/api/formatters/jsonwrap-v1.c`, with the helpers of `jsonwrap.c`,
//! `jsonwrap-query-plan.c`, the v1 summaries and `buffer_json_query_timings()`). Spec §9.5.

use std::collections::HashSet;
use std::time::Instant;

use netdata_agent_rrd::chart::ID_LENGTH_MAX;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::format::exposed;
use crate::rrdr::Rrdr;
use crate::tables::{options, options_to_json_array};
use crate::target::QueryTarget;

/// `JSKEY()`: the short key, or the long one with `long-json-keys`.
pub fn jskey(options: u64, short: &'static str, long: &'static str) -> &'static str {
    if options & options::LONG_JSON_KEYS != 0 {
        long
    } else {
        short
    }
}

/// The `"a:b"` keys C dedups by, cut like `snprintfz(buf, RRD_ID_LENGTH_MAX * 2 + 1, ...)`.
fn pair_key(a: &str, b: &str, max: usize) -> Vec<u8> {
    let mut key = format!("{a}:{b}").into_bytes();
    key.truncate(max);
    key
}

/// `rrdr_dimension_names()` / `rrdr_dimension_ids()`: one item per exposed column; returns the count.
fn column_strings(w: &mut JsonWriter, key: &str, r: &Rrdr, options: u64, names: bool) -> usize {
    w.member_add_array(Some(key.as_bytes()));
    let mut i = 0;
    for c in 0..r.columns {
        if !exposed(r.od[c], options) {
            continue;
        }
        w.add_array_item_string(if names { &r.dn[c] } else { &r.di[c] });
        i += 1;
    }
    w.array_close();
    i
}

/// `query_target_summary_instances_v1()`: `[id, name]` of every instance, deduplicated.
fn full_chart_list(w: &mut JsonWriter, qt: &QueryTarget) {
    w.member_add_array(Some(b"full_chart_list"));
    let mut seen = HashSet::new();
    for qi in &qt.instances {
        let (id, name) = (qi.ri.id(), qi.ri.state().name);
        if seen.insert(pair_key(id, &name, ID_LENGTH_MAX * 2 + 1)) {
            w.add_array_item_array();
            w.add_array_item_string(id);
            w.add_array_item_string(&name);
            w.array_close();
        }
    }
    w.array_close();
}

/// `query_target_summary_dimensions_v12(v2 = false)`: `[id, name]` of every dimension, deduplicated.
fn full_dimension_list(w: &mut JsonWriter, qt: &QueryTarget) {
    w.member_add_array(Some(b"full_dimension_list"));
    let mut seen = HashSet::new();
    for qd in &qt.dimensions {
        let (id, name) = (qd.rm.id(), qd.rm.state().name);
        if seen.insert(pair_key(id, &name, ID_LENGTH_MAX * 2 + 1)) {
            w.add_array_item_array();
            w.add_array_item_string(id);
            w.add_array_item_string(&name);
            w.array_close();
        }
    }
    w.array_close();
}

/// One label key of `full_chart_labels`: its values in first-seen order, each once.
struct LabelKey {
    name: Vec<u8>,
    values: Vec<Vec<u8>>,
    seen: HashSet<Vec<u8>>,
}

/// `query_target_summary_labels_v12(v2 = false)`: `[key, value]` of every instance label, grouped by key in the
/// order keys first appear, each pair once.
fn full_chart_labels(w: &mut JsonWriter, qt: &QueryTarget) {
    let mut keys: Vec<LabelKey> = Vec::new();
    for qi in &qt.instances {
        for label in qi.ri.labels().iter() {
            let at = match keys.iter().position(|k| k.name == label.name) {
                Some(at) => at,
                None => {
                    keys.push(LabelKey {
                        name: label.name.clone(),
                        values: Vec::new(),
                        seen: HashSet::new(),
                    });
                    keys.len() - 1
                }
            };
            let key = &mut keys[at];
            let mut pair = label.name.clone();
            pair.push(b':');
            pair.extend_from_slice(&label.value);
            pair.truncate(ID_LENGTH_MAX * 2);
            if key.seen.insert(pair) {
                key.values.push(label.value.clone());
            }
        }
    }
    w.member_add_array(Some(b"full_chart_labels"));
    for key in &keys {
        for value in &key.values {
            w.add_array_item_array();
            w.add_array_item_string(&key.name);
            w.add_array_item_string(value);
            w.array_close();
        }
    }
    w.array_close();
}

/// `jsonwrap_v1_chart_ids()`: the instance of every exposed metric.
fn chart_ids(w: &mut JsonWriter, r: &Rrdr, qt: &QueryTarget, options: u64) -> usize {
    w.member_add_array(Some(b"chart_ids"));
    let mut i = 0;
    for (c, qm) in qt.query.iter().enumerate() {
        if !exposed(r.od[c], options) {
            continue;
        }
        let qi = qt.dimensions[qm.dimension].instance;
        w.add_array_item_string(qt.instances[qi].ri.id());
        i += 1;
    }
    w.array_close();
    i
}

/// `query_target_chart_labels_filter_v1()`: per `chart_label_key` word, the label of every exposed metric (a
/// missing label adds nothing); returns the count of the last word.
fn chart_labels(w: &mut JsonWriter, r: &Rrdr, qt: &QueryTarget, options: u64) -> usize {
    w.member_add_object(b"chart_labels");
    let mut i = 0;
    let words = qt
        .chart_label_key
        .iter()
        .flat_map(|p| p.words().map_while(|word| word));
    for key in words {
        w.member_add_array(Some(key));
        i = 0;
        for (c, qm) in qt.query.iter().enumerate() {
            if !exposed(r.od[c], options) {
                continue;
            }
            let qi = qt.dimensions[qm.dimension].instance;
            if let Some(value) = qt.instances[qi].ri.labels().get(key) {
                w.add_array_item_string(value);
            }
            i += 1;
        }
        w.array_close();
    }
    w.object_close();
    i
}

/// `query_target_metrics_latest_values()`: the collector's last stored value of every exposed metric.
fn latest_values(w: &mut JsonWriter, r: &Rrdr, qt: &QueryTarget, options: u64) -> usize {
    w.member_add_array(Some(b"latest_values"));
    let mut i = 0;
    for (c, qm) in qt.query.iter().enumerate() {
        if !exposed(r.od[c], options) {
            continue;
        }
        let value = qt.dimensions[qm.dimension]
            .rm
            .dim()
            .map_or(f64::NAN, |dim| dim.collection().last_stored_value);
        w.add_array_item_double(value);
        i += 1;
    }
    w.array_close();
    i
}

/// `rrdr_dimension_view_latest_values()`: the newest row of every exposed column (`null`, or 0 with `null2zero`,
/// when empty or without rows).
fn view_latest_values(w: &mut JsonWriter, r: &Rrdr, options: u64) -> usize {
    w.member_add_array(Some(b"view_latest_values"));
    let mut i = 0;
    for c in 0..r.columns {
        if !exposed(r.od[c], options) {
            continue;
        }
        i += 1;
        if r.rows > 0 {
            let idx = r.index(r.rows - 1, c);
            if r.o[idx] & crate::rrdr::value_flags::EMPTY == 0 {
                w.add_array_item_double(r.v[idx]);
                continue;
            }
        }
        w.add_array_item_double(if options & options::NULL2ZERO != 0 {
            0.0
        } else {
            f64::NAN
        });
    }
    w.array_close();
    i
}

/// `jsonwrap_query_plan()`: each metric's plans and tiers.
fn query_plan(w: &mut JsonWriter, qt: &QueryTarget, options: u64) {
    let rfc3339 = options & options::RFC3339 != 0;
    w.member_add_object(b"query_plan");
    for qm in &qt.query {
        w.member_add_object(qt.dimensions[qm.dimension].rm.id());
        w.member_add_array(Some(b"plans"));
        if let Some((after, before)) = qm.plan {
            w.add_array_item_object();
            w.member_add_uint64(jskey(options, "tr", "tier"), 0);
            w.member_add_time_t_formatted(jskey(options, "af", "after"), after, rfc3339);
            w.member_add_time_t_formatted(jskey(options, "bf", "before"), before, rfc3339);
            w.object_close();
        }
        w.array_close();
        w.member_add_array(Some(b"tiers"));
        w.add_array_item_object();
        w.member_add_uint64(jskey(options, "tr", "tier"), 0);
        w.member_add_time_t_formatted(
            jskey(options, "fe", "first_entry"),
            qm.tier0.first_time_s,
            rfc3339,
        );
        w.member_add_time_t_formatted(
            jskey(options, "le", "last_entry"),
            qm.tier0.last_time_s,
            rfc3339,
        );
        // Tier weights are only computed with two tiers or more.
        w.member_add_int64(jskey(options, "wg", "weight"), 0);
        w.object_close();
        w.array_close();
        w.object_close();
    }
    w.object_close();
}

/// `buffer_json_query_timings()`: milliseconds from whole microseconds.
pub fn query_timings(w: &mut JsonWriter, key: &str, received: Instant, qt: &QueryTarget) {
    let finished = Instant::now();
    let executed = qt.executed.unwrap_or(finished);
    let ms =
        |to: Instant, from: Instant| to.saturating_duration_since(from).as_micros() as f64 / 1000.0;
    w.member_add_object(key);
    w.member_add_double("prep_ms", ms(qt.preprocessed, received));
    w.member_add_double("query_ms", ms(executed, qt.preprocessed));
    w.member_add_double("output_ms", ms(finished, executed));
    w.member_add_double("total_ms", ms(finished, received));
    w.member_add_double("cloud_ms", ms(finished, received));
    w.object_close();
}

/// `rrdr_json_wrapper_begin()`: a new writer holding every member before `result`. `options` are the window's.
pub fn begin_v1(r: &Rrdr, qt: &QueryTarget, options: u64) -> JsonWriter {
    let (kq, sq): (&[u8], &[u8]) = if options & options::GOOGLE_JSON != 0 {
        (b"", b"'")
    } else {
        (b"\"", b"\"")
    };
    let json_options = if options & options::MINIFY != 0 {
        JsonOptions::MINIFY
    } else {
        JsonOptions::DEFAULT
    };
    let mut w = JsonWriter::with_quotes(kq, sq, 0, true, json_options);
    let rfc3339 = options & options::RFC3339 != 0;
    let mut rows = r.rows;
    w.member_add_uint64("api", 1);
    w.member_add_string("id", &qt.id);
    w.member_add_string("name", &qt.id);
    w.member_add_time_t("view_update_every", r.view.update_every);
    w.member_add_time_t("update_every", qt.db.minimum_latest_update_every_s);
    w.member_add_time_t_formatted("first_entry", qt.db.first_time_s, rfc3339);
    w.member_add_time_t_formatted("last_entry", qt.db.last_time_s, rfc3339);
    w.member_add_time_t_formatted("after", r.view.after, rfc3339);
    w.member_add_time_t_formatted("before", r.view.before, rfc3339);
    w.member_add_string("group", qt.request.time_group.name());
    options_to_json_array(&mut w, b"options", options);
    if column_strings(&mut w, "dimension_names", r, options, true) == 0 {
        rows = 0;
    }
    if column_strings(&mut w, "dimension_ids", r, options, false) == 0 {
        rows = 0;
    }
    if options & options::ALL_DIMENSIONS != 0 {
        full_chart_list(&mut w, qt);
        full_dimension_list(&mut w, qt);
        full_chart_labels(&mut w, qt);
    }
    // Chart-scoped functions no longer exist; the key stays, always empty.
    w.member_add_array(Some(b"functions"));
    w.array_close();
    if !qt.chart_scoped && chart_ids(&mut w, r, qt, options) == 0 {
        rows = 0;
    }
    if qt.chart_label_key.is_some() && chart_labels(&mut w, r, qt, options) == 0 {
        rows = 0;
    }
    if latest_values(&mut w, r, qt, options) == 0 {
        rows = 0;
    }
    let dimensions = view_latest_values(&mut w, r, options);
    if dimensions == 0 {
        rows = 0;
    }
    w.member_add_uint64("dimensions", dimensions as u64);
    w.member_add_uint64("points", rows as u64);
    w.member_add_string("format", qt.request.format.name());
    w.member_add_array(Some(b"db_points_per_tier"));
    w.add_array_item_uint64(qt.db.tier0_points as u64);
    w.array_close();
    if options & options::DEBUG != 0 {
        query_plan(&mut w, qt, options);
    }
    w
}

/// `rrdr_json_wrapper_end()`.
pub fn end_v1(w: &mut JsonWriter, r: &Rrdr, qt: &QueryTarget, received: Instant) {
    w.member_add_double("min", r.view.min);
    w.member_add_double("max", r.view.max);
    query_timings(w, "timings", received, qt);
    w.finalize();
}
