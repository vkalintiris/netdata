//! The explorer's numbers by brute force: every span is looked at, every
//! count is an integer, nothing is indexed.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::{
    DURATION_BAND_FIELD, ERR_ORIGIN_FIELD, OracleSpan, ROLE_FIELD, SERVICE_FIELD, STATUS_FIELD,
};

/// Bucket widths the explorer may use, seconds.
const BUCKET_WIDTHS_S: [u32; 25] = [
    1, 2, 5, 10, 15, 30, 60, 120, 180, 300, 600, 900, 1800, 3600, 7200, 21600, 28800, 43200, 86400,
    172800, 259200, 432000, 604800, 1209600, 2592000,
];

/// The widest bucket that still gives at least this many buckets.
const TARGET_BUCKETS: u32 = 60;

/// A time window cut into equal buckets: `[after_s, before_s)`, both multiples
/// of `width_s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub after_s: u32,
    pub before_s: u32,
    pub width_s: u32,
}

impl Grid {
    /// The grid for a requested window: the widest listed width giving at
    /// least 60 buckets (else 1 s), `after` floored and `before` ceiled to it.
    pub fn for_window(after_s: u32, before_s: u32) -> Self {
        let span = before_s.saturating_sub(after_s);
        let mut width_s = 1;
        for w in BUCKET_WIDTHS_S {
            if span / w >= TARGET_BUCKETS {
                width_s = w;
            }
        }
        let after = after_s - after_s % width_s;
        let before = match before_s % width_s {
            0 => before_s,
            rest => before_s.saturating_add(width_s - rest),
        };
        Grid {
            after_s: after,
            before_s: before,
            width_s,
        }
    }

    pub fn buckets(&self) -> usize {
        ((self.before_s - self.after_s) / self.width_s) as usize
    }

    /// The bucket a row timestamp falls in, if inside the window.
    pub fn bucket_of(&self, start_ns: i64) -> Option<usize> {
        let after_ns = i64::from(self.after_s) * 1_000_000_000;
        let before_ns = i64::from(self.before_s) * 1_000_000_000;
        if start_ns < after_ns || start_ns >= before_ns {
            return None;
        }
        Some(((start_ns - after_ns) / (i64::from(self.width_s) * 1_000_000_000)) as usize)
    }
}

/// Field chips: a row matches when, for every field, it has at least one of
/// the listed values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub terms: BTreeMap<String, BTreeSet<String>>,
    /// A literal that must appear, case-insensitively, in some value of a field
    /// not starting with `_`.
    pub text: Option<String>,
    /// When non-empty, only these traces.
    pub trace_ids: BTreeSet<[u8; 16]>,
}

impl Scope {
    /// The explorer's default: spans where a request enters a service.
    pub fn entry_spans() -> Self {
        Scope::default().with("_role", &["root", "inbound"])
    }

    pub fn with(mut self, field: &str, values: &[&str]) -> Self {
        let set = self.terms.entry(field.to_string()).or_default();
        for v in values {
            set.insert(v.to_string());
        }
        self
    }

    pub fn with_text(mut self, text: &str) -> Self {
        self.text = Some(text.to_lowercase());
        self
    }

    pub fn with_trace_ids(mut self, ids: &[[u8; 16]]) -> Self {
        self.trace_ids.extend(ids.iter().copied());
        self
    }

    pub fn matches(&self, span: &OracleSpan) -> bool {
        if !self.trace_ids.is_empty()
            && !span.trace_id.is_some_and(|id| self.trace_ids.contains(&id))
        {
            return false;
        }
        if let Some(text) = &self.text {
            let mut found = false;
            for (field, values) in &span.fields {
                if field.starts_with('_') {
                    continue;
                }
                if values
                    .iter()
                    .any(|v| v.to_lowercase().contains(text.as_str()))
                {
                    found = true;
                    break;
                }
            }
            if !found {
                return false;
            }
        }
        for (field, wanted) in &self.terms {
            let Some(values) = span.fields.get(field) else {
                return false;
            };
            if values.is_disjoint(wanted) {
                return false;
            }
        }
        true
    }
}

/// One bucket of a stacked histogram: rows per value of the stack field, rows
/// without that field, and rows of units where the field is high (counted
/// without a value).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bucket {
    pub counts: BTreeMap<String, u64>,
    pub unset: u64,
    pub other: u64,
}

/// Rows in scope per bucket, stacked by `stack_field`; a row with several
/// values counts once under each; a row of a unit where the field is high
/// counts once as `other`.
pub fn histogram(
    spans: &[OracleSpan],
    grid: &Grid,
    scope: &Scope,
    stack_field: &str,
) -> Vec<Bucket> {
    let high = high_units(spans, stack_field);
    let mut buckets = vec![Bucket::default(); grid.buckets()];
    for span in spans {
        if !scope.matches(span) {
            continue;
        }
        let Some(index) = grid.bucket_of(span.start_ns) else {
            continue;
        };
        let bucket = &mut buckets[index];
        if high.contains(&span.unit) {
            bucket.other += 1;
            continue;
        }
        match span.fields.get(stack_field) {
            Some(values) => {
                for value in values {
                    *bucket.counts.entry(value.to_string()).or_default() += 1;
                }
            }
            None => bucket.unset += 1,
        }
    }
    buckets
}

/// Rows in scope inside the window, and how many of them are errors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub spans: u64,
    pub errors: u64,
}

