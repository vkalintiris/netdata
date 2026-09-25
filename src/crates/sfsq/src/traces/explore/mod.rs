//! The traces explorer's engine: one request answers the sections of the
//! explorer page over span rows (one row per stored span), from the same
//! per-file index statistics the logs engine uses. Wire-neutral: the Function
//! shape lives in `otel-ledger`.
//!
//! Every source that may hold rows for the window is either evaluated whole or
//! counted in the status: a file that fails to open or evaluate, or a WAL the
//! capture refused (`source_failure`), a file written before the explorer's
//! per-span entries (`legacy_file`), a remote file that could not be
//! downloaded (`remote_unavailable`). A source's numbers are never partly
//! mixed in.

mod query;
mod run;
mod shard;
mod source;

pub use query::{
    DEFAULT_POPULATION, DEFAULT_STACK_FIELD, ExploreQuery, ExploreRequestError, ExploreScope,
    FacetSpec, HIDDEN_FIELDS, HistogramSpec, Sections,
};
pub use run::explore;

/// The fixed duration bands the explorer stacks and selects by: one definition,
/// written at flatten time.
pub use ng_flatten::{DURATION_BAND_COUNT, DURATION_BAND_EDGES_NS, DURATION_BAND_LABELS};

use super::{PartialReason, QueryStatus, StatusBuilder};

/// One explorer answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExploreData {
    pub status: QueryStatus,
    /// Sources that may hold rows for the window: the "of" in "N of M files".
    pub sources: u64,
    pub histogram: Option<HistogramData>,
    pub facets: Option<FacetsData>,
}

impl ExploreData {
    fn cancelled() -> Self {
        let mut status = StatusBuilder::new();
        status.add(PartialReason::Cancelled);
        ExploreData {
            status: status.finish(),
            sources: 0,
            histogram: None,
            facets: None,
        }
    }
}

/// Scope rows per bucket, stacked by one field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramData {
    /// The source reasons plus this section's own.
    pub status: QueryStatus,
    pub stack: String,
    /// The stack field's values, lexicographic; each bucket's `counts` is
    /// parallel to it.
    pub dimensions: Vec<String>,
    pub buckets: Vec<StackBucket>,
    pub totals: Totals,
    /// Whether percentiles were computed (asked for).
    pub percentiles: bool,
}

/// One bucket: `counts` per value, `unset` for rows without the field, and
/// `other` for rows of sources where the field is high-cardinality. Their sum
/// is the bucket's scope rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackBucket {
    pub counts: Vec<u64>,
    pub unset: u64,
    pub other: u64,
    /// When asked for; `None` for a bucket without rows.
    pub percentiles: Option<Percentiles>,
}

/// Scope rows in the whole window, and how many are errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Totals {
    pub count: u64,
    pub errors: u64,
    /// When asked for; `None` for a window without rows.
    pub percentiles: Option<Percentiles>,
}

/// Values with their scope-row counts for each faceted field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetsData {
    /// The source reasons plus this section's own.
    pub status: QueryStatus,
    /// Requested fields in request order, or every eligible field in name
    /// order.
    pub fields: Vec<FacetData>,
    /// Requested fields that could not be faceted, and why.
    pub unavailable: Vec<(String, PartialReason)>,
}

/// One field's values, in lexicographic order, and what the value cap left
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetData {
    pub field: String,
    pub values: Vec<FacetValue>,
    pub omitted_values: u64,
    pub omitted_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetValue {
    pub value: String,
    pub count: u64,
}

/// Duration percentiles from the fixed histogram
/// ([`duration_hist`](super::duration_hist)): approximate, within its
/// maximum relative error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Percentiles {
    pub p50_ns: i64,
    pub p95_ns: i64,
    pub p99_ns: i64,
}

impl Percentiles {
    fn of(histogram: &super::duration_hist::DurationHistogram) -> Option<Self> {
        Some(Percentiles {
            p50_ns: histogram.percentile(50)?,
            p95_ns: histogram.percentile(95)?,
            p99_ns: histogram.percentile(99)?,
        })
    }
}
