//! Step 1: statistics (the aggregatable step).
//!
//! Computes a query's matched count, facets, histogram, and field table.
//! This step is an associative monoid: [`evaluate`] turns one candidate
//! file into a [`LogsShard`], and [`LogsShard::merge`] folds many shards
//! into one. Because the fold is associative with the
//! [`Default`](LogsShard::default) shard as identity, a node can merge the
//! files it owns and a parent can merge the children's shards with the
//! same function — the result is identical to merging every file at once.
//! That's the basis for fanning the query out across nodes without opening
//! every file in one place.

use super::engine::SfstCandidate;
use super::merge::{merge_facet_results, merge_field_tables, merge_timelines};
use super::query::{LogsQuery, build_filter};

/// Default histogram dimension when the query doesn't specify one.
/// Always `severity_text` — it's the OTel canonical log-level field,
/// and what makes a meaningful chart is the producer's responsibility
/// (set it, populate it with varied values). The consumer exposes the
/// full `available_fields` list for users to pick something else.
const DEFAULT_HISTOGRAM_FIELD: &str = "severity_text";

/// Default facet field when the query doesn't specify any. A consumer's
/// first-load request typically carries an empty facet list, so we can't
/// infer which fields the user cares about; rather than auto-curate a
/// set (which can't be done well across multiple SFSTs — a field's
/// cardinality composes unpredictably across files), we surface only
/// this one. Always `severity_text` — the OTel canonical log-level
/// field, same rationale as [`DEFAULT_HISTOGRAM_FIELD`]. Users add more
/// via an explicit `facet_fields`.
const DEFAULT_FACET_FIELD: &str = "severity_text";

/// One file's (or one node's) contribution to a query's statistics:
/// matched count, facets, histogram, and field table — everything in
/// step 1, with no materialized rows.
///
/// A shard is the unit of delegated work. [`evaluate`] produces one from
/// a single file; [`LogsShard::merge`] folds many into one. Because the
/// fold is an associative monoid, a node can merge the files it owns into
/// a single shard and a parent can merge those node-shards the same way —
/// the result is identical to merging every file at once.
#[derive(Debug, Default)]
pub struct LogsShard {
    /// Filter-matching logs within the window, summed across the shard.
    pub matched: u64,
    /// Per-field facet counts (unmerged across files until [`merge`]).
    ///
    /// [`merge`]: LogsShard::merge
    pub facets: Vec<sfst::FacetResult>,
    /// The histogram on the query grid, or `None` if this shard
    /// contributed none (histogram field high-card here, or the timeline
    /// errored). Merging keeps it `None` only when *no* shard had one.
    pub timeline: Option<sfst::Timeline>,
    /// The field table, all tiers kept and the tier bumped to `High` if
    /// high-card anywhere in the shard (see [`merge_field_tables`]).
    pub fields: sfst::FieldTable,
}

impl LogsShard {
    /// Fold per-file (or per-node) shards into one.
    ///
    /// `matched` sums; facets and timelines combine via the cross-file
    /// merge helpers; field tables merge associatively. Facets for a
    /// field that is high-card in *any* shard are dropped here — each
    /// shard's [`evaluate`] already skips a field high-card in its own
    /// file, and this completes the rule across shards so the facet set
    /// stays consistent with the offerable `available_fields`. The merged
    /// `timeline` is `None` only when no shard contributed one.
    ///
    /// The fold is associative and has an identity (the
    /// [`Default`](LogsShard::default) shard), so it is safe to apply at
    /// every level of a fan-out.
    pub fn merge(shards: Vec<LogsShard>) -> LogsShard {
        let mut matched: u64 = 0;
        let mut field_tables: Vec<sfst::FieldTable> = Vec::with_capacity(shards.len());
        let mut per_shard_facets: Vec<Vec<sfst::FacetResult>> = Vec::with_capacity(shards.len());
        let mut timelines: Vec<sfst::Timeline> = Vec::new();

        for shard in shards {
            matched += shard.matched;
            field_tables.push(shard.fields);
            per_shard_facets.push(shard.facets);
            if let Some(timeline) = shard.timeline {
                timelines.push(timeline);
            }
        }

        let fields = merge_field_tables(&field_tables);
        let facets = merge_facet_results(per_shard_facets)
            .into_iter()
            .filter(|facet| {
                !fields
                    .get(facet.field.as_str())
                    .is_some_and(|entry| entry.is_high_card())
            })
            .collect();
        let timeline = merge_timelines(timelines);

        LogsShard {
            matched,
            facets,
            timeline,
            fields,
        }
    }
}