pub fn totals(spans: &[OracleSpan], grid: &Grid, scope: &Scope) -> Totals {
    let mut out = Totals::default();
    for span in spans {
        if scope.matches(span) && grid.bucket_of(span.start_ns).is_some() {
            out.spans += 1;
            if span.is_error() {
                out.errors += 1;
            }
        }
    }
    out
}

/// The explorer's duration percentiles, from its documented fixed histogram:
/// zero and negative durations in bucket 0, one bucket per nanosecond below
/// 128, then 64 equal sub-buckets per power of two; a bucket reports its
/// midpoint (exact when one nanosecond wide); percentiles by nearest rank
/// (rank = ceil(q·n/100)).
pub mod fixed_histogram {
    use std::collections::BTreeMap;

    /// Largest relative error of a reported value, by definition.
    pub const MAX_RELATIVE_ERROR: f64 = 1.0 / 128.0;

    pub fn bucket(duration_ns: i64) -> u16 {
        if duration_ns <= 0 {
            return 0;
        }
        if duration_ns < 128 {
            return duration_ns as u16;
        }
        let mut exponent = 0u32;
        let mut rest = duration_ns;
        while rest > 1 {
            rest >>= 1;
            exponent += 1;
        }
        let top_seven_bits = (duration_ns >> (exponent - 6)) as u16;
        (exponent as u16 - 5) * 64 + (top_seven_bits - 64)
    }

    pub fn value(bucket: u16) -> i64 {
        if bucket < 128 {
            return i64::from(bucket);
        }
        let exponent = u32::from(bucket / 64) + 5;
        let width = 1u128 << (exponent - 6);
        let low = (64 + u128::from(bucket % 64)) * width;
        let mid = if width == 1 { low } else { low + width / 2 };
        i64::try_from(mid).unwrap_or(i64::MAX)
    }

    /// p50, p95 and p99 of `durations`, or `None` when there are none.
    pub fn percentiles(durations: &[i64]) -> Option<[i64; 3]> {
        if durations.is_empty() {
            return None;
        }
        let mut counts: BTreeMap<u16, u64> = BTreeMap::new();
        for d in durations {
            *counts.entry(bucket(*d)).or_default() += 1;
        }
        let n = durations.len() as u64;
        let mut out = [0i64; 3];
        for (slot, q) in [50u64, 95, 99].into_iter().enumerate() {
            let rank = (q * n).div_ceil(100).max(1);
            let mut seen = 0;
            for (bucket, count) in &counts {
                seen += count;
                if seen >= rank {
                    out[slot] = value(*bucket);
                    break;
                }
            }
        }
        Some(out)
    }

    /// The exact nearest-rank p50, p95 and p99, for checking the bound.
    pub fn exact_percentiles(durations: &[i64]) -> Option<[i64; 3]> {
        if durations.is_empty() {
            return None;
        }
        let mut sorted: Vec<i64> = durations.iter().map(|d| (*d).max(0)).collect();
        sorted.sort_unstable();
        let n = sorted.len() as u64;
        let mut out = [0i64; 3];
        for (slot, q) in [50u64, 95, 99].into_iter().enumerate() {
            let rank = (q * n).div_ceil(100).max(1);
            out[slot] = sorted[(rank - 1) as usize];
        }
        Some(out)
    }

    /// Whether approximate percentiles honour the documented bound: each
    /// within [`MAX_RELATIVE_ERROR`] of the exact one, and exactly 0 where the
    /// exact one is 0; both absent or both present.
    pub fn within_bound(approximate: Option<[i64; 3]>, exact: Option<[i64; 3]>) -> bool {
        match (approximate, exact) {
            (None, None) => true,
            (Some(approximate), Some(exact)) => approximate.into_iter().zip(exact).all(|(a, e)| {
                if e == 0 {
                    a == 0
                } else {
                    (a - e).abs() as f64 / e as f64 <= MAX_RELATIVE_ERROR
                }
            }),
            _ => false,
        }
    }
}

/// Rows in the window per value of `field`, counted under the scope without
/// its own terms on `field`; a row with several values counts once under each.
pub fn facet_counts(
    spans: &[OracleSpan],
    grid: &Grid,
    scope: &Scope,
    field: &str,
) -> BTreeMap<String, u64> {
    let mut others = scope.clone();
    others.terms.remove(field);
    let mut counts = BTreeMap::new();
    for span in spans {
        if !others.matches(span) || grid.bucket_of(span.start_ns).is_none() {
            continue;
        }
        if let Some(values) = span.fields.get(field) {
            for value in values {
                *counts.entry(value.to_string()).or_default() += 1;
            }
        }
    }
    counts
}

/// Values a facet lists at most; the rest are reported as omitted.
pub const FACET_VALUE_CAP: usize = 1_000;

/// A facet as the explorer returns it: values in byte order, and what the cap
/// left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facet {
    pub field: String,
    pub values: Vec<(String, u64)>,
    pub omitted_values: u64,
    pub omitted_rows: u64,
}

