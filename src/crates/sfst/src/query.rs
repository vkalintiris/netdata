//! Query primitives over an SFST index.
//!
//! This module exposes:
//!
//! - [`Filter`] — a netdata-style selection set: `field → allowed values`,
//!   with **OR within a field** and **AND across fields**.
//! - [`FacetResult`] — per-field `(value, count)` breakdown for the UI.
//! - [`Timeline`] — 2D time-bucket × dimension count grid for chart rendering.
//!
//! The query methods themselves ([`crate::IndexReader::facets`],
//! [`crate::IndexReader::timeline`]) live on the reader. They consume types
//! defined here.
//!
//! # Filter semantics
//!
//! `Filter` mirrors netdata's UI selection model:
//! `HashMap<field, Vec<value>>`. A log matches the filter iff, for every
//! field present in the selection, the log has at least one of the allowed
//! values for that field.
//!
//! Facet and timeline computations automatically *exclude* the field they
//! are computing for, so a selection on `PRIORITY=error` doesn't hide the
//! sibling values of the `PRIORITY` facet — see
//! [`Filter::without`].

use std::collections::BTreeMap;

/// A conjunction of per-field disjunctions.
///
/// `selections[field]` is the list of allowed values for `field`. A log
/// matches the filter iff for every entry `(field, values)` in
/// `selections`, the log's attribute for `field` is in `values`.
///
/// An empty `Filter` matches every log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub selections: BTreeMap<String, Vec<String>>,
}

impl Filter {
    pub fn new() -> Self {
        Self::default()
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

    /// Returns a copy of this filter with `field`'s entry removed.
    ///
    /// Used by facet and timeline computations to exclude a field's own
    /// selection when computing counts for that field — so a selection of
    /// `level=error` doesn't reduce the `level` facet to a single bar.
    pub fn without(&self, field: &str) -> Self {
        let mut s = self.selections.clone();
        s.remove(field);
        Self { selections: s }
    }

    /// True iff `field` has a selection entry.
    pub fn has_field(&self, field: &str) -> bool {
        self.selections.contains_key(field)
    }

    pub fn is_empty(&self) -> bool {
        self.selections.is_empty()
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
    pub fn range_ns(&self) -> std::ops::Range<i64> {
        self.bucket_start_ns..self.bucket_start_ns + self.bucket_width_ns * self.num_buckets as i64
    }
}

/// 2D time × dimension count grid for chart rendering.
///
/// `buckets[i]` corresponds to the time window described by
/// `grid` (see [`Grid`]). `buckets[i][j]` is the count for
/// dimension `dimensions[j]`.
///
/// `unset[i]` is the count of logs in bucket `i` that match the
/// (without-field) filter but **don't have the histogram field set**.
/// Computed as `filter_total_in_bucket - sum(buckets[i])`, exact
/// because OTel attribute keys are unique per LogRecord
/// (`common.proto §KeyValue`): every matching log either appears in
/// exactly one dimension or in `unset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub grid: Grid,
    pub dimensions: Vec<String>,
    pub buckets: Vec<Vec<u64>>,
    pub unset: Vec<u64>,
}

/// A single materialized log row: its timestamp plus the full set of
/// `(key, value)` attribute pairs stored for that position.
///
/// Produced by [`IndexReader::materialize_rows`](crate::IndexReader::materialize_rows).
/// Pairs appear in the order the position's `KvId`s were stored; keys
/// are not deduplicated (OTel attribute keys are unique per LogRecord,
/// so duplicates don't arise in practice).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterializedRow {
    pub timestamp_ns: i64,
    pub fields: Vec<(String, String)>,
}
