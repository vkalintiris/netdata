//! Orchestration: prepare every source once, evaluate the readable ones,
//! count the rest, and assemble the sections.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

use super::super::duration_hist::DurationHistogram;
use super::query::{ExploreQuery, ExploreRequestError, HIDDEN_FIELDS};
use super::rows::{self, MoreRows, PageFold, ROW_VALUE_COLUMNS, RowFields};
use super::shard::{self, Evaluated, evaluate};
use super::source::{Prepared, SourceTally, prepare};
use super::{
    ExploreData, FacetData, FacetValue, FacetsData, FieldInfo, FieldsData, HistogramData,
    Percentiles, Row, RowsData, StackBucket, Totals,
};
use crate::merge::{MergedFacet, merge_facets, merge_field_tables, merge_timelines};
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

    let mut tally = SourceTally::default();
    let mut stack_high = 0u64;
    let mut matched = 0u64;
    let mut errors = 0u64;
    let mut timelines = Vec::new();
    let mut other = vec![0u64; buckets];
    let mut durations = vec![DurationHistogram::new(); buckets];
    let mut facets = Vec::new();
    let mut facet_high = BTreeSet::new();
    let mut page = query.sections.rows.as_ref().map(PageFold::new);
    let mut field_tables = Vec::new();
    // With rows asked for, every evaluated source stays open: to read the page's
    // fields, and to select the page again without a source whose fields fail.
    // A candidate's `source` indexes this list.
    let mut opened: Vec<(&TraceSource, Mapped)> = Vec::new();

    for source in &sources {
        if cancel.is_cancelled() {
            return Ok(ExploreData::cancelled());
        }
        match prepare(source, &window) {
            Prepared::Outside => {}
            Prepared::Unavailable => {
                tally.candidates += 1;
                tally.unavailable += 1;
            }
            Prepared::Failed(error) => {
                tally.candidates += 1;
                tally.failed += 1;
                tracing::warn!("sfsq traces: source {} failed: {error}", source.source_id());
            }
            Prepared::Open(mapped) => {
                tally.candidates += 1;
                let stop = page.as_ref().and_then(PageFold::stop);
                match evaluate(mapped.bytes(), &query, opened.len(), stop) {
                    Ok(Evaluated::Legacy) => tally.legacy += 1,
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
                        field_tables.extend(shard.field_table);
                        if let (Some(rows), Some(page)) = (shard.rows, page.as_mut()) {
                            opened.push((source, mapped));
                            page.add(rows);
                        }
                    }
                    Err(e) => {
                        tally.failed += 1;
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
    let shared = tally.status();
    let candidates = tally.candidates;
    let mut status = shared.clone();

    let histogram = query.sections.histogram.as_ref().map(|spec| {
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
            stack: spec.stack.clone(),
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

    let facets = query.sections.facets.as_ref().map(|spec| {
        let mut own = StatusBuilder::new();
        let mut merged: BTreeMap<String, MergedFacet> = merge_facets(facets)
            .into_iter()
            .map(|facet| (facet.field.clone(), facet))
            .collect();
        let mut fields = Vec::new();
        let mut unavailable = Vec::new();
        match &spec.fields {
            Some(requested) => {
                for field in requested {
                    if facet_high.contains(field) {
                        own.add(PartialReason::FacetHighCard);
                        own.detail(PartialReason::FacetHighCard, field.clone());
                        unavailable.push((field.clone(), PartialReason::FacetHighCard));
                    } else {
                        let facet = merged.remove(field).unwrap_or(MergedFacet {
                            field: field.clone(),
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

    let rows = page.map(|page| {
        let spec = page.spec();
        let (items, more, own) = rows_section(page, &opened, &query, candidates);
        status.merge(own.clone());
        let mut section = shared.clone();
        section.merge(own);
        RowsData {
            status: section.finish(),
            order: spec.order,
            matched,
            more,
            columns: spec.columns.clone(),
            items,
        }
    });

    let fields = query.sections.fields.then(|| {
        let mut items = Vec::new();
        for entry in merge_field_tables(&field_tables).iter() {
            if !HIDDEN_FIELDS.contains(&entry.name.as_str()) {
                items.push(FieldInfo::of(entry.clone()));
            }
        }
        FieldsData {
            status: shared.clone().finish(),
            items,
            columns: ROW_VALUE_COLUMNS.to_vec(),
        }
    });

    Ok(ExploreData {
        status: status.finish(),
        sources: candidates,
        histogram,
        facets,
        rows,
        fields,
    })
}

/// The page with its rows' fields, and this section's own reasons. A source
/// whose fields cannot be read is counted as failed and the page is selected
/// again without it, so the page stays contiguous and its cursor valid; the
/// source's rows still count in the other sections and in `matched`.
fn rows_section(
    fold: PageFold<'_>,
    opened: &[(&TraceSource, Mapped)],
    query: &ExploreQuery,
    candidates: u64,
) -> (Vec<Row>, Option<MoreRows>, StatusBuilder) {
    let spec = fold.spec();
    let mut own = StatusBuilder::new();
    let fail = |own: &mut StatusBuilder, source: &TraceSource, error: &sfst::Error| {
        own.add(PartialReason::SourceFailure);
        own.of(PartialReason::SourceFailure, candidates);
        tracing::warn!(
            "sfsq traces: source {} failed to read rows: {error}",
            source.source_id()
        );
    };
    let mut excluded = BTreeSet::new();
    let mut fold = fold;
    loop {
        let (page, more) = fold.finish();
        let (source, error) = match read_fields(&page, opened, &spec.columns) {
            Ok(fields) => {
                let mut items = Vec::with_capacity(page.len());
                for (candidate, fields) in page.into_iter().zip(fields) {
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
                return (items, more, own);
            }
            Err(failure) => failure,
        };
        fail(&mut own, opened[source].0, &error);
        excluded.insert(source);
        fold = PageFold::new(spec);
        for (slot, (trace_source, mapped)) in opened.iter().enumerate() {
            if excluded.contains(&slot) {
                continue;
            }
            match shard::rows_of(mapped.bytes(), query, spec, slot, fold.stop()) {
                Ok(rows) => fold.add(rows),
                Err(e) => {
                    fail(&mut own, trace_source, &e);
                    excluded.insert(slot);
                }
            }
        }
    }
}

/// The fields of the page's rows, in page order, each source read once; the
/// first source that fails, with its error.
fn read_fields(
    page: &[rows::Candidate],
    opened: &[(&TraceSource, Mapped)],
    columns: &[String],
) -> Result<Vec<RowFields>, (usize, sfst::Error)> {
    let mut by_source: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, candidate) in page.iter().enumerate() {
        by_source.entry(candidate.source).or_default().push(index);
    }
    let mut fields: Vec<RowFields> = vec![RowFields::default(); page.len()];
    for (source, indexes) in by_source {
        let mut positions = Vec::with_capacity(indexes.len());
        for &index in &indexes {
            positions.push(page[index].position);
        }
        let values = rows::materialize(opened[source].1.bytes(), &positions, columns)
            .map_err(|e| (source, e))?;
        for (index, value) in indexes.into_iter().zip(values) {
            fields[index] = value;
        }
    }
    Ok(fields)
}