/// Keeps the [`FACET_VALUE_CAP`] values with the most rows (ties go to the
/// value first in byte order) and lists them in byte order.
pub fn capped(field: &str, counts: BTreeMap<String, u64>) -> Facet {
    let mut values: Vec<(String, u64)> = counts.into_iter().collect();
    let mut omitted_values = 0;
    let mut omitted_rows = 0;
    if values.len() > FACET_VALUE_CAP {
        values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        for (_, count) in values.drain(FACET_VALUE_CAP..) {
            omitted_values += 1;
            omitted_rows += count;
        }
        values.sort_by(|a, b| a.0.cmp(&b.0));
    }
    Facet {
        field: field.to_string(),
        values,
        omitted_values,
        omitted_rows,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Facets {
    pub fields: Vec<Facet>,
    /// Requested fields left out because they are high in some unit.
    pub unavailable: Vec<String>,
}

/// ORC-FACET over the rows of the units a window overlaps: the requested
/// fields in request order (one that is high in any unit is unavailable
/// instead; one without rows is empty), or by default every listed field of
/// those units that is not high in any of them, in name order.
pub fn facets(
    spans: &[OracleSpan],
    grid: &Grid,
    scope: &Scope,
    requested: Option<&[String]>,
) -> Facets {
    let tiers = field_list(spans);
    let high = |field: &str| tiers.get(field) == Some(&Tier::High);
    let mut out = Facets::default();
    let chosen: Vec<String> = match requested {
        Some(fields) => fields.to_vec(),
        None => tiers.keys().filter(|field| !high(field)).cloned().collect(),
    };
    for field in chosen {
        if high(&field) {
            out.unavailable.push(field);
        } else {
            let counts = facet_counts(spans, grid, scope, &field);
            out.fields.push(capped(&field, counts));
        }
    }
    out
}

/// A partial reason the explorer should report: its name, count, the
/// number it is out of, and the fields it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub reason: &'static str,
    pub count: u64,
    pub of: Option<u64>,
    pub detail: BTreeSet<String>,
}

/// The histogram's own partial reason: the stack field is high in some of the
/// window's `candidates` units.
pub fn histogram_reasons(spans: &[OracleSpan], stack_field: &str, candidates: u64) -> Vec<Reason> {
    let high = high_units(spans, stack_field);
    if high.is_empty() {
        return Vec::new();
    }
    vec![Reason {
        reason: "stack_field_high_card",
        count: high.len() as u64,
        of: Some(candidates),
        detail: BTreeSet::from([stack_field.to_string()]),
    }]
}

/// The facets section's own partial reasons: unavailable fields, then capped
/// ones.
pub fn facet_reasons(facets: &Facets) -> Vec<Reason> {
    let mut out = Vec::new();
    if !facets.unavailable.is_empty() {
        out.push(Reason {
            reason: "facet_high_card",
            count: facets.unavailable.len() as u64,
            of: None,
            detail: facets.unavailable.iter().cloned().collect(),
        });
    }
    let capped: BTreeSet<String> = facets
        .fields
        .iter()
        .filter(|facet| facet.omitted_values > 0)
        .map(|facet| facet.field.clone())
        .collect();
    if !capped.is_empty() {
        out.push(Reason {
            reason: "facet_value_cap",
            count: capped.len() as u64,
            of: None,
            detail: capped,
        });
    }
    out
}

/// Scope-row durations per bucket.
pub fn bucket_durations(spans: &[OracleSpan], grid: &Grid, scope: &Scope) -> Vec<Vec<i64>> {
    let mut buckets = vec![Vec::new(); grid.buckets()];
    for span in spans {
        if !scope.matches(span) {
            continue;
        }
        if let Some(index) = grid.bucket_of(span.start_ns) {
            buckets[index].push(span.duration_ns);
        }
    }
    buckets
}

/// A field's cardinality tier in one stored unit, from its distinct values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    Low,
    Mid,
    High,
}

/// Distinct values from which a field is mid; ten times as many make it high.
const TIER_THRESHOLD: usize = 100;

/// Fields the explorer never lists: the raw enum numbers behind `kind` and
/// `status_code`.
const UNLISTED_FIELDS: [&str; 2] = ["_kind", "_status_code"];

/// The traces core fields the plugin pins: never high, whatever their
/// values (the calculator's own copy of the documented list).
const PINNED_FIELDS: [&str; 6] = [
    SERVICE_FIELD,
    "name",
    STATUS_FIELD,
    "kind",
    ROLE_FIELD,
    DURATION_BAND_FIELD,
];

/// The tier of `field` with `distinct` values in one unit.
pub fn tier_of(field: &str, distinct: usize) -> Tier {
    if distinct < TIER_THRESHOLD {
        Tier::Low
    } else if distinct < 10 * TIER_THRESHOLD || PINNED_FIELDS.contains(&field) {
        Tier::Mid
    } else {
        Tier::High
    }
}

/// The units in which `field` is high.
pub fn high_units(spans: &[OracleSpan], field: &str) -> BTreeSet<usize> {
    let mut distinct: BTreeMap<usize, BTreeSet<&str>> = BTreeMap::new();
    for span in spans {
        if let Some(values) = span.fields.get(field) {
            let set = distinct.entry(span.unit).or_default();
            for value in values {
                set.insert(value);
            }
        }
    }
    let mut out = BTreeSet::new();
    for (unit, values) in distinct {
        if tier_of(field, values.len()) == Tier::High {
            out.insert(unit);
        }
    }
    out
}

/// What the explorer lets a listed field do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldFlags {
    pub chip: bool,
    pub facet: bool,
    pub stack: bool,
    pub text: bool,
    pub column: bool,
}

/// Any field makes a chip; a high one neither a facet nor a stack; a field
/// the plugin adds (leading `_`) is not searched as text; event and link
/// fields are not columns.
pub fn field_flags(field: &str, tier: Tier) -> FieldFlags {
    FieldFlags {
        chip: true,
        facet: tier != Tier::High,
        stack: tier != Tier::High,
        text: !field.starts_with('_'),
        column: !(field.starts_with("events.") || field.starts_with("links.")),
    }
}

