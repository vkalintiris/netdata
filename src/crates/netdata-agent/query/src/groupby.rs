//! The v2 group-by: the passes' result objects and how metrics map into them (`rrd2rrdr_group_by_initialize()`,
//! `src/web/api/queries/query-group-by-init.c`), adding a metric or a previous pass's group, and the finalize that
//! runs the later passes, marks partial points, computes percentages, trims the live edge and averages
//! (`query-group-by-finalize.c`). Spec §8.2-8.4.

use std::collections::HashMap;

use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};

use crate::finalize::{DVIEW_ANOMALY_COUNT_MULTIPLIER, percentage_of_total};
use crate::rrdr::{GroupLabels, Rrdr, Trimming, value_flags};
use crate::tables::{Aggregation, TimeGrouping, group_by, options};
use crate::target::{QueryTarget, metric_status};
use crate::window::Window;

/// `MAX_QUERY_GROUP_BY_PASSES`.
pub const MAX_PASSES: usize = 2;
/// `GROUP_BY_MAX_LABEL_KEYS`.
pub const MAX_LABEL_KEYS: usize = 10;

/// `SUPPORTED_GROUP_BY_METHODS`.
const SUPPORTED: u32 = group_by::SELECTED
    | group_by::DIMENSION
    | group_by::INSTANCE
    | group_by::LABEL
    | group_by::NODE
    | group_by::CONTEXT
    | group_by::UNITS
    | group_by::PERCENTAGE_OF_INSTANCE;

/// `HGBC_PARTIAL_FLAG`: the top bit of a hidden contribution count marks a partial hidden source.
const HGBC_PARTIAL_FLAG: u32 = 1 << 31;
const HGBC_COUNT_MASK: u32 = !HGBC_PARTIAL_FLAG;

const HIDDEN_BUCKET: &str = "__hidden_dimensions__";

/// `GROUP_BY_HIDDEN_MODE`: where hidden metrics go at a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HiddenMode {
    /// With the selected ones; the percentage pass routes them to `vh` by flag.
    Normal,
    /// All into one query-wide bucket (no percentage of group).
    Global,
    /// Per-group shadow buckets, before the percentage pass.
    Shadow,
}

/// `query_target_aggregatable()`: `raw`.
pub fn aggregatable(window_options: u64) -> bool {
    window_options & options::RETURN_RAW != 0
}

/// `query_target_has_percentage_of_group()`: any pass, NONE ones included.
pub fn has_percentage_of_group(qt: &QueryTarget) -> bool {
    qt.request.group_by.iter().any(|g| {
        g.group_by & group_by::PERCENTAGE_OF_INSTANCE != 0
            || g.aggregation == Aggregation::Percentage
    })
}

/// `query_has_group_by_aggregation_percentage()`: the last pass before NONE aggregates by percentage and no pass
/// asks for percentage-of-instance.
pub fn has_aggregation_percentage(qt: &QueryTarget) -> bool {
    let mut last = false;
    for g in &qt.request.group_by {
        if g.group_by == group_by::NONE {
            break;
        }
        if g.group_by & group_by::PERCENTAGE_OF_INSTANCE != 0 {
            return false;
        }
        last = g.aggregation == Aggregation::Percentage;
    }
    last
}

/// `query_target_has_percentage_units()`.
pub fn has_percentage_units(qt: &QueryTarget, window_options: u64) -> bool {
    qt.request.time_group == TimeGrouping::Cv
        || (qt.request.options & options::PERCENTAGE != 0 && !aggregatable(window_options))
        || has_percentage_of_group(qt)
}

/// `group_by_pass_calculates_percentage()`.
fn pass_calculates_percentage(qt: &QueryTarget, pass: usize) -> bool {
    let g = &qt.request.group_by[pass];
    g.aggregation == Aggregation::Percentage || g.group_by & group_by::PERCENTAGE_OF_INSTANCE != 0
}

/// `group_by_pass_propagates_raw_contributor_counts()`.
fn pass_propagates_raw_contributor_counts(
    qt: &QueryTarget,
    window_options: u64,
    pass: usize,
) -> bool {
    !aggregatable(window_options)
        && qt.request.group_by[..=pass].iter().all(|g| {
            g.aggregation != Aggregation::Average
                && g.aggregation != Aggregation::Percentage
                && g.group_by & group_by::PERCENTAGE_OF_INSTANCE == 0
        })
}

