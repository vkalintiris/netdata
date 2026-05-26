//! Query primitives over an SFST index.
//!
//! This module exposes:
//!
//! - [`Filter`] — a netdata-style selection set: `field → allowed values`,
//!   with **OR within a field** and **AND across fields**.
//! - [`FacetResult`] — per-field `(value, count)` breakdown for the UI.
//! - [`Timeline`] — 2D time-bucket × dimension count grid for chart rendering.
//! - [`bitmap_value_to_roaring`] — convert an on-disk
//!   [`BitmapValue`](crate::BitmapValue) to a [`RoaringBitmap`] for set algebra.
//!
//! The query methods themselves ([`crate::IndexReader::evaluate`],
//! [`crate::IndexReader::facets`], [`crate::IndexReader::timeline`]) live on
//! the reader. They consume types defined here.
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

use roaring::RoaringBitmap;

use crate::BitmapValue;

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

/// 2D time × dimension count grid for chart rendering.
///
/// `buckets[i]` corresponds to the time window
/// `[bucket_start_ns + i * bucket_width_ns,
///   bucket_start_ns + (i + 1) * bucket_width_ns)`,
/// except the last bucket which is clamped to the file's max timestamp.
/// `buckets[i][j]` is the count for dimension `dimensions[j]`.
///
/// `unset[i]` is the count of logs in bucket `i` that match the
/// (without-field) filter but **don't have the histogram field set**.
/// Computed as `filter_total_in_bucket - sum(buckets[i])`, exact
/// because OTel attribute keys are unique per LogRecord
/// (`common.proto §KeyValue`): every matching log either appears in
/// exactly one dimension or in `unset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub bucket_start_ns: i64,
    pub bucket_width_ns: i64,
    pub dimensions: Vec<String>,
    pub buckets: Vec<Vec<u64>>,
    pub unset: Vec<u64>,
}

/// Convert an on-disk `BitmapValue` to a `RoaringBitmap`. `treight::Bitmap`
/// iterates set positions in ascending order, so the bulk-load path applies.
pub fn bitmap_value_to_roaring(bv: &BitmapValue) -> RoaringBitmap {
    RoaringBitmap::from_sorted_iter(bv.desc.iter(&bv.data))
        .expect("treight::Bitmap::iter yields sorted positions")
}
