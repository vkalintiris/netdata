//! Query primitives over an SFST index.
//!
//! This module defines the inputs and outputs of the reader's query API;
//! the methods themselves ([`crate::IndexReader::facets`],
//! [`crate::IndexReader::timeline`], and the matched-count helpers) live on
//! the reader and consume these types.
//!
//! - [`Filter`] — a selection set (`field → allowed values`) with **OR
//!   within a field** and **AND across fields**.
//! - [`FacetResult`] — a per-field `(value, count)` breakdown: the
//!   distribution of a field's values across the matched logs.
//! - [`Grid`] / [`Timeline`] — a time-bucketed, per-value count grid for
//!   plotting a stacked time series of the matched logs.
//!
//! # Filter semantics
//!
//! A `Filter` is a `field → values` map. A log matches iff, for every field
//! present in the filter, the log's value for that field is one of the
//! allowed values — disjunction within a field, conjunction across fields.
//! An empty filter matches every log.
//!
//! When computing a facet or timeline *for* a field, that field's own
//! selection is excluded from the filter, so selecting `level=error`
//! doesn't collapse the `level` facet to a single value.

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

/// A conjunction of per-field disjunctions.
///
/// Each entry maps a `field` to its list of allowed values. A log matches
/// the filter iff for every entry `(field, values)`, the log's attribute
/// for `field` is in `values`.
///
/// An empty `Filter` matches every log. Build one with [`new`](Self::new) +
/// [`select`](Self::select) or [`From`] a `field → values` map; inspect it
/// with [`iter`](Self::iter), [`has_field`](Self::has_field), and
/// [`is_empty`](Self::is_empty).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    selections: BTreeMap<String, Vec<String>>,
}

impl Filter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Iterate the `(field, values)` selections — disjunction within a
    /// field, conjunction across fields. Fields are yielded in sorted
    /// order; within a field, values keep insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &Vec<String>)> {
        self.selections.iter()
    }

    /// Add `value` to the allowed values for `field`. Multiple values on
    /// the same field combine with OR; different fields combine with AND.
    pub fn select(mut self, field: impl Into<String>, value: impl Into<String>) -> Self {
        self.selections
            .entry(field.into())
            .or_default()
            .push(value.into());
        self
    }

    /// True iff `field` has a selection entry.
    pub fn has_field(&self, field: &str) -> bool {
        self.selections.contains_key(field)
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
    }
}

impl From<&HashMap<String, Vec<String>>> for Filter {
    /// Build a filter from a `field -> values` map (OR within a field, AND
    /// across fields).
    ///
    /// A field whose value list is empty is **skipped** (it adds no
    /// constraint), rather than being stored as an empty selection — an
    /// empty list would otherwise mean "OR of no values", collapsing the
    /// whole filter to match nothing. So a cleared field clears its filter.
    fn from(selections: &HashMap<String, Vec<String>>) -> Self {
        let mut filter = Filter::new();
        for (field, values) in selections {
            for value in values {
                filter = filter.select(field.clone(), value.clone());
            }
        }
        filter
    }
}

/// Per-field facet result. `values` is sorted by the order chunks
/// surface entries (FST iteration for low/mid-card fields).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetResult {
    pub field: String,
    pub values: Vec<(String, u32)>,
}

/// Bucket grid for a [`Timeline`] — the time geometry shared between
/// the caller, the reader, and any downstream merger.
///
/// `bucket i` covers `[bucket_start_ns + i * bucket_width_ns,
/// bucket_start_ns + (i + 1) * bucket_width_ns)`. Two `Grid` values
/// compare equal iff they describe the same buckets at the same
/// offsets — which is exactly the precondition for bucket-wise
/// merging of multi-file timelines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub bucket_start_ns: i64,
    pub bucket_width_ns: i64,
    pub num_buckets: usize,
}

impl Grid {
    pub fn new(bucket_start_ns: i64, bucket_width_ns: i64, num_buckets: usize) -> Self {
        Self {
            bucket_start_ns,
            bucket_width_ns,
            num_buckets,
        }
    }

    /// The half-open nanosecond range this grid covers:
    /// `bucket_start_ns .. bucket_start_ns + bucket_width_ns * num_buckets`.
    pub fn range_ns(&self) -> Range<i64> {
        self.bucket_start_ns..self.bucket_start_ns + self.bucket_width_ns * self.num_buckets as i64
    }
}

