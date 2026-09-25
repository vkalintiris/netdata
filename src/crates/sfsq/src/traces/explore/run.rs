//! Orchestration: prepare every source once, evaluate the readable ones,
//! count the rest, and assemble the sections.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use super::super::duration_hist::DurationHistogram;
use super::query::{ExploreQuery, ExploreRequestError};
use super::rows::{self, MoreRows, RowFields, RowsSpec, SourceRows};
use super::shard::{Evaluated, evaluate};
use super::source::{Prepared, prepare};
use super::{
    ExploreData, FacetData, FacetValue, FacetsData, HistogramData, Percentiles, Row, RowsData,
    StackBucket, Totals,
};
use crate::merge::{MergedFacet, merge_facets, merge_timelines};
use crate::source::Mapped;
use crate::traces::{PartialReason, StatusBuilder, TimeWindow, TraceSource, validate_sources};

/// Answer an explorer request over `sources`.
///
/// Pure sync: reads and decompresses files and builds the live tail's image;
/// run it off any async runtime thread. `progress` ticks once per source.
/// Cancellation is all-or-empty: a cancelled call returns no sections and
/// the `cancelled` reason.
pub fn explore(
    sources: Vec<TraceSource>,
    query: ExploreQuery,
    cancel: CancellationToken,
    progress: Arc<AtomicUsize>,
) -> Result<ExploreData, ExploreRequestError> {
    validate_sources(&sources)?;
    query.validate()?;
    let window_ns = query.grid.range_ns();
    let window = TimeWindow::new(window_ns.start, window_ns.end)?;
    let buckets = query.grid.num_buckets;

    let mut candidates = 0u64;
    let mut failed = 0u64;
    let mut legacy = 0u64;
    let mut unavailable = 0u64;
    let mut stack_high = 0u64;
    let mut matched = 0u64;
    let mut errors = 0u64;
    let mut timelines = Vec::new();
    let mut other = vec![0u64; buckets];
    let mut durations = vec![DurationHistogram::new(); buckets];
    let mut facets = Vec::new();
    let mut facet_high = BTreeSet::new();
    let mut page_rows = Vec::new();
    // Sources holding row candidates, kept open to read the page's fields; a
    // candidate's `source` indexes this list.
    let mut opened: Vec<(&TraceSource, Mapped)> = Vec::new();

    for source in &sources {
        if cancel.is_cancelled() {
            return Ok(ExploreData::cancelled());
        }
        match prepare(source, &window) {
            Prepared::Outside => {}
            Prepared::Unavailable => {
                candidates += 1;
                unavailable += 1;
            }
            Prepared::Failed(error) => {
                candidates += 1;
                failed += 1;
                tracing::warn!("sfsq traces: source {} failed: {error}", source.source_id());
            }
            Prepared::Open(mapped) => {
                candidates += 1;
                match evaluate(mapped.bytes(), &query, opened.len()) {
                    Ok(Evaluated::Legacy) => legacy += 1,
                    Ok(Evaluated::Shard(shard)) => {
                        matched += shard.matched;
                        errors += shard.errors;
                        if let Some(timeline) = shard.timeline {
                            timelines.push(timeline);
                        }
                        for (sum, histogram) in durations.iter_mut().zip(&shard.durations) {
                            sum.merge(histogram);
                        }
                        facets.push(shard.facets);
                        facet_high.extend(shard.facet_high);
                        if shard.stack_high {
                            stack_high += 1;
                            for (sum, n) in other.iter_mut().zip(&shard.other) {
                                *sum += n;
                            }
                        }
                        if let Some(rows) = shard.rows {
                            if !rows.candidates.is_empty() {
                                opened.push((source, mapped));
                            }
                            page_rows.push(rows);
                        }
                    }
                    Err(e) => {
                        failed += 1;
                        tracing::warn!(
                            "sfsq traces: source {} failed to evaluate: {e}",
                            source.source_id()
                        );
                    }
                }
            }
        }
        progress.fetch_add(1, Ordering::Relaxed);
    }
    if cancel.is_cancelled() {
        return Ok(ExploreData::cancelled());
    }

    // Reasons about sources hold for every section; each section adds its own.
    let mut shared = StatusBuilder::new();
    for (reason, count) in [
        (PartialReason::SourceFailure, failed),
        (PartialReason::RemoteUnavailable, unavailable),
        (PartialReason::LegacyFile, legacy),
    ] {
        shared.add_n(reason, count);
        shared.of(reason, candidates);
    }
    let mut status = shared.clone();

    let histogram = query.sections.histogram.map(|spec| {
        let mut own = StatusBuilder::new();
        own.add_n(PartialReason::StackFieldHighCard, stack_high);
        own.of(PartialReason::StackFieldHighCard, candidates);
        own.detail(PartialReason::StackFieldHighCard, spec.stack.clone());
        status.merge(own.clone());
        let mut section = shared.clone();
        section.merge(own);
        let (dimensions, stacked) = match merge_timelines(timelines) {
            Some(timeline) => (timeline.dimensions, timeline.buckets),
            None => (Vec::new(), Vec::new()),
        };
        let percentiles = |histogram: &DurationHistogram| {
            if spec.percentiles {
                Percentiles::of(histogram)
            } else {
                None
            }
        };
        let mut out = Vec::with_capacity(buckets);
        let mut window_durations = DurationHistogram::new();
        for (index, (other, bucket_durations)) in other.into_iter().zip(&durations).enumerate() {
            let (counts, unset) = match stacked.get(index) {
                Some(bucket) => (bucket.counts.clone(), bucket.unset),
                None => (vec![0; dimensions.len()], 0),
            };
            window_durations.merge(bucket_durations);
            out.push(StackBucket {
                counts,
                unset,
                other,
                percentiles: percentiles(bucket_durations),
            });
        }
        HistogramData {
            status: section.finish(),
            stack: spec.stack,
            dimensions,
            buckets: out,
            totals: Totals {
                count: matched,
                errors,
                percentiles: percentiles(&window_durations),
            },
            percentiles: spec.percentiles,
        }
    });

    let facets = query.sections.facets.map(|spec| {
        let mut own = StatusBuilder::new();
        let mut merged: BTreeMap<String, MergedFacet> = merge_facets(facets)
            .into_iter()
            .map(|facet| (facet.field.clone(), facet))
            .collect();
        let mut fields = Vec::new();
        let mut unavailable = Vec::new();
        match spec.fields {
            Some(requested) => {
                for field in requested {
                    if facet_high.contains(&field) {
                        own.add(PartialReason::FacetHighCard);
                        own.detail(PartialReason::FacetHighCard, field.clone());
                        unavailable.push((field, PartialReason::FacetHighCard));
                    } else {
                        let facet = merged.remove(&field).unwrap_or(MergedFacet {
                            field,
                            values: Vec::new(),
                            omitted_values: 0,
                            omitted_rows: 0,
                        });
                        fields.push(facet);
                    }
                }
            }
            None => fields.extend(
                merged
                    .into_values()
                    .filter(|facet| !facet_high.contains(&facet.field)),
            ),
        }
        let mut out = Vec::with_capacity(fields.len());
        for facet in fields {
            if facet.omitted_values > 0 {
                own.add(PartialReason::FacetValueCap);
                own.detail(PartialReason::FacetValueCap, facet.field.clone());
            }
            let mut values = Vec::with_capacity(facet.values.len());
            for (value, count) in facet.values {
                values.push(FacetValue { value, count });
            }
            out.push(FacetData {
                field: facet.field,
                values,
                omitted_values: facet.omitted_values,
                omitted_rows: facet.omitted_rows,
            });
        }
        status.merge(own.clone());
        let mut section = shared.clone();
        section.merge(own);
        FacetsData {
            status: section.finish(),
            fields: out,
            unavailable,
        }
    });

    let rows = query.sections.rows.map(|spec| {
        let (items, more, own) = rows_section(&spec, page_rows, &opened, candidates);
        status.merge(own.clone());
        let mut section = shared.clone();
        section.merge(own);
        RowsData {
            status: section.finish(),
            matched,
            more,
            columns: spec.columns,
            items,
        }
    });

    Ok(ExploreData {
        status: status.finish(),
        sources: candidates,
        histogram,
        facets,
        rows,
    })
}

