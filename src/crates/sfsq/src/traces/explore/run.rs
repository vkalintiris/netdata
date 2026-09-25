//! Orchestration: prepare every source once, evaluate the readable ones,
//! count the rest, and assemble the sections.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use tokio_util::sync::CancellationToken;

use super::super::duration_hist::DurationHistogram;
use super::query::{ExploreQuery, ExploreRequestError, HIDDEN_FIELDS};
use super::rows::{self, MoreRows, PageFold, ROW_VALUE_COLUMNS, RowFields};
use super::shard::{self, Evaluated, ExploreShard, evaluate};
use super::source::{ExploreOptions, SourceTally, evaluate_sources};
use super::{
    ExploreData, FacetData, FacetValue, FacetsData, FieldInfo, FieldsData, HistogramData,
    Percentiles, Row, RowsData, StackBucket, Totals,
};
use crate::merge::{MergedFacet, merge_facets, merge_field_tables, merge_timelines};
use crate::source::Mapped;
use crate::traces::{PartialReason, StatusBuilder, TimeWindow, TraceSource, validate_sources};

/// What one worker has folded from the sources it evaluated.
struct Lane<'q> {
    matched: u64,
    errors: u64,
    stack_high: u64,
    timelines: Vec<sfst::Timeline>,
    other: Vec<u64>,
    durations: Vec<DurationHistogram>,
    facets: Vec<Vec<sfst::FacetResult>>,
    facet_high: BTreeSet<String>,
    field_tables: Vec<sfst::FieldTable>,
    page: Option<PageFold<'q>>,
    /// With rows asked for, every evaluated source stays open: to read the
    /// page's fields, and to select the page again without a source whose
    /// fields fail. By index in the request's sources.
    opened: Vec<(usize, Mapped)>,
}

impl<'q> Lane<'q> {
    fn new(query: &'q ExploreQuery) -> Self {
        let buckets = query.grid.num_buckets;
        Lane {
            matched: 0,
            errors: 0,
            stack_high: 0,
            timelines: Vec::new(),
            other: vec![0; buckets],
            durations: vec![DurationHistogram::new(); buckets],
            facets: Vec::new(),
            facet_high: BTreeSet::new(),
            field_tables: Vec::new(),
            page: query.sections.rows.as_ref().map(PageFold::new),
            opened: Vec::new(),
        }
    }

    fn add(&mut self, shard: ExploreShard, source: usize, mapped: Mapped) {
        self.matched += shard.matched;
        self.errors += shard.errors;
        if let Some(timeline) = shard.timeline {
            self.timelines.push(timeline);
        }
        for (sum, histogram) in self.durations.iter_mut().zip(&shard.durations) {
            sum.merge(histogram);
        }
        self.facets.push(shard.facets);
        self.facet_high.extend(shard.facet_high);
        if shard.stack_high {
            self.stack_high += 1;
            for (sum, n) in self.other.iter_mut().zip(&shard.other) {
                *sum += n;
            }
        }
        self.field_tables.extend(shard.field_table);
        if let (Some(rows), Some(page)) = (shard.rows, self.page.as_mut()) {
            self.opened.push((source, mapped));
            page.add(rows);
        }
    }

    fn merge(&mut self, other: Lane<'q>) {
        self.matched += other.matched;
        self.errors += other.errors;
        self.stack_high += other.stack_high;
        self.timelines.extend(other.timelines);
        for (sum, n) in self.other.iter_mut().zip(&other.other) {
            *sum += n;
        }
        for (sum, histogram) in self.durations.iter_mut().zip(&other.durations) {
            sum.merge(histogram);
        }
        self.facets.extend(other.facets);
        self.facet_high.extend(other.facet_high);
        self.field_tables.extend(other.field_tables);
        if let (Some(page), Some(theirs)) = (self.page.as_mut(), other.page) {
            page.merge(theirs);
        }
        self.opened.extend(other.opened);
    }
}

/// Answer an explorer request over `sources`.
///
/// Pure sync: reads and decompresses files and builds the live tail's image,
/// over up to `options.workers` threads; run it off any async runtime thread.
/// `progress` ticks once per source. Cancellation is all-or-empty: a
/// cancelled call returns no sections and the `cancelled` reason.
pub fn explore(
    sources: Vec<TraceSource>,
    query: ExploreQuery,
    options: ExploreOptions,
    cancel: CancellationToken,
    progress: Arc<AtomicUsize>,
) -> Result<ExploreData, ExploreRequestError> {
    validate_sources(&sources)?;
    query.validate()?;
    let window_ns = query.grid.range_ns();
    let window = TimeWindow::new(window_ns.start, window_ns.end)?;
    let buckets = query.grid.num_buckets;

    let Some(lanes) = evaluate_sources(
        &sources,
        &window,
        options.workers,
        &cancel,
        &progress,
        || Lane::new(&query),
        |lane, tally, index, mapped| {
            let stop = lane.page.as_ref().and_then(PageFold::stop);
            match evaluate(mapped.bytes(), &query, index, stop) {
                Ok(Evaluated::Legacy) => tally.legacy += 1,
                Ok(Evaluated::Shard(shard)) => lane.add(*shard, index, mapped),
                Err(e) => {
                    tally.failed += 1;
                    tracing::warn!(
                        "sfsq traces: source {} failed to evaluate: {e}",
                        sources[index].source_id()
                    );
                }
            }
        },
    ) else {
        return Ok(ExploreData::cancelled());
    };
    let mut tally = SourceTally::default();
    let mut folded = Lane::new(&query);
    for (lane_tally, lane) in lanes {
        tally.add(&lane_tally);
        folded.merge(lane);
    }
    let Lane {
        matched,
        errors,
        stack_high,
        timelines,
        other,
        durations,
        facets,
        facet_high,
        field_tables,
        page,
        opened: opened_lanes,
    } = folded;
    let mut opened: Vec<Option<Mapped>> = vec![None; sources.len()];
    for (index, mapped) in opened_lanes {
        opened[index] = Some(mapped);
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
        let (items, more, own) = rows_section(page, &sources, &opened, &query, candidates);
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
    sources: &[TraceSource],
    opened: &[Option<Mapped>],
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
        fail(&mut own, &sources[source], &error);
        excluded.insert(source);
        fold = PageFold::new(spec);
        for (index, mapped) in opened.iter().enumerate() {
            let Some(mapped) = mapped else {
                continue;
            };
            if excluded.contains(&index) {
                continue;
            }
            match shard::rows_of(mapped.bytes(), query, spec, index, fold.stop()) {
                Ok(rows) => fold.add(rows),
                Err(e) => {
                    fail(&mut own, &sources[index], &e);
                    excluded.insert(index);
                }
            }
        }
    }
}

/// The fields of the page's rows, in page order, each source read once; the
/// first source that fails, with its error.
fn read_fields(
    page: &[rows::Candidate],
    opened: &[Option<Mapped>],
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
        let mapped = opened[source]
            .as_ref()
            .expect("a page row's source stays open");
        let values =
            rows::materialize(mapped.bytes(), &positions, columns).map_err(|e| (source, e))?;
        for (index, value) in indexes.into_iter().zip(values) {
            fields[index] = value;
        }
    }
    Ok(fields)
}