/// Per-log timestamps for one file: ascending nanoseconds, parallel-indexed
/// to log positions (position `p` has timestamp `at(p)`).
///
/// A sorted position↔time index. The windowed query paths use it both ways:
/// *time → position* ([`window`](Self::window), [`bucket_ranges`](Self::bucket_ranges))
/// and *position → time* ([`at`](Self::at)).
pub struct Timestamps(Vec<i64>);

impl Timestamps {
    /// Wrap the decoded per-log timestamps. They must be ascending (the
    /// on-disk TIMS chunk stores them in chronological order).
    pub(crate) fn new(timestamps: Vec<i64>) -> Self {
        Self(timestamps)
    }

    /// Number of logs (one timestamp per log position).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no logs.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The raw ascending timestamps, parallel-indexed to log positions.
    pub fn as_slice(&self) -> &[i64] {
        &self.0
    }

    /// Timestamp (ns) at log position `pos`, or `None` if out of range.
    pub fn at(&self, pos: u32) -> Option<i64> {
        self.0.get(pos as usize).copied()
    }

    /// First position whose timestamp is `>= t_ns` — i.e. the number of
    /// logs strictly before `t_ns`. Clamps to `len()` past the last log.
    fn lower_bound(&self, t_ns: i64) -> u32 {
        self.0.partition_point(|&t| t < t_ns) as u32
    }

    /// Position range `[lo, hi)` whose timestamps fall in `window`
    /// (`[start, end)`). Clamps naturally when the window extends past the
    /// file's range.
    pub fn window(&self, window: Range<i64>) -> (u32, u32) {
        (self.lower_bound(window.start), self.lower_bound(window.end))
    }

    /// Per-bucket position ranges `[lo, hi)` for `grid`. Each bucket edge is
    /// resolved once — a bucket's `hi` is the next bucket's `lo` — and a
    /// grid extending past the file's range yields empty outer buckets.
    pub fn bucket_ranges(&self, grid: Grid) -> Vec<(u32, u32)> {
        let edges: Vec<u32> = (0..=grid.num_buckets)
            .map(|b| {
                let edge_ns = grid.bucket_start_ns + (b as i64) * grid.bucket_width_ns;
                self.lower_bound(edge_ns)
            })
            .collect();
        edges.windows(2).map(|w| (w[0], w[1])).collect()
    }
}

/// A 2D time × value count grid: for each time bucket, the number of
/// matched logs carrying each value of the histogram field. Suitable for
/// plotting a stacked time series.
///
/// `buckets[i]` holds the counts for the `i`-th bucket of `grid` (see
/// [`Grid`]), in time order. The field-value labels live once in
/// `dimensions`, parallel to each [`Bucket::counts`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub grid: Grid,
    /// Field-value labels, parallel to each [`Bucket::counts`].
    pub dimensions: Vec<String>,
    /// One entry per `grid` bucket, in time order.
    pub buckets: Vec<Bucket>,
}

/// Counts for one time bucket of a [`Timeline`].
///
/// `counts[j]` is the number of matched logs in this bucket carrying value
/// [`dimensions[j]`](Timeline::dimensions); `unset` is the count of matched
/// logs in this bucket that don't have the field set at all.
///
/// `unset` is `bucket_total - sum(counts)`, exact because attribute keys are
/// unique per log record (as in an OpenTelemetry `LogRecord`,
/// `common.proto §KeyValue`): every matching log lands in exactly one
/// dimension or in `unset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    /// Per-dimension counts, parallel to [`Timeline::dimensions`].
    pub counts: Vec<u64>,
    /// Matched logs in this bucket lacking the field.
    pub unset: u64,
}

/// A single materialized log row: its timestamp plus the full set of
/// `(key, value)` attribute pairs stored for that position.
///
/// Produced by [`IndexReader::materialize_rows`](crate::IndexReader::materialize_rows).
/// Pairs appear in the order the position's `KvId`s were stored; keys
/// are not deduplicated (where attribute keys are unique per record, as in
/// an OpenTelemetry `LogRecord`, duplicates don't arise in practice).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedRow {
    pub timestamp_ns: i64,
    pub fields: Vec<(String, String)>,
}