/// Every listed field of the rows, with its highest tier across the stored
/// units (each unit classifies its fields alone).
pub fn field_list(spans: &[OracleSpan]) -> BTreeMap<String, Tier> {
    let mut distinct: BTreeMap<(usize, &str), BTreeSet<&str>> = BTreeMap::new();
    for span in spans {
        for (field, values) in &span.fields {
            let set = distinct.entry((span.unit, field)).or_default();
            for value in values {
                set.insert(value);
            }
        }
    }
    let mut out: BTreeMap<String, Tier> = BTreeMap::new();
    for ((_, field), values) in distinct {
        if UNLISTED_FIELDS.contains(&field) {
            continue;
        }
        let tier = tier_of(field, values.len());
        let entry = out.entry(field.to_string()).or_insert(tier);
        *entry = (*entry).max(tier);
    }
    out
}

/// The distinct values of `field` starting with `prefix` on the rows in the
/// window, in byte order.
pub fn field_values(
    spans: &[OracleSpan],
    grid: &Grid,
    field: &str,
    prefix: &str,
) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for span in spans {
        if grid.bucket_of(span.start_ns).is_none() {
            continue;
        }
        if let Some(values) = span.fields.get(field) {
            for value in values {
                if value.starts_with(prefix) {
                    out.insert(value.to_string());
                }
            }
        }
    }
    out
}

/// The distinct values of `field` starting with `prefix` on every row given
/// (the rows of the units a window overlaps): the most the explorer may
/// suggest.
pub fn unit_values(spans: &[OracleSpan], field: &str, prefix: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for span in spans {
        if let Some(values) = span.fields.get(field) {
            for value in values {
                if value.starts_with(prefix) {
                    out.insert(value.to_string());
                }
            }
        }
    }
    out
}

/// A row's content key: start, trace id, span id; an unset id is all zeros.
/// Newest order is this key descending, ids compared as bytes.
pub type RowKey = (i64, [u8; 16], [u8; 8]);

pub fn row_key(span: &OracleSpan) -> RowKey {
    (
        span.start_ns,
        span.trace_id.unwrap_or([0; 16]),
        span.span_id.unwrap_or([0; 8]),
    )
}

/// Which way a newest page walks from its anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    Older,
    Newer,
}

/// A page of the newest-first list, and whether rows exist past it on each
/// side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<'a> {
    pub rows: Vec<&'a OracleSpan>,
    pub has_older: bool,
    pub has_newer: bool,
}

/// Scope rows in the window, newest first.
fn newest_first<'a>(spans: &'a [OracleSpan], grid: &Grid, scope: &Scope) -> Vec<&'a OracleSpan> {
    let mut rows = Vec::new();
    for span in spans {
        if scope.matches(span) && grid.bucket_of(span.start_ns).is_some() {
            rows.push(span);
        }
    }
    rows.sort_by_key(|span| std::cmp::Reverse(row_key(span)));
    rows
}

/// `limit`, grown over the rows whose key equals the last one taken: rows with
/// one key are never split between pages.
fn page_len(rows: &[&OracleSpan], limit: usize) -> usize {
    let mut n = limit.min(rows.len());
    while n > 0 && n < rows.len() && row_key(rows[n]) == row_key(rows[n - 1]) {
        n += 1;
    }
    n
}

/// One page of the newest-first list: the first page when `anchor` is `None`
/// (walking older); otherwise the `limit` rows just past the anchor in `walk`'s
/// direction. The page itself is always newest first.
pub fn newest_page<'a>(
    spans: &'a [OracleSpan],
    grid: &Grid,
    scope: &Scope,
    limit: usize,
    anchor: Option<RowKey>,
    walk: Walk,
) -> Page<'a> {
    let rows = newest_first(spans, grid, scope);
    let mut side = Vec::new();
    let mut other = 0;
    for span in rows {
        let key = row_key(span);
        let on_side = match (anchor, walk) {
            (None, _) => true,
            (Some(anchor), Walk::Older) => key < anchor,
            (Some(anchor), Walk::Newer) => key > anchor,
        };
        if on_side {
            side.push(span);
        } else {
            other += 1;
        }
    }
    match walk {
        Walk::Older => {
            let n = page_len(&side, limit);
            Page {
                has_older: n < side.len(),
                has_newer: other > 0,
                rows: side[..n].to_vec(),
            }
        }
        Walk::Newer => {
            side.reverse();
            let n = page_len(&side, limit);
            let mut rows = side[..n].to_vec();
            rows.reverse();
            Page {
                rows,
                has_older: other > 0,
                has_newer: n < side.len(),
            }
        }
    }
}

/// The `k` slowest scope rows: duration descending, then start descending,
/// then trace id and span id ascending.
pub fn slowest<'a>(
    spans: &'a [OracleSpan],
    grid: &Grid,
    scope: &Scope,
    k: usize,
) -> Vec<&'a OracleSpan> {
    let mut rows = newest_first(spans, grid, scope);
    rows.sort_by_key(|span| {
        let (start, trace, span_id) = row_key(span);
        (
            std::cmp::Reverse(span.duration_ns),
            std::cmp::Reverse(start),
            trace,
            span_id,
        )
    });
    rows.truncate(k);
    rows
}

/// A row's values derived within its unit (one file, D25): whether it
/// originates an error, and the time its children cover.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Derived {
    /// ERROR, and no child is ERROR.
    pub error_origin: bool,
    /// The union of the children's intervals clipped to the row's own; the
    /// row's self time is its duration minus this.
    pub child_ns: i64,
}

