//! Neutral query input for the log engine.
//!
//! [`LogsQuery`] is what [`run`](super::run) consumes: a plain
//! description of *what to match, how to bucket, and which page to
//! return* — no transport or wire concerns. A consumer parses its own
//! request format (HTTP params, CLI flags, a UI payload) and maps it
//! onto these types; the engine never sees the wire shape.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::cursor::Cursor;

/// A multi-file log query in engine terms.
///
/// The caller supplies the histogram [`grid`](LogsQuery::grid) outright:
/// the engine buckets the histogram on exactly that grid and clips every
/// count (matched, facets, the page) to its span. Deciding the bucket
/// geometry — window bounds, bucket width, bucket count, alignment — is a
/// presentation choice the consumer owns; the engine does not second-guess
/// it. The remaining fields are taken as-is: an empty `selections` matches
/// everything, an empty `facet_fields` requests the engine's default
/// facet, and a `None` `histogram_field` requests the engine's default
/// dimension.
#[derive(Debug, Clone)]
pub struct LogsQuery {
    /// Histogram bucket geometry. Its span (`bucket_start_ns` ..
    /// `bucket_start_ns + bucket_width_ns * num_buckets`) *is* the query
    /// window — counts and the materialized page are clipped to it.
    pub grid: sfst::Grid,
    /// Filter selections: OR within a field, AND across fields.
    pub selections: HashMap<String, Vec<String>>,
    /// Histogram dimension field; `None` requests the default.
    pub histogram_field: Option<String>,
    /// Facet fields to tabulate; empty requests the default facet.
    pub facet_fields: Vec<String>,
    /// Pagination anchor; `None` starts at the newest (backward) or
    /// oldest (forward) edge.
    pub anchor: Option<Anchor>,
    /// Page direction relative to the anchor.
    pub direction: Direction,
    /// Maximum number of rows to materialize for the page.
    pub limit: usize,
}

/// A pagination anchor in engine terms — already parsed from whatever
/// form the consumer's wire protocol used.
#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    /// A specific boundary row, from a prior page's [`Cursor`].
    Cursor(Cursor),
    /// A point in time (nanoseconds since epoch). Resolves to "the
    /// newest row at or before this instant".
    Timestamp(i64),
}

/// Page direction relative to the anchor. `Backward` walks toward older
/// rows (the default), `Forward` toward newer ones.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Forward,
    #[default]
    Backward,
}

/// Translate a query's `selections` map into an [`sfst::Filter`]. Same
/// shape, just a constructor walk: OR within field, AND across fields.
/// Shared by both query steps (`aggregate::evaluate` and
/// `page::evaluate_page`).
pub(super) fn build_filter(selections: &HashMap<String, Vec<String>>) -> sfst::Filter {
    let mut filter = sfst::Filter::new();
    for (field, values) in selections {
        for value in values {
            filter = filter.select(field.clone(), value.clone());
        }
    }
    filter
}