/// Evaluate one candidate file into a [`LogsShard`] — step 1 for a single
/// file. Opens the file, computes the matched count, facets, histogram,
/// and field table against the query's [`grid`](LogsQuery::grid), and
/// returns a fully-owned shard (the reader is dropped before returning).
///
/// Any failure — unreadable/corrupt file, a per-computation error — is
/// logged and degrades that part to empty (an empty shard if the file
/// can't be opened), so one bad file never sinks the others when its
/// shard is merged.
///
/// Facets are picked against *this file's* table, so a field that's
/// high-card here is skipped; a field high-card in some *other* file is
/// dropped later, in [`LogsShard::merge`].
pub fn evaluate(candidate: &SfstCandidate, query: &LogsQuery) -> LogsShard {
    let bytes = match std::fs::read(&candidate.path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::warn!("sfsq: failed to read {}: {e}", candidate.path.display());
            return LogsShard::default();
        }
    };
    let reader = match sfst::IndexReader::open(&bytes) {
        Ok(reader) => reader,
        Err(e) => {
            tracing::warn!("sfsq: failed to open {}: {e}", candidate.path.display());
            return LogsShard::default();
        }
    };

    let grid = query.grid;
    let filter = build_filter(&query.selections);
    let fields = reader.field_table().clone();

    // matched: filter-matching logs restricted to the grid window.
    let matched = match per_file_matched(&reader, &filter, grid) {
        Ok(count) => count,
        Err(e) => {
            tracing::warn!(
                "sfsq: matched count failed for {}: {e}",
                candidate.path.display()
            );
            0
        }
    };

    // Facets: pick the requested set against this file's table (skipping
    // a field high-card *here*), then keep only fields actually present —
    // an unknown field would make `facets()` error and cost the whole
    // file.
    let facet_fields: Vec<String> = pick_facet_fields(&query.facet_fields, &fields)
        .into_iter()
        .filter(|name| fields.contains(name))
        .collect();
    let facets = match reader.facets(&facet_fields, &filter, grid.range_ns()) {
        Ok(facets) => facets,
        Err(e) => {
            tracing::warn!("sfsq: facets failed for {}: {e}", candidate.path.display());
            Vec::new()
        }
    };

    // Histogram: a file lacking the field yields a dimensionless timeline
    // whose matching logs all land in `unset`; only a high-card field
    // errors, in which case the file contributes no timeline.
    let histogram_field = pick_histogram_field(query.histogram_field.as_deref());
    let timeline = match reader.timeline(&histogram_field, &filter, grid) {
        Ok(timeline) => Some(timeline),
        Err(e) => {
            tracing::warn!(
                "sfsq: timeline failed for {}: {e}",
                candidate.path.display()
            );
            None
        }
    };

    LogsShard {
        matched,
        facets,
        timeline,
        fields,
    }
}

/// Per-file matched count: filter-matching logs restricted to the grid's
/// window. `evaluate` returns positions across the file's full range;
/// intersect with the window range bitmap (the same primitive `facets`
/// uses) to clip outside-window logs.
fn per_file_matched(
    reader: &sfst::IndexReader<'_>,
    filter: &sfst::Filter,
    grid: sfst::Grid,
) -> Result<u64, sfst::Error> {
    let bm = reader.evaluate(filter)?;
    let range = reader.range_bitmap(grid.range_ns())?;
    Ok((bm & &range).len())
}

/// Pick the histogram field. Honors the query's `histogram_field` when
/// set; otherwise returns [`DEFAULT_HISTOGRAM_FIELD`]. No eligibility
/// filtering — if the chosen field isn't in a given SFST or is
/// high-cardinality, `sfst::timeline` surfaces that as an error and the
/// file is skipped. A consumer can steer the user toward a different
/// field via [`LogsData::available_fields`](super::LogsData::available_fields).
pub(super) fn pick_histogram_field(requested: Option<&str>) -> String {
    requested.unwrap_or(DEFAULT_HISTOGRAM_FIELD).to_string()
}

/// Pick the facet field set. With no explicit request, return just
/// [`DEFAULT_FACET_FIELD`]; we don't try to auto-curate a wider set (see
/// that constant). Explicit `requested` fields are honored as-is, modulo
/// high-card / unknown fields (those would error or surface no options).
fn pick_facet_fields(requested: &[String], fields: &sfst::FieldTable) -> Vec<String> {
    if requested.is_empty() {
        return vec![DEFAULT_FACET_FIELD.to_string()];
    }
    requested
        .iter()
        .filter(|name| fields.get(name).is_some_and(|f| !f.is_high_card()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests;
