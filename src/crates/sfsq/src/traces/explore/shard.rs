//! One source's contribution, from its index statistics.

use std::collections::BTreeSet;

use super::super::duration_hist::DurationHistogram;
use super::query::{ExploreQuery, HIDDEN_FIELDS, STATUS_FIELD};
use super::rows::{RowsSpec, SourceRows, source_rows};

/// A readable source's numbers for the request.
#[derive(Default)]
pub(super) struct ExploreShard {
    /// Scope rows in the window.
    pub matched: u64,
    /// Of which `status_code=ERROR`.
    pub errors: u64,
    /// Scope rows per bucket stacked by the stack field; `None` when the
    /// field is high-cardinality in this source (see `other`).
    pub timeline: Option<sfst::Timeline>,
    /// Per bucket: rows of this source counted without a value because the
    /// stack field is high-cardinality here.
    pub other: Vec<u64>,
    pub stack_high: bool,
    /// Per bucket: scope-row durations (only when percentiles are asked for).
    pub durations: Vec<DurationHistogram>,
    /// Scope-row counts per value of each faceted field.
    pub facets: Vec<sfst::FacetResult>,
    /// Faceted fields that are high-cardinality here.
    pub facet_high: BTreeSet<String>,
    /// Scope rows that can make the rows page.
    pub rows: Option<SourceRows>,
    /// This source's fields, for the field list.
    pub field_table: Option<sfst::FieldTable>,
}

/// How a readable source was evaluated.
pub(super) enum Evaluated {
    /// Written before the explorer's per-span entries: left out and reported,
    /// because counting it would silently report zero for it.
    Legacy,
    Shard(Box<ExploreShard>),
}

/// Whether a file was written before the explorer's per-span entries: it holds
/// rows but no `_role`.
pub(super) fn is_legacy(reader: &sfst::IndexReader<'_>) -> bool {
    reader.summary().record_count > 0 && !reader.field_table().contains(ng_flatten::ROLE_FIELD)
}

/// The scope's rows in one source: the chips, then the text and the trace ids.
fn compile_scope(
    reader: &sfst::IndexReader<'_>,
    query: &ExploreQuery,
) -> Result<sfst::BitmapFilter, sfst::Error> {
    let mut scope = reader.compile_filter(&query.scope.filter, None)?;
    if let Some(text) = &query.scope.text {
        scope = scope.conjoin(&reader.compile_text(text)?);
    }
    if !query.scope.trace_ids.is_empty() {
        scope = scope.conjoin(&reader.compile_trace_ids(&query.scope.trace_ids)?);
    }
    Ok(scope)
}

/// One evaluated source's rows candidates alone: for a page selected again
/// after another source failed to read its rows' fields.
pub(super) fn rows_of(
    bytes: &[u8],
    query: &ExploreQuery,
    spec: &RowsSpec,
    source: usize,
    stop: Option<i64>,
) -> Result<SourceRows, sfst::Error> {
    let reader = sfst::IndexReader::open(bytes)?;
    let scope = compile_scope(&reader, query)?;
    let window = query.grid.range_ns();
    let matched = reader.matched_count(&scope, window.clone())?;
    source_rows(&reader, &scope, window, matched, spec, stop, source)
}

/// Evaluate one source. Any error drops the whole source (its numbers are
/// never partly mixed in); the caller reports it. `source` is stamped on the
/// source's row candidates; `stop` is the rows page's so far (see
/// [`source_rows`]).
pub(super) fn evaluate(
    bytes: &[u8],
    query: &ExploreQuery,
    source: usize,
    stop: Option<i64>,
) -> Result<Evaluated, sfst::Error> {
    let reader = sfst::IndexReader::open(bytes)?;
    if is_legacy(&reader) {
        return Ok(Evaluated::Legacy);
    }

    let grid = query.grid;
    let window = grid.range_ns();
    let scope = compile_scope(&reader, query)?;
    let errors_only =
        reader.compile_filter(&sfst::Filter::new().select(STATUS_FIELD, "ERROR"), None)?;

    let mut shard = ExploreShard {
        matched: reader.matched_count(&scope, window.clone())?,
        errors: reader.matched_count(&scope.conjoin(&errors_only), window.clone())?,
        ..ExploreShard::default()
    };
    if let Some(histogram) = &query.sections.histogram {
        match reader.timeline(&histogram.stack, &scope, grid) {
            Ok(timeline) => shard.timeline = Some(timeline),
            Err(sfst::Error::HighCardFacet(_)) => {
                shard.other = reader.timeline_totals(&scope, grid)?;
                shard.stack_high = true;
            }
            Err(e) => return Err(e),
        }
        if histogram.percentiles {
            shard.durations = bucket_durations(&reader, &scope, grid)?;
        }
    }
    if let Some(spec) = &query.sections.facets {
        let table = reader.field_table();
        let mut eligible = Vec::new();
        let mut consider = |name: &str| match table.get(name) {
            Some(entry) if entry.is_high_card() => {
                shard.facet_high.insert(name.to_string());
            }
            Some(_) => eligible.push(name.to_string()),
            None => {}
        };
        match &spec.fields {
            Some(fields) => fields.iter().for_each(|f| consider(f)),
            None => table
                .names()
                .filter(|name| !HIDDEN_FIELDS.contains(name))
                .for_each(&mut consider),
        }
        shard.facets = reader.facets(&eligible, &scope, window.clone())?;
    }
    if let Some(spec) = &query.sections.rows {
        shard.rows = Some(source_rows(
            &reader,
            &scope,
            window,
            shard.matched,
            spec,
            stop,
            source,
        )?);
    }
    if query.sections.fields {
        shard.field_table = Some(reader.field_table().clone());
    }
    Ok(Evaluated::Shard(Box::new(shard)))
}

/// Scope-row durations per grid bucket: the scope's positions in the window
/// walked against each bucket's position range (both ascending).
fn bucket_durations(
    reader: &sfst::IndexReader<'_>,
    scope: &sfst::BitmapFilter,
    grid: sfst::Grid,
) -> Result<Vec<DurationHistogram>, sfst::Error> {
    let positions = reader.matched_positions(scope, grid.range_ns())?;
    let ranges = reader.load_timestamps()?.bucket_ranges(grid);
    let durations = reader.durations()?;
    let mut out = vec![DurationHistogram::new(); ranges.len()];
    let mut next = 0;
    for (bucket, (lo, hi)) in ranges.into_iter().enumerate() {
        while next < positions.len() && positions[next] < lo {
            next += 1;
        }
        while next < positions.len() && positions[next] < hi {
            let duration = durations
                .0
                .get(positions[next] as usize)
                .copied()
                .ok_or_else(|| {
                    sfst::Error::CorruptIndex(format!("no duration for row {}", positions[next]))
                })?;
            out[bucket].record(duration);
            next += 1;
        }
    }
    Ok(out)
}