/// The pieces a metric contributes to its group's key, id and name, in C's fixed order.
struct Parts<'a> {
    qt: &'a QueryTarget,
    d: usize,
    mode: HiddenMode,
    percentage_units: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Naming {
    Key,
    Id,
    Name,
}

impl Parts<'_> {
    fn hidden_bucket(&self) -> bool {
        self.qt.query[self.d].status & metric_status::HIDDEN != 0 && self.mode != HiddenMode::Normal
    }

    /// `query_group_by_make_dimension_key/id/name()`.
    fn make(&self, naming: Naming, gb: u32, label_keys: &[Vec<u8>]) -> Vec<u8> {
        let qt = self.qt;
        let qm = &qt.query[self.d];
        let qd = &qt.dimensions[qm.dimension];
        let qi = &qt.instances[qd.instance];
        let qc = &qt.contexts[qi.context];
        let host = &qt.nodes[qc.node].host;
        let mut out = Vec::new();
        if self.hidden_bucket() && self.mode == HiddenMode::Global {
            out.extend_from_slice(HIDDEN_BUCKET.as_bytes());
            return out;
        }
        if gb & group_by::SELECTED != 0 {
            out.extend_from_slice(b"selected");
            return out;
        }
        if self.hidden_bucket() {
            out.extend_from_slice(HIDDEN_BUCKET.as_bytes());
        }
        let part = |out: &mut Vec<u8>, text: &[u8]| {
            if naming == Naming::Key {
                out.push(b'|');
            } else if !out.is_empty() {
                out.push(b',');
            }
            out.extend_from_slice(text);
        };
        if gb & group_by::DIMENSION != 0 {
            part(&mut out, qd.rm.state().name.as_bytes());
        }
        if gb & (group_by::INSTANCE | group_by::PERCENTAGE_OF_INSTANCE) != 0 {
            let with_node = gb & group_by::NODE != 0;
            let text = match naming {
                Naming::Key => qi.id_fqdn.clone(),
                Naming::Id if with_node => qi.ri.id().to_string(),
                Naming::Id => qi.id_fqdn.clone(),
                Naming::Name if with_node => qi.ri.state().name,
                Naming::Name => qi.name_fqdn.clone(),
            };
            part(&mut out, text.as_bytes());
        }
        if gb & group_by::LABEL != 0 {
            let labels = qi.ri.labels();
            for key in label_keys {
                part(&mut out, labels.get(key).unwrap_or(b"[unset]"));
            }
        }
        if gb & group_by::NODE != 0 {
            let text = if naming == Naming::Name {
                host.hostname()
            } else {
                host.machine_guid().to_string()
            };
            part(&mut out, text.as_bytes());
        }
        if gb & group_by::CONTEXT != 0 {
            part(&mut out, qc.rc.id().as_bytes());
        }
        if gb & group_by::UNITS != 0 {
            let units = if self.percentage_units {
                "%".to_string()
            } else {
                qi.ri.state().units
            };
            part(&mut out, units.as_bytes());
        }
        out
    }
}

/// `struct rrdr_group_by_entry`.
struct Entry {
    priority: usize,
    count: u32,
    id: String,
    name: String,
    units: String,
    od: u32,
    dl: Option<GroupLabels>,
}

/// Adds every label of an instance to a group's label set (`rrdlabels_traversal_cb_to_group_by_label_key()`);
/// new keys also join the pass's key list.
fn add_group_labels(
    dl: &mut GroupLabels,
    keys: &mut Vec<Vec<u8>>,
    labels: &netdata_agent_rrd::labels::Labels,
) {
    for label in labels.iter() {
        let at = match dl.iter().position(|(k, _)| *k == label.name) {
            Some(at) => at,
            None => {
                if !keys.contains(&label.name) {
                    keys.push(label.name.clone());
                }
                dl.push((label.name.clone(), Vec::new()));
                dl.len() - 1
            }
        };
        if !dl[at].1.contains(&label.value) {
            dl[at].1.push(label.value.clone());
        }
    }
}