/// Every row's derived values, index-parallel to `spans`. A row's children
/// are the rows of the same unit and set trace id whose set parent id is its
/// set span id; a row whose parent id is its own span id is nobody's child
/// (not even of its resent copy), and only direct children count.
pub fn derived(spans: &[OracleSpan]) -> Vec<Derived> {
    let mut children: BTreeMap<(usize, [u8; 16], [u8; 8]), Vec<usize>> = BTreeMap::new();
    for (index, span) in spans.iter().enumerate() {
        if let (Some(trace), Some(id), Some(parent)) =
            (span.trace_id, span.span_id, span.parent_span_id)
            && parent != id
        {
            children
                .entry((span.unit, trace, parent))
                .or_default()
                .push(index);
        }
    }

    let mut out = Vec::with_capacity(spans.len());
    for span in spans {
        let own_end = span.start_ns.saturating_add(span.duration_ns);
        let mut child_error = false;
        let mut intervals: Vec<(i64, i64)> = Vec::new();
        if let (Some(trace), Some(id)) = (span.trace_id, span.span_id)
            && let Some(list) = children.get(&(span.unit, trace, id))
        {
            for &index in list {
                let child = &spans[index];
                child_error |= child.is_error();
                let start = child.start_ns.max(span.start_ns);
                let end = child
                    .start_ns
                    .saturating_add(child.duration_ns)
                    .min(own_end);
                if start < end {
                    intervals.push((start, end));
                }
            }
        }
        intervals.sort();
        let mut child_ns: i64 = 0;
        let mut reached: Option<i64> = None;
        for (start, end) in intervals {
            let from = match reached {
                Some(reached) => start.max(reached),
                None => start,
            };
            if end > from {
                child_ns = child_ns.saturating_add(end - from);
            }
            reached = Some(reached.map_or(end, |reached| reached.max(end)));
        }
        out.push(Derived {
            error_origin: span.is_error() && !child_error,
            child_ns,
        });
    }
    out
}

