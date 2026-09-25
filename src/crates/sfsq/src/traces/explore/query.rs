//! The explorer request: a window cut into buckets, the scope that picks the
//! spans, and the sections the page wants.

use super::super::sources::SourceSetError;
use super::super::window::WindowError;

/// Fully resolved explorer request. The window is `grid.range_ns()`; every
/// number clips to it by span start (`[start, end)`).
pub struct ExploreQuery {
    pub grid: sfst::Grid,
    pub scope: ExploreScope,
    pub sections: Sections,
}

/// The spans the page is about: field chips in storage names, exact values,
/// OR within a field and AND across fields. The engine applies no default;
/// the UI sends the entry-spans chip ([`DEFAULT_POPULATION`]).
pub struct ExploreScope {
    pub filter: sfst::Filter,
    /// A literal searched in every value (never keys or plugin fields).
    pub text: Option<sfst::text::LiteralText>,
    /// Keep only these traces' spans; empty means no trace-id term.
    pub trace_ids: Vec<sfst::TraceId>,
}

/// Which parts of the page to compute.
pub struct Sections {
    pub histogram: Option<HistogramSpec>,
    pub facets: Option<FacetSpec>,
    pub rows: Option<super::rows::RowsSpec>,
    /// The field list: every field of the window's files and what it supports.
    pub fields: bool,
}

/// The time histogram: rows per bucket, stacked by the values of one field,
/// and optionally each bucket's and the window's p50/p95/p99 durations.
pub struct HistogramSpec {
    pub stack: String,
    pub percentiles: bool,
}

/// Values with their scope-row counts, per field; a field's own chips do not
/// narrow its own counts.
pub struct FacetSpec {
    /// `None`: every field that is low or mid cardinality in every source.
    pub fields: Option<Vec<String>>,
}

/// Fields never offered as facets: the raw enum ints behind `kind` and
/// `status_code`.
pub const HIDDEN_FIELDS: [&str; 2] = ["_kind", "_status_code"];

/// Field the histogram is stacked by unless the request says otherwise.
pub const DEFAULT_STACK_FIELD: &str = STATUS_FIELD;

/// Storage names of the fields every row shows.
pub(super) const SERVICE_FIELD: &str = "resource.attributes.service.name";
pub(super) const NAME_FIELD: &str = "name";
pub(super) const STATUS_FIELD: &str = "status_code";

/// The scope the explorer opens with: spans where a request enters a service.
pub const DEFAULT_POPULATION: (&str, [&str; 2]) = (ng_flatten::ROLE_FIELD, ["root", "inbound"]);

/// A request the engine refuses before reading anything.
#[derive(Debug, thiserror::Error)]
pub enum ExploreRequestError {
    #[error(transparent)]
    SourceSet(#[from] SourceSetError),
    #[error(transparent)]
    Window(#[from] WindowError),
    #[error("{0}")]
    Invalid(String),
}

impl ExploreQuery {
    /// Checks the wire also makes; repeated here so the engine never runs a
    /// request it cannot answer.
    pub(super) fn validate(&self) -> Result<(), ExploreRequestError> {
        if self.grid.bucket_width_ns <= 0 || self.grid.num_buckets == 0 {
            return Err(ExploreRequestError::Invalid(
                "the grid needs a positive bucket width and at least one bucket".to_string(),
            ));
        }
        if self.scope.trace_ids.iter().any(|id| id.is_unset()) {
            return Err(ExploreRequestError::Invalid(
                "the all-zero trace id is not queryable".to_string(),
            ));
        }
        if let Some(histogram) = &self.sections.histogram
            && histogram.stack.is_empty()
        {
            return Err(ExploreRequestError::Invalid(
                "the histogram's stack field is empty".to_string(),
            ));
        }
        if let Some(rows) = &self.sections.rows {
            let max = match rows.order {
                super::rows::RowOrder::Newest { .. } => super::rows::ROWS_PAGE_MAX,
                super::rows::RowOrder::Slowest => super::rows::TOP_K_MAX,
            };
            if rows.limit == 0 || rows.limit > max {
                return Err(ExploreRequestError::Invalid(format!(
                    "rows need a limit of 1 to {max}"
                )));
            }
            if rows.columns.len() > super::rows::ROW_COLUMNS_MAX
                || !rows.columns.iter().all(|c| super::rows::is_row_column(c))
            {
                return Err(ExploreRequestError::Invalid(format!(
                    "rows take at most {} named span columns",
                    super::rows::ROW_COLUMNS_MAX
                )));
            }
            for (i, column) in rows.columns.iter().enumerate() {
                if rows.columns[..i].contains(column) {
                    return Err(ExploreRequestError::Invalid(format!(
                        "rows name the column `{column}` twice"
                    )));
                }
            }
        }
        Ok(())
    }
}