/// The passes' results (finest first) and the one-column result each metric executes into.
pub struct Grouped {
    pub passes: Vec<Rrdr>,
    pub r_tmp: Rrdr,
}

/// `rrd2rrdr_group_by_initialize()` for a v2 query: normalises `qt.request.group_by` and its label keys
/// (`qt.group_by_label_keys`), maps every metric to a group of every pass and creates the passes' results. `None`
/// when there is nothing to group.
pub fn initialize(qt: &mut QueryTarget, window: &Window) -> Option<Grouped> {
    let window_options = window.options;
    let mut label_keys: [Vec<Vec<u8>>; MAX_PASSES] = Default::default();
    for (g, keys) in label_keys.iter_mut().enumerate() {
        let pass = &mut qt.request.group_by[g];
        if pass.group_by & group_by::LABEL != 0
            && let Some(label) = pass.label.as_deref().filter(|l| !l.is_empty())
        {
            *keys = quoted_strings_splitter(label, MAX_LABEL_KEYS, Separators::GroupByLabel);
        }
        if keys.is_empty() {
            pass.group_by &= !group_by::LABEL;
        }
    }
    for g in 0..MAX_PASSES {
        if qt.request.group_by[g].group_by & SUPPORTED == 0 {
            qt.request.group_by[g].group_by = if g == 0 {
                group_by::DIMENSION
            } else {
                group_by::NONE
            };
        }
    }
    let has_pog = has_percentage_of_group(qt);

    // Every pass gains the groupings of the passes after it, so pass 0 is the finest.
    for g in 0..MAX_PASSES - 1 {
        let gb = qt.request.group_by[g].group_by;
        if gb == group_by::NONE {
            continue;
        }
        if gb == group_by::SELECTED {
            for r in g + 1..MAX_PASSES {
                qt.request.group_by[r].group_by = group_by::NONE;
            }
            continue;
        }
        for r in g + 1..MAX_PASSES {
            let later = qt.request.group_by[r].group_by;
            if later == group_by::NONE || later == group_by::SELECTED {
                continue;
            }
            if later & group_by::PERCENTAGE_OF_INSTANCE != 0 {
                qt.request.group_by[g].group_by |= group_by::INSTANCE;
            } else {
                qt.request.group_by[g].group_by |= later;
            }
            if later & group_by::LABEL != 0 {
                for key in label_keys[r].clone() {
                    if !label_keys[g].contains(&key)
                        && label_keys[g].len() < MAX_LABEL_KEYS * MAX_PASSES
                    {
                        label_keys[g].push(key);
                    }
                }
            }
        }
    }

    // The pass that consumes the hidden metrics for percentage of group; the scan stops at the first NONE.
    let mut pog_pass = None;
    let mut final_pass = 0;
    for g in 0..MAX_PASSES {
        let pass = &qt.request.group_by[g];
        if pass.group_by == group_by::NONE {
            break;
        }
        final_pass = g;
        if pog_pass.is_none()
            && (pass.group_by & group_by::PERCENTAGE_OF_INSTANCE != 0
                || pass.aggregation == Aggregation::Percentage)
        {
            pog_pass = Some(g);
        }
    }
    let final_pass_needs_arc = !aggregatable(window_options)
        && final_pass > 0
        && (qt.request.group_by[final_pass].aggregation == Aggregation::Average
            || qt.request.group_by[final_pass].aggregation == Aggregation::Percentage
            || qt.request.group_by[final_pass].group_by & group_by::PERCENTAGE_OF_INSTANCE != 0);
    let percentage_units = has_percentage_units(qt, window_options);

    let mut passes: Vec<Rrdr> = Vec::new();
    for (g, keys) in label_keys.iter().enumerate() {
        let gb = qt.request.group_by[g].group_by;
        let aggregation = qt.request.group_by[g].aggregation;
        if gb == group_by::NONE {
            break;
        }
        let final_grouping =
            g == MAX_PASSES - 1 || qt.request.group_by[g + 1].group_by == group_by::NONE;
        let mode = if !has_pog {
            HiddenMode::Global
        } else if pog_pass.is_some_and(|p| g < p) {
            HiddenMode::Shadow
        } else {
            HiddenMode::Normal
        };
        let with_labels = final_grouping && window_options & options::GROUP_BY_LABELS != 0;
        let mut pass_label_keys = Vec::new();
        let mut entries: Vec<Entry> = Vec::new();
        let mut groups: HashMap<Vec<u8>, usize> = HashMap::new();
        let mut hidden_dimensions = 0;
        let mut update_every_max = 0i64;
        let mut last_instance = None;
        for d in 0..qt.query.len() {
            let parts = Parts {
                qt,
                d,
                mode,
                percentage_units,
            };
            let qd = &qt.dimensions[qt.query[d].dimension];
            if last_instance != Some(qd.instance) {
                last_instance = Some(qd.instance);
                update_every_max = update_every_max.max(i64::from(
                    qt.instances[qd.instance].ri.state().update_every_s,
                ));
            }
            let priority = qd.priority;
            let status = qt.query[d].status;
            if status & metric_status::HIDDEN != 0 {
                hidden_dimensions += 1;
            }
            let key = parts.make(Naming::Key, gb, keys);
            let pos = match groups.get(&key) {
                Some(&pos) => pos,
                None => {
                    let pos = entries.len();
                    groups.insert(key, pos);
                    let text = |naming| {
                        String::from_utf8_lossy(&parts.make(naming, gb, keys)).into_owned()
                    };
                    entries.push(Entry {
                        priority,
                        count: 0,
                        id: text(Naming::Id),
                        name: text(Naming::Name),
                        units: qt.instances[qd.instance].ri.state().units,
                        od: 0,
                        dl: with_labels.then(GroupLabels::new),
                    });
                    pos
                }
            };
            let hidden_bucket = parts.hidden_bucket();
            let instance = qd.instance;
            let entry = &mut entries[pos];
            entry.count += 1;
            if priority < entry.priority {
                entry.priority = priority;
            }
            if let Some(dl) = entry.dl.as_mut() {
                add_group_labels(
                    dl,
                    &mut pass_label_keys,
                    &qt.instances[instance].ri.labels(),
                );
            }
            let qm = &mut qt.query[d];
            if g > 0 {
                let previous = passes.last_mut().expect("an earlier pass exists");
                previous.dgbs[qm.grouped_as.slot] = pos;
            } else {
                qm.grouped_as.first_slot = pos;
            }
            qm.grouped_as.slot = pos;
            qm.grouped_as.id = entry.id.clone();
            qm.grouped_as.name = entry.name.clone();
            qm.grouped_as.units = entry.units.clone();
            qm.status |= metric_status::GROUPED;
            // With percentage of group no hidden column reaches the result, except the buckets that route the
            // denominator.
            entry.od |= if has_pog && !hidden_bucket {
                qm.status & !metric_status::HIDDEN
            } else {
                qm.status
            };
        }

        let mut r = Rrdr::new(window, entries.len());
        if r.columns > 0 {
            r.dp = vec![0; r.columns];
            r.dview = vec![StoragePoint::default(); r.columns];
            r.dgbc = vec![0; r.columns];
            r.dqp = vec![StoragePoint::default(); r.columns];
            if !final_grouping {
                r.dgbs = vec![0; r.columns];
            }
            if with_labels {
                r.dl = Some(vec![GroupLabels::new(); r.columns]);
                r.label_keys = Some(std::mem::take(&mut pass_label_keys));
            }
            r.du = vec![String::new(); r.columns];
            if r.n > 0 {
                let cells = r.n * r.columns;
                r.gbc = vec![0; cells];
                if final_grouping && final_pass_needs_arc {
                    r.arc = vec![0; cells];
                }
                if hidden_dimensions > 0
                    && (gb & group_by::PERCENTAGE_OF_INSTANCE != 0
                        || aggregation == Aggregation::Percentage)
                {
                    r.vh = vec![f64::NAN; cells];
                    r.hgbc = vec![0; cells];
                }
            }
        }
        for (d, entry) in entries.into_iter().enumerate() {
            r.di[d] = entry.id;
            r.dn[d] = entry.name;
            r.od[d] = entry.od;
            r.du[d] = entry.units;
            r.dp[d] = entry.priority;
            r.dgbc[d] = entry.count;
            if let (Some(dl), Some(labels)) = (r.dl.as_mut(), entry.dl) {
                dl[d] = labels;
            }
        }
        let max_update_every = update_every_max * 2;
        r.trimming = Trimming {
            max_update_every,
            expected_after: if !aggregatable(window_options)
                && window.before >= qt.start_s - max_update_every
            {
                window.before - max_update_every
            } else {
                window.before
            },
            trimmed_after: window.before,
        };
        r.v.fill(f64::NAN);
        r.ar.fill(0.0);
        r.o.fill(value_flags::EMPTY);
        passes.push(r);
    }
    if passes.is_empty() {
        return None;
    }
    qt.group_by_label_keys = label_keys;
    Some(Grouped {
        passes,
        r_tmp: Rrdr::new(window, 1),
    })
}