/// The page from every source's candidates, with its fields read from the
/// sources holding it, and this section's own reasons: a source whose fields
/// cannot be read is left out of the page and counted as failed.
fn rows_section(
    spec: &RowsSpec,
    per_source: Vec<SourceRows>,
    opened: &[(&TraceSource, Mapped)],
    candidates: u64,
) -> (Vec<Row>, Option<MoreRows>, StatusBuilder) {
    let (page, more) = rows::select_page(spec, per_source);
    let mut by_source: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, candidate) in page.iter().enumerate() {
        by_source.entry(candidate.source).or_default().push(index);
    }
    let mut fields: Vec<Option<RowFields>> = vec![None; page.len()];
    let mut own = StatusBuilder::new();
    for (source, indexes) in by_source {
        let (trace_source, mapped) = &opened[source];
        let mut positions = Vec::with_capacity(indexes.len());
        for &index in &indexes {
            positions.push(page[index].position);
        }
        match rows::materialize(mapped.bytes(), &positions, &spec.columns) {
            Ok(values) => {
                for (index, value) in indexes.into_iter().zip(values) {
                    fields[index] = Some(value);
                }
            }
            Err(e) => {
                own.add(PartialReason::SourceFailure);
                own.of(PartialReason::SourceFailure, candidates);
                tracing::warn!(
                    "sfsq traces: source {} failed to read row fields: {e}",
                    trace_source.source_id()
                );
            }
        }
    }
    let mut items = Vec::with_capacity(page.len());
    for (candidate, fields) in page.into_iter().zip(fields) {
        let Some(fields) = fields else { continue };
        items.push(Row {
            key: candidate.key,
            duration_ns: candidate.duration_ns,
            service: fields.service,
            name: fields.name,
            role: fields.role,
            status: fields.status,
            columns: fields.columns,
        });
    }
    (items, more, own)
}
