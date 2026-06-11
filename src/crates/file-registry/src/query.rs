use std::ops::Range;

use crate::ServiceStream;

/// A time-range + optional stream filter, used by `Registry::candidates`
/// implementations across the file-registry-backed sources (`sfst`,
/// `wal`, …) to identify which files satisfy a read.
///
/// The query is intentionally minimal: it carries only what the
/// registries can answer from their cheap inline summaries (per-file
/// `(min, max)` timestamps and stream identity), without opening any
/// file. Predicate pushdown for within-file selection is a separate
/// concern handled by the readers.
#[derive(Debug, Clone)]
pub struct Query {
    /// Time window of interest, in seconds since the Unix epoch.
    /// Inclusive lower bound, exclusive upper bound. A registry treats a
    /// file as a candidate if its `[min_timestamp, max_timestamp]` range
    /// overlaps `[start, end)`.
    pub time_range: Range<u32>,
    /// Stream filter. `None` matches every stream; `Some(s)` requires
    /// exact equality with the file's stream identity (or, for sources
    /// that only carry `ns_hash`, equality with
    /// `compute_ns_hash(s.namespace, s.name)`).
    pub stream: Option<ServiceStream>,
}

impl Query {
    /// Whether a file whose data spans `[min_s, max_s]` overlaps this
    /// query's window — see [`range_overlaps`] for the rule.
    pub fn overlaps(&self, min_s: u32, max_s: u32) -> bool {
        range_overlaps(&self.time_range, min_s, max_s)
    }
}

/// The one time-overlap rule every registry and catalog uses: a data
/// range `[min, max]` (inclusive on both ends) overlaps a query window
/// `[start, end)` (half-open) iff `max >= start && min < end`; an empty
/// window (`start >= end`) matches nothing.
///
/// Centralized because a drift between copies of this predicate means
/// silent query gaps — one source skipping files another would serve.
/// Generic over the unit so second-based (`u32`) and nanosecond-based
/// (`u64`) candidates share it.
pub fn range_overlaps<T: Ord + Copy>(window: &Range<T>, min: T, max: T) -> bool {
    if window.start >= window.end {
        return false;
    }
    max >= window.start && min < window.end
}