/// How a source feeds its group: raw contributor counts propagated, anomaly contributors tracked.
#[derive(Clone, Copy, Default)]
pub struct AddMode {
    pub propagate_raw_contributor_counts: bool,
    pub track_raw_anomaly_contributors: bool,
}

/// `rrd2rrdr_group_by_add_metric_internal()`: adds column `d_src` of `src` (a metric, or a previous pass's group)
/// into column `d_dst` of `dst`; hidden sources feed the percentage denominator when `dst` has one.
pub fn add_metric(
    dst: &mut Rrdr,
    d_dst: usize,
    src: &Rrdr,
    d_src: usize,
    aggregation: Aggregation,
    query_points: &StoragePoint,
    mode: AddMode,
) {
    if src.od[d_src] & metric_status::QUERIED == 0 {
        return;
    }
    let to_hidden = src.od[d_src] & metric_status::HIDDEN != 0 && !dst.vh.is_empty();
    if !to_hidden {
        dst.od[d_dst] |= src.od[d_src];
        dst.dqp[d_dst].merge_to(query_points);
    }
    for i in 0..src.rows {
        let idx_src = src.index(i, d_src);
        let (x, o, ar) = (src.v[idx_src], src.o[idx_src], src.ar[idx_src]);
        if o & value_flags::EMPTY != 0 {
            continue;
        }
        let idx = dst.index(i, d_dst);
        let cn = if to_hidden {
            &mut dst.vh[idx]
        } else {
            &mut dst.v[idx]
        };
        match aggregation {
            Aggregation::Average | Aggregation::Sum | Aggregation::Percentage => {
                *cn = if cn.is_nan() { x } else { *cn + x };
            }
            Aggregation::Min => {
                if cn.is_nan() || x < *cn {
                    *cn = x;
                }
            }
            Aggregation::Max => {
                if cn.is_nan() || x > *cn {
                    *cn = x;
                }
            }
            Aggregation::Extremes => {
                if cn.is_nan() || x.abs() > cn.abs() {
                    *cn = x;
                }
            }
        }
        if !to_hidden {
            dst.o[idx] &= !value_flags::EMPTY;
            dst.o[idx] |= o & (value_flags::RESET | value_flags::PARTIAL);
            dst.ar[idx] += ar;
            dst.gbc[idx] += if mode.propagate_raw_contributor_counts {
                src.gbc[idx_src]
            } else {
                1
            };
            if mode.track_raw_anomaly_contributors {
                dst.arc[idx] += src.gbc[idx_src];
            }
        } else if !dst.hgbc.is_empty() {
            dst.hgbc[idx] += 1;
            if o & value_flags::PARTIAL != 0 {
                dst.hgbc[idx] |= HGBC_PARTIAL_FLAG;
            }
        }
    }
}

