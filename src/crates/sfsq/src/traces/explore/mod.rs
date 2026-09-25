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

mod compare;
mod groups;
mod live;
mod query;
mod rows;
mod run;
mod shard;
mod source;
mod values;

pub use crate::merge::MAX_FACET_VALUES;
pub use compare::{ComparisonTotals, FieldComparison, MIN_SUPPORT, ShareDiff, ValueComparison};
pub use groups::GROUPS_CAP;
pub use query::{
    DEFAULT_POPULATION, DEFAULT_STACK_FIELD, ExploreQuery, ExploreRequestError, ExploreScope,
    ExploreSelection, FacetSpec, HIDDEN_FIELDS, HistogramSpec, Sections, TRACE_IDS_MAX,
    UNSET_FACET_FIELDS,
};
pub use rows::{
    MoreRows, NOT_ROW_COLUMN_PREFIXES, ROW_COLUMNS_MAX, ROW_VALUE_COLUMNS, ROWS_PAGE_MAX,
    RowDirection, RowKey, RowOrder, RowsSpec, TOP_K_MAX, is_row_column,
};
pub use run::explore;
pub use source::ExploreOptions;
pub use values::{VALUES_LIMIT_MAX, ValuesData, ValuesQuery, field_values};

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
    pub groups: Option<GroupsData>,
    pub rows: Option<RowsData>,
    pub fields: Option<FieldsData>,
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
            groups: None,
            rows: None,
            fields: None,
        }
    }
}

/// Every span in the window of the traces with a span in scope, by service
/// and operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupsData {
    /// The source reasons plus this section's own.
    pub status: QueryStatus,
    /// The window's length, for rates.
    pub window_s: u64,
    /// Self time over every group, `other` included.
    pub self_ns_total: u128,
    /// The groups with the most spans, at most [`GROUPS_CAP`].
    pub rows: Vec<GroupRow>,
    /// The rest folded together; `None` when nothing was folded.
    pub other: Option<OtherGroups>,
    /// With a selection: the scope's traces on each side of it; every row
    /// and `other` then carry their sides.
    pub delta: Option<GroupsDelta>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub key: GroupKey,
    pub numbers: GroupNumbers,
    /// With a selection.
    pub delta: Option<GroupSides>,
}

/// The scope's traces split by a selection (QRY-20): a trace is on the
/// selection side when any of its scope rows is a selection row, on the
/// baseline side otherwise; a scope row without a trace id is its own trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GroupsDelta {
    pub selection_traces: u64,
    pub baseline_traces: u64,
    /// Self time over every group, `other` included, per side.
    pub selection_self_ns_total: u128,
    pub baseline_self_ns_total: u128,
}

/// A group's rows split by the side of their trace; the two add up to the
/// group's own numbers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroupSides {
    pub selection: SideNumbers,
    pub baseline: SideNumbers,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SideNumbers {
    pub spans: u64,
    pub errors_originated: u64,
    pub self_ns: u128,
}

/// A group: the rows' service and operation, `None` for rows without one.
/// Ordered by value with `None` after every value.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupKey {
    pub service: Option<String>,
    pub operation: Option<String>,
}

impl Ord for GroupKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        fn part(a: &Option<String>, b: &Option<String>) -> std::cmp::Ordering {
            match (a, b) {
                (Some(a), Some(b)) => a.cmp(b),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        }
        part(&self.service, &other.service).then_with(|| part(&self.operation, &other.operation))
    }
}

impl PartialOrd for GroupKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupNumbers {
    pub spans: u64,
    /// Of which `status_code=ERROR`.
    pub errors: u64,
    /// Of which carry `_err_origin=true`.
    pub errors_originated: u64,
    /// `None` without spans.
    pub p95_ns: Option<i64>,
    /// Summed self time; rows without child time add nothing.
    pub self_ns: u128,
}

/// The groups past [`GROUPS_CAP`], folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherGroups {
    /// How many groups were folded.
    pub groups: u64,
    pub numbers: GroupNumbers,
    /// With a selection.
    pub delta: Option<GroupSides>,
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
    /// With a selection: the scope and selection rows the comparison is out
    /// of; fields then come ranked first, values eligible first.
    pub comparison: Option<ComparisonTotals>,
}

/// One field's values, in lexicographic order, and what the value cap left
/// out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetData {
    pub field: String,
    pub values: Vec<FacetValue>,
    pub omitted_values: u64,
    pub omitted_rows: u64,
    pub comparison: Option<FieldComparison>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetValue {
    /// `None`: the rows without the field (on [`UNSET_FACET_FIELDS`] only),
    /// listed after the values.
    pub value: Option<String>,
    /// Scope rows.
    pub count: u64,
    pub comparison: Option<ValueComparison>,
}

/// Scope rows: a newest-first page, or the slowest K.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowsData {
    /// The source reasons plus this section's own.
    pub status: QueryStatus,
    /// The order asked for.
    pub order: RowOrder,
    /// Scope rows in the window (selection rows under a selection); every
    /// page reports the same number.
    pub matched: u64,
    /// For a newest page: whether rows exist beyond it on each side; `None`
    /// for the slowest order.
    pub more: Option<MoreRows>,
    /// The requested columns, in request order.
    pub columns: Vec<String>,
    pub items: Vec<Row>,
}

/// One span row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// Its content key; `encode` gives the page cursor.
    pub key: RowKey,
    pub duration_ns: i64,
    /// The duration less the time its direct children cover (in its file, or
    /// over its live WAL); `None` when a live WAL's live pass failed.
    pub self_duration_ns: Option<i64>,
    pub service: Option<String>,
    pub name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
    /// Parallel to [`RowsData::columns`]: every value, sorted.
    pub columns: Vec<Vec<String>>,
}

/// Every field of the window's readable files, and what the explorer can do
/// with each.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldsData {
    /// The source reasons.
    pub status: QueryStatus,
    /// By name, without [`HIDDEN_FIELDS`].
    pub items: Vec<FieldInfo>,
    /// Values every row carries besides its fields ([`ROW_VALUE_COLUMNS`]).
    pub columns: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInfo {
    pub name: String,
    /// The highest tier across files.
    pub tier: sfst::FieldTier,
    /// A scope chip may name it (every field).
    pub chip: bool,
    /// Facet values and stacking need it low or mid cardinality in every file.
    pub facet: bool,
    pub stack: bool,
    /// Text search looks at its values: not a plugin-added `_` field.
    pub text: bool,
    /// A rows column may show it: not an events or links field.
    pub column: bool,
}

impl FieldInfo {
    fn of(entry: sfst::FieldEntry) -> Self {
        let low_or_mid = !entry.is_high_card();
        FieldInfo {
            chip: true,
            facet: low_or_mid,
            stack: low_or_mid,
            text: !entry.name.starts_with('_'),
            column: rows::is_row_column(&entry.name),
            tier: entry.tier,
            name: entry.name,
        }
    }
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
