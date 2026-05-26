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