/// `rrd2rrdr_group_by_stamp_partial()`: a point with visible contributions that got fewer (visible or hidden)
/// than its column expects, or a partial hidden source, is PARTIAL.
fn stamp_partial(r: &mut Rrdr, expected_gbc: &[u32], expected_hgbc: &[u32]) {
    for i in 0..r.n {
        for d in 0..r.columns {
            let idx = r.index(i, d);
            let gbc = r.gbc[idx];
            if gbc == 0 {
                continue;
            }
            let hgbc = r.hgbc.get(idx).copied().unwrap_or(0);
            if gbc != expected_gbc[d]
                || hgbc & HGBC_COUNT_MASK != expected_hgbc[d]
                || hgbc & HGBC_PARTIAL_FLAG != 0
            {
                r.o[idx] |= value_flags::PARTIAL;
            }
        }
    }
}

/// `rrdr2rrdr_group_by_partial_trimming()`: from the last row before `expected_after`, the first later row whose
/// contributions drop (or vanish) ends the shown rows.
fn partial_trimming(r: &mut Rrdr) {
    let trimmable_after = r.trimming.expected_after;
    let Some(start) = (0..r.n).rev().find(|&i| r.t[i] < trimmable_after) else {
        return;
    };
    let mut last_row_gbc = 0;
    for i in start..r.n {
        let row_gbc: u64 = (0..r.columns)
            .filter(|&d| r.od[d] & metric_status::QUERIED != 0)
            .map(|d| u64::from(r.gbc[r.index(i, d)]))
            .sum();
        if r.t[i] >= trimmable_after && (row_gbc < last_row_gbc || row_gbc == 0) {
            r.trimming.trimmed_after = r.t[i];
            r.rows = i;
            break;
        }
        last_row_gbc = row_gbc;
    }
}