/// Adds the seal's `_err_origin=true` token to the error-origin rows of the
/// units `sealed` accepts: sealed files store it; WAL chunk images and the
/// tail do not.
pub fn add_stored_origins(spans: &mut [OracleSpan], sealed: &dyn Fn(usize) -> bool) {
    let values = derived(spans);
    for (span, value) in spans.iter_mut().zip(values) {
        if value.error_origin && sealed(span.unit) {
            span.fields.insert(ERR_ORIGIN_FIELD, "true");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DURATION_BAND_FIELD, ROLE_FIELD, STATUS_FIELD};

    fn row(start_s: i64, fields: &[(&str, &str)]) -> OracleSpan {
        OracleSpan {
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            start_ns: start_s * 1_000_000_000,
            duration_ns: 0,
            fields: fields.iter().copied().collect(),
            unit: 0,
        }
    }

    /// A row of `unit` with ids as repeated bytes (0 = unset).
    fn linked(
        unit: usize,
        (trace, span, parent): (u8, u8, u8),
        start_ns: i64,
        duration_ns: i64,
        error: bool,
    ) -> OracleSpan {
        let status: &[(&str, &str)] = if error {
            &[(STATUS_FIELD, "ERROR")]
        } else {
            &[]
        };
        OracleSpan {
            trace_id: (trace != 0).then_some([trace; 16]),
            span_id: (span != 0).then_some([span; 8]),
            parent_span_id: (parent != 0).then_some([parent; 8]),
            start_ns,
            duration_ns,
            fields: status.iter().copied().collect(),
            unit,
        }
    }

    #[test]
    fn derived_rules() {
        const E: bool = true;
        const OK: bool = false;
        let near_max = i64::MAX - 10;
        // A name, the rows, and each row's (error origin, child time).
        type Case = (&'static str, Vec<OracleSpan>, Vec<(bool, i64)>);
        let cases: Vec<Case> = vec![
            (
                "an ERROR chain marks only its leaf",
                vec![
                    linked(0, (1, 1, 0), 0, 100, E),
                    linked(0, (1, 2, 1), 10, 50, E),
                    linked(0, (1, 3, 2), 20, 10, E),
                ],
                vec![(false, 50), (false, 10), (true, 0)],
            ),
            (
                "ERROR over OK over ERROR: two origins",
                vec![
                    linked(0, (1, 1, 0), 0, 100, E),
                    linked(0, (1, 2, 1), 10, 50, OK),
                    linked(0, (1, 3, 2), 20, 10, E),
                ],
                vec![(true, 50), (false, 10), (true, 0)],
            ),
            (
                "unset ids link nothing",
                vec![
                    linked(0, (0, 1, 0), 0, 100, E),
                    linked(0, (0, 2, 1), 10, 10, E),
                    linked(0, (1, 0, 0), 0, 100, E),
                ],
                vec![(true, 0), (true, 0), (true, 0)],
            ),
            (
                "a self-parent row and its resent copy are nobody's children",
                vec![
                    linked(0, (1, 1, 1), 0, 100, E),
                    linked(0, (1, 1, 1), 0, 100, E),
                ],
                vec![(true, 0), (true, 0)],
            ),
            (
                "every copy of a resent parent gets the child; duplicates count once",
                vec![
                    linked(0, (1, 1, 0), 0, 100, E),
                    linked(0, (1, 1, 0), 0, 100, E),
                    linked(0, (1, 2, 1), 10, 30, E),
                    linked(0, (1, 2, 1), 10, 30, E),
                ],
                vec![(false, 30), (false, 30), (true, 0), (true, 0)],
            ),
            (
                "overlaps count once and children are clipped",
                vec![
                    linked(0, (1, 1, 0), 100, 100, OK),
                    linked(0, (1, 2, 1), 50, 100, OK),
                    linked(0, (1, 3, 1), 120, 40, OK),
                    linked(0, (1, 4, 1), 180, 100, OK),
                    linked(0, (1, 5, 1), 170, 0, OK),
                ],
                vec![(false, 80), (false, 0), (false, 0), (false, 0), (false, 0)],
            ),
            (
                "ends past i64::MAX saturate",
                vec![
                    linked(0, (1, 1, 0), near_max, 100, OK),
                    linked(0, (1, 2, 1), near_max + 5, 100, OK),
                ],
                vec![(false, 5), (false, 0)],
            ),
            (
                "a cycle counts direct children only",
                vec![
                    linked(0, (1, 1, 2), 0, 100, E),
                    linked(0, (1, 2, 1), 10, 50, E),
                ],
                vec![(false, 50), (false, 50)],
            ),
            (
                "a child in another file or trace does not count",
                vec![
                    linked(0, (1, 1, 0), 0, 100, E),
                    linked(1, (1, 2, 1), 10, 50, E),
                    linked(0, (2, 3, 1), 10, 50, E),
                    linked(1, (1, 4, 0), 0, 100, E),
                    linked(0, (1, 5, 4), 10, 50, E),
                ],
                vec![(true, 0), (true, 0), (true, 0), (true, 0), (true, 0)],
            ),
        ];
        for (name, spans, expected) in cases {
            let got: Vec<(bool, i64)> = derived(&spans)
                .into_iter()
                .map(|d| (d.error_origin, d.child_ns))
                .collect();
            assert_eq!(got, expected, "{name}");
        }
    }

    #[test]
    fn grid_picks_the_widest_width_with_sixty_buckets_and_aligns_outward() {
        let cases: [(u32, u32, Grid); 4] = [
            (
                1_000_000,
                1_000_900,
                Grid {
                    after_s: 1_000_000 - 1_000_000 % 15,
                    before_s: 1_000_905,
                    width_s: 15,
                },
            ),
            (
                7_200,
                10_800,
                Grid {
                    after_s: 7_200,
                    before_s: 10_800,
                    width_s: 60,
                },
            ),
            (
                0,
                86_400,
                Grid {
                    after_s: 0,
                    before_s: 86_400,
                    width_s: 900,
                },
            ),
            (
                100,
                130,
                Grid {
                    after_s: 100,
                    before_s: 130,
                    width_s: 1,
                },
            ),
        ];
        for (after, before, want) in cases {
            assert_eq!(Grid::for_window(after, before), want, "{after}..{before}");
        }
    }

    #[test]
    fn histogram_counts_scope_rows_per_bucket_and_value() {
        let grid = Grid {
            after_s: 100,
            before_s: 130,
            width_s: 10,
        };
        let spans = [
            row(100, &[(ROLE_FIELD, "root"), (STATUS_FIELD, "ERROR")]),
            row(105, &[(ROLE_FIELD, "inbound")]),
            row(109, &[(ROLE_FIELD, "outbound"), (STATUS_FIELD, "OK")]),
            row(120, &[(ROLE_FIELD, "inbound"), (STATUS_FIELD, "OK")]),
            row(130, &[(ROLE_FIELD, "root"), (STATUS_FIELD, "ERROR")]),
            row(99, &[(ROLE_FIELD, "root")]),
        ];
        let got = histogram(&spans, &grid, &Scope::entry_spans(), STATUS_FIELD);
        let bucket = |pairs: &[(&str, u64)], unset| Bucket {
            counts: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            unset,
            other: 0,
        };
        assert_eq!(
            got,
            vec![
                bucket(&[("ERROR", 1)], 1),
                bucket(&[], 0),
                bucket(&[("OK", 1)], 0)
            ]
        );
        assert_eq!(
            totals(&spans, &grid, &Scope::entry_spans()),
            Totals {
                spans: 3,
                errors: 1
            }
        );
        let by_band = histogram(&spans, &grid, &Scope::default(), DURATION_BAND_FIELD);
        assert_eq!(by_band.iter().map(|b| b.unset).sum::<u64>(), 4);
    }

    #[test]
    fn fixed_histogram_follows_its_definition() {
        use fixed_histogram::{bucket, exact_percentiles, percentiles, value};
        let cases: [(i64, u16, i64); 7] = [
            (0, 0, 0),
            (127, 127, 127),
            (128, 128, 129),
            (256, 192, 258),
            (1_000_000, 954, 1_003_520),
            (10_000_000_000, 1_802, 9_999_220_736),
            (i64::MAX, 3_711, 9_187_343_239_835_811_840),
        ];
        for (duration, index, reported) in cases {
            assert_eq!(bucket(duration), index, "{duration}");
            assert_eq!(value(index), reported, "{duration}");
        }
        let durations: Vec<i64> = (1..=100).collect();
        assert_eq!(percentiles(&durations), Some([50, 95, 99]));
        assert_eq!(exact_percentiles(&durations), Some([50, 95, 99]));
        assert_eq!(percentiles(&[]), None);
    }

    fn span_at(start_s: i64, trace: u8, span: u8, duration_ns: i64) -> OracleSpan {
        OracleSpan {
            trace_id: Some([trace; 16]),
            span_id: Some([span; 8]),
            duration_ns,
            ..row(start_s, &[])
        }
    }

    fn keys(rows: &[&OracleSpan]) -> Vec<RowKey> {
        rows.iter().map(|span| row_key(span)).collect()
    }

    #[test]
    fn newest_pages_walk_by_key_and_never_split_a_key() {
        let grid = Grid::for_window(0, 60);
        // Newest first: a (30 s), b (20 s, trace 2), c and its resend d (20 s,
        // trace 1), e (10 s).
        let spans = [
            span_at(20, 1, 1, 0),
            span_at(30, 1, 1, 0),
            span_at(10, 1, 1, 0),
            span_at(20, 2, 1, 0),
            span_at(20, 1, 1, 0),
        ];
        let (a, b, c, e) = (
            row_key(&spans[1]),
            row_key(&spans[3]),
            row_key(&spans[0]),
            row_key(&spans[2]),
        );
        let all = Scope::default();
        let page = |limit, anchor, walk| {
            let page = newest_page(&spans, &grid, &all, limit, anchor, walk);
            (keys(&page.rows), page.has_older, page.has_newer)
        };
        assert_eq!(page(2, None, Walk::Older), (vec![a, b], true, false));
        assert_eq!(page(1, Some(b), Walk::Older), (vec![c, c], true, true));
        assert_eq!(page(3, Some(b), Walk::Older), (vec![c, c, e], false, true));
        assert_eq!(page(1, Some(e), Walk::Newer), (vec![c, c], true, true));
        assert_eq!(page(3, Some(c), Walk::Newer), (vec![a, b], true, false));
        assert_eq!(page(5, Some(a), Walk::Newer), (vec![], true, false));
    }

    #[test]
    fn slowest_orders_by_duration_then_later_start_then_ids() {
        let grid = Grid::for_window(0, 60);
        let spans = [
            span_at(10, 2, 1, 5),
            span_at(20, 9, 1, 5),
            span_at(10, 1, 1, 5),
            span_at(1, 1, 1, 9),
        ];
        let top = slowest(&spans, &grid, &Scope::default(), 3);
        assert_eq!(
            keys(&top),
            [row_key(&spans[3]), row_key(&spans[1]), row_key(&spans[2])]
        );
    }

    #[test]
    fn field_tiers_follow_distinct_values_per_unit() {
        for (field, distinct, tier) in [
            ("x", 1, Tier::Low),
            ("x", 99, Tier::Low),
            ("x", 100, Tier::Mid),
            ("x", 999, Tier::Mid),
            ("x", 1000, Tier::High),
            ("name", 1000, Tier::Mid),
            ("name", 50, Tier::Low),
            ("_kind", 1000, Tier::High),
        ] {
            assert_eq!(tier_of(field, distinct), tier, "{field} {distinct}");
        }
        let mut spans = Vec::new();
        for i in 0..150 {
            let value = i.to_string();
            spans.push(row(1, &[("x", &value), ("_kind", "2")]));
        }
        for value in ["a", "b", "c"] {
            let mut span = row(1, &[("x", value), ("y", value)]);
            span.unit = 1;
            spans.push(span);
        }
        assert_eq!(
            field_list(&spans),
            BTreeMap::from([("x".to_string(), Tier::Mid), ("y".to_string(), Tier::Low)]),
            "x is mid in unit 0 and low in unit 1; `_kind` is never listed"
        );
    }

    #[test]
    fn field_values_keep_the_window_rows_values_with_the_prefix() {
        let grid = Grid::for_window(60, 120);
        let spans = [
            row(70, &[("route", "/api/cart")]),
            row(80, &[("route", "/api/checkout")]),
            row(90, &[("route", "/health")]),
            row(10, &[("route", "/api/old")]),
        ];
        assert_eq!(
            field_values(&spans, &grid, "route", "/api/c"),
            BTreeSet::from(["/api/cart".to_string(), "/api/checkout".to_string()])
        );
        assert_eq!(field_values(&spans, &grid, "route", "").len(), 3);
        assert!(field_values(&spans, &grid, "nope", "").is_empty());
    }

    fn in_unit(unit: usize, mut span: OracleSpan) -> OracleSpan {
        span.unit = unit;
        span
    }

    /// Unit 1 holds 1,000 distinct values of `http.route`: high there only.
    fn two_units() -> Vec<OracleSpan> {
        let mut spans = vec![
            in_unit(0, row(100, &[(ROLE_FIELD, "root"), ("http.route", "/a")])),
            in_unit(0, row(101, &[(ROLE_FIELD, "root")])),
        ];
        for i in 0..1_000 {
            let route = format!("/r{i:04}");
            spans.push(in_unit(
                1,
                row(120, &[(ROLE_FIELD, "root"), ("http.route", &route)]),
            ));
        }
        spans
    }

    #[test]
    fn a_stack_field_high_in_one_unit_counts_that_units_rows_as_other() {
        let grid = Grid::for_window(90, 150);
        let buckets = histogram(&two_units(), &grid, &Scope::default(), "http.route");

        let mut want = vec![Bucket::default(); grid.buckets()];
        want[grid.bucket_of(100 * 1_000_000_000).unwrap()]
            .counts
            .insert("/a".to_string(), 1);
        want[grid.bucket_of(101 * 1_000_000_000).unwrap()].unset += 1;
        want[grid.bucket_of(120 * 1_000_000_000).unwrap()].other = 1_000;
        assert_eq!(buckets, want);
        assert_eq!(
            histogram_reasons(&two_units(), "http.route", 3),
            vec![Reason {
                reason: "stack_field_high_card",
                count: 1,
                of: Some(3),
                detail: BTreeSet::from(["http.route".to_string()]),
            }]
        );
        assert_eq!(histogram_reasons(&two_units(), ROLE_FIELD, 3), vec![]);
    }

    #[test]
    fn the_cap_keeps_the_values_with_most_rows_ties_first_in_byte_order() {
        let mut counts = BTreeMap::new();
        for i in 0..FACET_VALUE_CAP + 2 {
            counts.insert(format!("v{i:04}"), 1);
        }
        counts.insert("v0500".to_string(), 7);
        counts.insert("zz".to_string(), 3);

        let facet = capped("f", counts);

        assert_eq!(facet.values.len(), FACET_VALUE_CAP);
        assert_eq!((facet.omitted_values, facet.omitted_rows), (3, 3));
        assert_eq!(facet.values.first(), Some(&("v0000".to_string(), 1)));
        assert_eq!(facet.values.last(), Some(&("zz".to_string(), 3)));
        assert!(facet.values.contains(&("v0500".to_string(), 7)));
        assert!(!facet.values.iter().any(|(value, _)| value == "v1001"));
        assert!(facet.values.windows(2).all(|pair| pair[0].0 < pair[1].0));

        let few = capped(
            "f",
            BTreeMap::from([("b".to_string(), 1), ("a".to_string(), 2)]),
        );
        assert_eq!(
            few,
            Facet {
                field: "f".to_string(),
                values: vec![("a".to_string(), 2), ("b".to_string(), 1)],
                omitted_values: 0,
                omitted_rows: 0,
            }
        );
    }

    #[test]
    fn facets_leave_out_high_fields_and_name_what_they_leave_out() {
        let spans = two_units();
        let grid = Grid::for_window(90, 150);
        let requested = [
            "http.route".to_string(),
            "absent".to_string(),
            ROLE_FIELD.to_string(),
        ];

        let asked = facets(&spans, &grid, &Scope::default(), Some(&requested));

        assert_eq!(asked.unavailable, vec!["http.route".to_string()]);
        assert_eq!(
            asked.fields,
            vec![
                capped("absent", BTreeMap::new()),
                capped(ROLE_FIELD, BTreeMap::from([("root".to_string(), 1_002)])),
            ]
        );
        assert_eq!(
            facet_reasons(&asked),
            vec![Reason {
                reason: "facet_high_card",
                count: 1,
                of: None,
                detail: BTreeSet::from(["http.route".to_string()]),
            }]
        );

        let default = facets(&spans, &grid, &Scope::default(), None);
        assert_eq!(default.unavailable, Vec::<String>::new());
        let names: Vec<&str> = default.fields.iter().map(|f| f.field.as_str()).collect();
        assert_eq!(names, vec![ROLE_FIELD]);
    }

    #[test]
    fn a_capped_facet_is_named() {
        let mut spans = Vec::new();
        for i in 0..(FACET_VALUE_CAP + 1) {
            let name = format!("op{i:04}");
            spans.push(in_unit(i % 2, row(100, &[("name", &name)])));
        }
        let grid = Grid::for_window(90, 150);

        let got = facets(&spans, &grid, &Scope::default(), None);

        assert_eq!(
            (got.fields[0].values.len(), got.fields[0].omitted_values),
            (FACET_VALUE_CAP, 1)
        );
        assert_eq!(
            facet_reasons(&got),
            vec![Reason {
                reason: "facet_value_cap",
                count: 1,
                of: None,
                detail: BTreeSet::from(["name".to_string()]),
            }]
        );
    }

    #[test]
    fn field_flags_follow_the_tier_and_the_name() {
        let cases = [
            ("http.route", Tier::Mid, (true, true, true, true, true)),
            ("http.route", Tier::High, (true, false, false, true, true)),
            ("_role", Tier::Low, (true, true, true, false, true)),
            ("events.name", Tier::Low, (true, true, true, true, false)),
            (
                "links.attributes.x",
                Tier::Low,
                (true, true, true, true, false),
            ),
        ];
        for (field, tier, (chip, facet, stack, text, column)) in cases {
            assert_eq!(
                field_flags(field, tier),
                FieldFlags {
                    chip,
                    facet,
                    stack,
                    text,
                    column
                },
                "{field} {tier:?}"
            );
        }
    }

    #[test]
    fn unit_values_bound_the_window_values_from_above() {
        let spans = vec![
            row(100, &[("name", "get a")]),
            row(200, &[("name", "get b")]),
            row(100, &[("name", "put")]),
        ];
        let grid = Grid::for_window(90, 150);

        assert_eq!(
            field_values(&spans, &grid, "name", "get"),
            BTreeSet::from(["get a".to_string()])
        );
        assert_eq!(
            unit_values(&spans, "name", "get"),
            BTreeSet::from(["get a".to_string(), "get b".to_string()])
        );
    }

    #[test]
    fn the_percentile_bound_holds_within_one_part_in_128() {
        use fixed_histogram::within_bound;
        let cases = [
            ("equal", Some([100, 200, 300]), Some([100, 200, 300]), true),
            (
                "within the bound",
                Some([1_280, 200, 300]),
                Some([1_290, 200, 300]),
                true,
            ),
            (
                "past the bound",
                Some([1_300, 200, 300]),
                Some([1_280, 200, 300]),
                false,
            ),
            (
                "zero must stay zero",
                Some([1, 200, 300]),
                Some([0, 200, 300]),
                false,
            ),
            ("both absent", None, None, true),
            ("one absent", Some([1, 2, 3]), None, false),
        ];
        for (name, approximate, exact, holds) in cases {
            assert_eq!(within_bound(approximate, exact), holds, "{name}");
        }
    }
}