/// `rrdr2rrdr_group_by_calculate_percentage_of_group()`: each point becomes its share of itself plus its hidden
/// denominator.
fn percentage_of_group(r: &mut Rrdr, qt: &QueryTarget, window_options: u64) {
    if r.vh.is_empty() || (aggregatable(window_options) && has_aggregation_percentage(qt)) {
        return;
    }
    for idx in 0..r.n * r.columns {
        let (n, h) = (r.v[idx], r.vh[idx]);
        r.v[idx] = if n.is_nan() {
            0.0
        } else if h.is_nan() {
            100.0
        } else {
            let t = n + h;
            if t != 0.0 { n * 100.0 / t } else { 0.0 }
        };
    }
}

/// `rrd2rrdr_group_by_finalize()` for a v2 query: runs the later passes over the earlier ones, then the final
/// statistics, percentage of total and the instance counters. `window.options` loses NONZERO when no column is
/// nonzero.
pub fn finalize(qt: &mut QueryTarget, window: &mut Window, grouped: Grouped) -> Rrdr {
    let window_options = window.options;
    let mut passes = grouped.passes.into_iter();
    let mut last_r = passes.next().expect("at least one pass");
    let mut expected_gbc = vec![0u32; last_r.columns];
    let mut expected_hgbc = vec![0u32; last_r.columns];
    for qm in &qt.query {
        if qm.status & metric_status::HIDDEN != 0 && !last_r.vh.is_empty() {
            expected_hgbc[qm.grouped_as.first_slot] += 1;
        } else {
            expected_gbc[qm.grouped_as.first_slot] += 1;
        }
    }
    stamp_partial(&mut last_r, &expected_gbc, &expected_hgbc);
    if pass_calculates_percentage(qt, 0) {
        percentage_of_group(&mut last_r, qt, window_options);
    }
    for (pass, mut r) in (1..).zip(passes) {
        let propagate = pass_propagates_raw_contributor_counts(qt, window_options, pass);
        let aggregation = qt.request.group_by[pass].aggregation;
        let mut expected_gbc = vec![0u32; r.columns];
        let mut expected_hgbc = vec![0u32; r.columns];
        for d in 0..last_r.columns {
            let slot = last_r.dgbs[d];
            if last_r.od[d] & metric_status::HIDDEN != 0 && !r.vh.is_empty() {
                expected_hgbc[slot] += 1;
            } else {
                expected_gbc[slot] += if propagate { last_r.dgbc[d] } else { 1 };
            }
            let mode = if !r.arc.is_empty() {
                AddMode {
                    track_raw_anomaly_contributors: true,
                    ..AddMode::default()
                }
            } else {
                AddMode {
                    propagate_raw_contributor_counts: propagate,
                    ..AddMode::default()
                }
            };
            let query_points = last_r.dqp[d];
            add_metric(&mut r, slot, &last_r, d, aggregation, &query_points, mode);
        }
        stamp_partial(&mut r, &expected_gbc, &expected_hgbc);
        if pass_calculates_percentage(qt, pass) {
            percentage_of_group(&mut r, qt, window_options);
        }
        last_r = r;
    }
    let mut r = last_r;
    // The hidden contribution counters are internal to the passes.
    r.hgbc = Vec::new();

    let (mut aggregation, mut final_pass) = (qt.request.group_by[0].aggregation, 0);
    for (g, pass) in qt.request.group_by.iter().enumerate() {
        if pass.group_by != group_by::NONE {
            aggregation = pass.aggregation;
            final_pass = g;
        }
    }
    let final_percentage = pass_calculates_percentage(qt, final_pass);
    let use_arc = (aggregation == Aggregation::Average || final_percentage) && !r.arc.is_empty();
    let raw = aggregatable(window_options);
    if !raw && r.trimming.expected_after < window.before {
        partial_trimming(&mut r);
    }
    // Only AVERAGE accumulates pre-division sums; every other final aggregation averages its view over rows.
    let stats_by_rows = aggregation != Aggregation::Average && !raw;
    let (mut seen, mut global_min, mut global_max) = (false, f64::NAN, f64::NAN);
    let mut dimensions_nonzero = 0;
    for d in 0..r.columns {
        if r.od[d] & metric_status::QUERIED == 0 {
            continue;
        }
        let mut points_nonzero = 0;
        let (mut min, mut max, mut sum, mut ars) = (0.0, 0.0, 0.0, 0.0);
        let mut count: u64 = 0;
        for i in 0..r.n {
            let idx = r.index(i, d);
            let gbc = r.gbc[idx];
            let anomaly_contributors = if use_arc { r.arc[idx] } else { gbc };
            if gbc == 0 {
                continue;
            }
            r.o[idx] &= !value_flags::EMPTY;
            sum += r.v[idx];
            ars += if stats_by_rows {
                r.ar[idx] / f64::from(anomaly_contributors)
            } else {
                r.ar[idx]
            };
            let n = if aggregation == Aggregation::Average && !raw {
                r.v[idx] /= f64::from(gbc);
                r.v[idx]
            } else {
                r.v[idx]
            };
            if !raw {
                r.ar[idx] /= f64::from(anomaly_contributors);
            }
            // islessgreater(): NaN is neither less nor greater.
            if n.partial_cmp(&0.0)
                .is_some_and(|o| o != std::cmp::Ordering::Equal)
            {
                points_nonzero += 1;
            }
            if count == 0 {
                min = n;
                max = n;
            } else {
                if n < min {
                    min = n;
                }
                if n > max {
                    max = n;
                }
            }
            if !seen {
                seen = true;
                global_min = n;
                global_max = n;
            } else {
                if n < global_min {
                    global_min = n;
                }
                if n > global_max {
                    global_max = n;
                }
            }
            count += if stats_by_rows { 1 } else { u64::from(gbc) };
        }
        if points_nonzero > 0 {
            r.od[d] |= metric_status::NONZERO;
            dimensions_nonzero += 1;
        }
        r.dview[d] = StoragePoint {
            sum,
            // C: size_t into the uint32_t field.
            count: count as u32,
            min,
            max,
            anomaly_count: (ars * DVIEW_ANOMALY_COUNT_MULTIPLIER / 100.0) as u64 as u32,
            ..StoragePoint::default()
        };
    }
    r.arc = Vec::new();
    r.view.min = global_min;
    r.view.max = global_max;
    if dimensions_nonzero == 0 && window.options & options::NONZERO != 0 {
        window.options &= !options::NONZERO;
    }
    percentage_of_total(&mut r, window.options);

    // The instance counters of contexts and nodes.
    for i in 0..qt.instances.len() {
        let (context, metrics) = (qt.instances[i].context, qt.instances[i].metrics);
        let node = qt.contexts[context].node;
        if metrics.queried > 0 {
            qt.contexts[context].instances.queried += 1;
            qt.nodes[node].instances.queried += 1;
        } else if metrics.failed > 0 {
            qt.contexts[context].instances.failed += 1;
            qt.nodes[node].instances.failed += 1;
        }
    }
    r
}
