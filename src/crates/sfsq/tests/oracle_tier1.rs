//! Tier-1 comparison with the reference calculator (`otel-oracle`): a seeded
//! multi-service corpus goes through the plugin's own ingest in-process (flatten
//! → WAL frames → a sealed file, and an unsealed WAL read as a chunk image and
//! as a tail), and what the store answers is compared with the calculator's
//! brute-force numbers over the same spans.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValueList, any_value};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::span::{Event, Link};
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span, Status};
use otel_oracle::assembly::{self, SpanContent, TraceView};
use otel_oracle::calc::{self, Grid, Scope, fixed_histogram};
use otel_oracle::corpus::{self, MeshParams};
use otel_oracle::model::{self, OracleSpan};
use sfsq::Source;
use sfsq::traces::explore::{
    self, ExploreQuery, ExploreScope, ExploreSelection, FacetSpec, HIDDEN_FIELDS, HistogramSpec,
    RowDirection, RowKey, RowOrder, RowsData, RowsSpec, Sections,
};
use sfsq::traces::{
    PartialReason, QueryStatus, SourceId, TraceSfstCandidate, TraceSource, TraceWalScan,
    WalCoverage,
};

const T0_S: u64 = 1_700_000_000;
const TRACE_SPACING_NS: u64 = 700_000_000;

/// Which stored unit a span landed in.
const SEALED: usize = 0;
const LIVE: usize = 1;

/// Fields every comparison looks at: the explorer's core fields.
const CORE_FIELDS: [&str; 7] = [
    model::ROLE_FIELD,
    model::DURATION_BAND_FIELD,
    model::STATUS_FIELD,
    model::SERVICE_FIELD,
    "name",
    "kind",
    "_kind",
];

struct Stored {
    _dir: tempfile::TempDir,
    sealed: Vec<u8>,
    live_wal: PathBuf,
    live_chunk: Vec<u8>,
    /// What the explorer shows: `_err_origin` on the origins of the sealed
    /// file and, through the live pass, of the live WAL as a whole.
    oracle: Vec<OracleSpan>,
    /// What the files store: `_err_origin` on the sealed file's origins only.
    files: Vec<OracleSpan>,
    grid: Grid,
}

/// The first two thirds of the export requests seal into a file; the rest stay
/// in a WAL that is read both as a chunk image and as a tail.
fn store(traces: usize, seed: u64) -> Stored {
    store_resending(traces, seed, None)
}

/// [`store`], with every `resend_every`-th export request sent twice in a row,
/// as an exporter retrying after a lost acknowledgement would: its rows are
/// stored twice with identical content. The cut then falls between the two
/// copies of one resent request, so one identical group spans both units.
fn store_resending(traces: usize, seed: u64, resend_every: Option<usize>) -> Stored {
    store_shaped(traces, seed, resend_every, None)
}

/// [`store_resending`], with span start times truncated to multiples of
/// `clock_ns` (durations kept), as a coarse exporter clock would: many spans
/// then share a start with distinct ids.
fn store_shaped(
    traces: usize,
    seed: u64,
    resend_every: Option<usize>,
    clock_ns: Option<u64>,
) -> Stored {
    let spans = corpus::generate(&MeshParams {
        traces,
        start_ns: T0_S * 1_000_000_000,
        trace_spacing_ns: TRACE_SPACING_NS,
        seed,
    });
    let mut requests = Vec::new();
    let mut resent = Vec::new();
    for (i, mut request) in corpus::build_requests(&spans, 50).into_iter().enumerate() {
        if let Some(clock) = clock_ns {
            for resource in &mut request.resource_spans {
                for scope in &mut resource.scope_spans {
                    for span in &mut scope.spans {
                        let shift = span.start_time_unix_nano % clock;
                        span.start_time_unix_nano -= shift;
                        span.end_time_unix_nano = span.end_time_unix_nano.saturating_sub(shift);
                    }
                }
            }
        }
        if resend_every.is_some_and(|every| i % every == 0) {
            resent.push(requests.len());
            requests.push(request.clone());
        }
        requests.push(request);
    }
    // The resent pair nearest two thirds that leaves the live unit two frames.
    let two_thirds = requests.len() * 2 / 3;
    let mut straddle: Option<usize> = None;
    for &first in &resent {
        let closer =
            straddle.is_none_or(|best| first.abs_diff(two_thirds) < best.abs_diff(two_thirds));
        if first + 3 <= requests.len() && closer {
            straddle = Some(first);
        }
    }
    let cut = straddle.map_or(two_thirds, |first| first + 1);
    store_requests(&requests, cut)
}

/// The requests before `cut` sealed into a file, the rest in the live WAL.
fn store_requests(requests: &[ExportTraceServiceRequest], cut: usize) -> Stored {
    let dir = tempfile::tempdir().unwrap();
    let mut oracle = Vec::new();
    for (i, request) in requests.iter().enumerate() {
        oracle.extend(model::spans_of_request(
            request,
            if i < cut { SEALED } else { LIVE },
        ));
    }

    let sealed_wal = common::write_wal(dir.path(), requests[..cut].to_vec(), "sealed");
    let sealed_path = dir.path().join("sealed.sfst");
    ng_index::build_sfst_traces_file(&sealed_wal, &sealed_path, &ng_index::Metrics::new()).unwrap();
    let live_wal = common::write_wal(dir.path(), requests[cut..].to_vec(), "live");
    let (_, live_chunk) =
        ng_index::build_sfst_traces_range(&live_wal, common::whole_range(&live_wal)).unwrap();

    // The sealed file stores the seal's error-origin tokens; the live chunk
    // does not, and the live pass derives them over the whole live WAL.
    let mut files = oracle.clone();
    calc::add_stored_derived(&mut files, &|unit| unit == SEALED);
    calc::add_derived(&mut oracle, &|unit| Some(unit));
    let last_start_s = oracle.iter().map(|s| s.start_ns).max().unwrap() / 1_000_000_000;
    let grid = Grid::for_window(T0_S as u32, last_start_s as u32 + 1);

    Stored {
        _dir: dir,
        sealed: std::fs::read(&sealed_path).unwrap(),
        live_wal,
        live_chunk,
        oracle,
        files,
        grid,
    }
}

/// A unit's spans as its file stores them.
fn unit_spans(stored: &Stored, unit: usize) -> Vec<&OracleSpan> {
    stored.files.iter().filter(|s| s.unit == unit).collect()
}

/// value → rows carrying it, per field, as the calculator sees a unit.
fn oracle_value_counts(spans: &[&OracleSpan], field: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for span in spans {
        if let Some(values) = span.fields.get(field) {
            for value in values {
                *counts.entry(value.to_string()).or_default() += 1;
            }
        }
    }
    counts
}

fn stored_value_counts(bytes: &[u8], field: &str) -> BTreeMap<String, u64> {
    stored_value_counts_in(bytes, &sfst::Filter::new(), field)
}

/// value → rows carrying it among the rows `filter` selects.
fn stored_value_counts_in(
    bytes: &[u8],
    filter: &sfst::Filter,
    field: &str,
) -> BTreeMap<String, u64> {
    let reader = sfst::IndexReader::open(bytes).unwrap();
    let all = reader.compile_filter(filter, None).unwrap();
    let facets = reader.facets(&[field], &all, i64::MIN..i64::MAX).unwrap();
    let mut counts = BTreeMap::new();
    for facet in facets {
        for (value, count) in facet.values {
            counts.insert(value, u64::from(count));
        }
    }
    counts
}

/// ORC-TOKENS: every unit stores exactly the calculator's rows and fields and,
/// per field, the same number of rows per value (the same values for a
/// high-cardinality field, which has no counts); `_role` is on every row
/// (FLAT-15). Events and links fields included.
#[test]
fn stored_tokens_match_the_calculator() {
    for seed in [11, 12] {
        let stored = store(300, seed);
        for (unit, bytes) in [(SEALED, &stored.sealed), (LIVE, &stored.live_chunk)] {
            let spans = unit_spans(&stored, unit);
            let reader = sfst::IndexReader::open(bytes).unwrap();
            assert_eq!(
                reader.summary().record_count as usize,
                spans.len(),
                "seed {seed} unit {unit}"
            );
            assert!(reader.field_table().get(model::ROLE_FIELD).is_some());
            let stored_fields: BTreeSet<&str> = reader.field_table().names().collect();
            let mut oracle_fields = BTreeSet::new();
            for span in &spans {
                oracle_fields.extend(span.fields.keys());
            }
            assert_eq!(stored_fields, oracle_fields, "seed {seed} unit {unit}");
            assert!(stored_fields.contains("events.attributes.exception.type"));
            assert!(stored_fields.contains("links.attributes.link.reason"));
            for entry in reader.field_table().iter() {
                let field = entry.name.as_str();
                let want = oracle_value_counts(&spans, field);
                let case = format!("seed {seed} unit {unit} field {field}");
                if entry.is_high_card() {
                    let got: BTreeSet<String> =
                        reader.field_values(field).unwrap().into_iter().collect();
                    assert_eq!(got, want.into_keys().collect(), "{case}");
                } else {
                    assert_eq!(stored_value_counts(bytes, field), want, "{case}");
                }
            }
            let roles: u64 = stored_value_counts(bytes, model::ROLE_FIELD).values().sum();
            assert_eq!(roles as usize, spans.len(), "one role per row");

            // Per (service.name, name) group, the role and band rows.
            let owned: Vec<OracleSpan> = spans.iter().map(|span| (*span).clone()).collect();
            let want = calc::token_counts(&owned).remove(&unit).unwrap_or_default();
            let mut got = BTreeMap::new();
            let mut groups = BTreeSet::new();
            for key in want.keys() {
                if let Some(group) = &key.group {
                    groups.insert(group.clone());
                }
            }
            assert!(groups.len() > 5, "seed {seed} unit {unit}: several groups");
            for group in groups {
                let mut filter = sfst::Filter::new();
                for (field, value) in [
                    (model::SERVICE_FIELD, &group.service),
                    ("name", &group.operation),
                ] {
                    filter = match value {
                        Some(value) => filter.select(field, value),
                        None => filter.select_absent(field),
                    };
                }
                for field in [model::ROLE_FIELD, model::DURATION_BAND_FIELD] {
                    for (value, count) in stored_value_counts_in(bytes, &filter, field) {
                        let key = calc::TokenKey {
                            group: Some(group.clone()),
                            field,
                            value,
                        };
                        got.insert(key, count);
                    }
                }
            }
            let mut want_groups = BTreeMap::new();
            for (key, count) in want {
                if key.group.is_some() {
                    want_groups.insert(key, count);
                }
            }
            assert_eq!(
                got, want_groups,
                "seed {seed} unit {unit}: tokens per group"
            );
        }
    }
}

/// ORC-TOKENS across sources: the live WAL read as a tail yields, per span, the
/// same core-field values as the calculator (so tail, chunk image and sealed
/// file agree, FLAT-13).
#[test]
fn tail_spans_carry_the_calculator_tokens() {
    let stored = store(240, 21);
    let scan =
        TraceWalScan::scan_range(&stored.live_wal, common::whole_range(&stored.live_wal)).unwrap();

    type Key = ([u8; 16], [u8; 8]);
    let mut expected: BTreeMap<Key, Vec<BTreeMap<String, BTreeSet<String>>>> = BTreeMap::new();
    for span in unit_spans(&stored, LIVE) {
        let key = (span.trace_id.unwrap(), span.span_id.unwrap());
        let mut core = BTreeMap::new();
        for field in CORE_FIELDS {
            if let Some(values) = span.fields.get(field) {
                core.insert(field.to_string(), values.to_set());
            }
        }
        expected.entry(key).or_default().push(core);
    }

    let mut got: BTreeMap<Key, Vec<BTreeMap<String, BTreeSet<String>>>> = BTreeMap::new();
    for (trace_id, span) in scan.spans_with_ids() {
        let key = (*trace_id.as_bytes(), *span.span_id.as_bytes());
        let mut core: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (field, value) in &span.fields {
            if CORE_FIELDS.contains(&field.as_str()) {
                core.entry(field.clone()).or_default().insert(value.clone());
            }
        }
        got.entry(key).or_default().push(core);
    }
    assert_eq!(got, expected);
}

type SpanKey = ([u8; 16], [u8; 8]);
type CoreTokens = BTreeMap<String, BTreeSet<String>>;

fn core_tokens<'a>(fields: impl Iterator<Item = (&'a str, &'a str)>) -> CoreTokens {
    let mut core = CoreTokens::new();
    for (field, value) in fields {
        if CORE_FIELDS.contains(&field) {
            core.entry(field.to_string())
                .or_default()
                .insert(value.to_string());
        }
    }
    core
}

/// Each row of an index file: its span and core tokens.
fn file_core_tokens(bytes: &[u8], out: &mut BTreeMap<SpanKey, Vec<CoreTokens>>) {
    let reader = sfst::IndexReader::open(bytes).unwrap();
    let positions: Vec<u32> = (0..reader.summary().record_count).collect();
    let rows = reader.materialize_rows(&positions).unwrap();
    let traces = reader.trace_ids().unwrap();
    let spans = reader.span_ids().unwrap();
    for (&position, row) in positions.iter().zip(&rows) {
        let key = (
            *traces.get(position as usize).as_bytes(),
            *spans.get(position as usize).as_bytes(),
        );
        let fields = row.fields.iter().map(|(f, v)| (f.as_str(), v.as_str()));
        out.entry(key).or_default().push(core_tokens(fields));
    }
}

/// FLAT-13: the sealed file, and the live WAL cut into chunk images at several
/// split points with the rest as the tail, store for every span the same core
/// tokens (`_role` and `_duration_band` included) as the calculator.
#[test]
fn every_layout_stores_each_span_s_core_tokens() {
    let stored = store(240, 21);
    let mut expected: BTreeMap<SpanKey, Vec<CoreTokens>> = BTreeMap::new();
    for span in &stored.oracle {
        let key = (span.trace_id.unwrap(), span.span_id.unwrap());
        let mut fields = Vec::new();
        for (field, values) in span.fields.iter() {
            for value in values {
                fields.push((field, value));
            }
        }
        expected
            .entry(key)
            .or_default()
            .push(core_tokens(fields.into_iter()));
    }
    for copies in expected.values_mut() {
        copies.sort();
    }

    let header = wal::HEADER_SIZE as u64;
    let whole = common::whole_range(&stored.live_wal);
    let frames = wal::scan_frame_boundaries(&stored.live_wal, whole).unwrap();
    let mut layouts = BTreeSet::new();
    for min_entries in [1, 13, 60, u64::MAX] {
        let mut got: BTreeMap<SpanKey, Vec<CoreTokens>> = BTreeMap::new();
        file_core_tokens(&stored.sealed, &mut got);
        let chunks = wal::prefix::chunk_boundaries(&frames, header, min_entries);
        for chunk in &chunks {
            let (_, bytes) =
                ng_index::build_sfst_traces_range(&stored.live_wal, chunk.range).unwrap();
            file_core_tokens(&bytes, &mut got);
        }
        let tail = wal::prefix::tail_start(&chunks, header);
        if tail < whole.end() {
            let scan =
                TraceWalScan::scan_range(&stored.live_wal, wal::FrameRange::new(tail, whole.end()))
                    .unwrap();
            for (trace_id, span) in scan.spans_with_ids() {
                let key = (*trace_id.as_bytes(), *span.span_id.as_bytes());
                let fields = span.fields.iter().map(|(f, v)| (f.as_str(), v.as_str()));
                got.entry(key).or_default().push(core_tokens(fields));
            }
        }
        for copies in got.values_mut() {
            copies.sort();
        }
        layouts.insert((chunks.len(), tail < whole.end()));
        assert_eq!(got, expected, "chunks of at least {min_entries} spans");
    }
    assert!(layouts.len() >= 3, "the splits differ: {layouts:?}");
}

/// ORC-HIST and ORC-TOTALS on the file statistics the explorer builds on: the
/// entry-span histogram stacked by status, summed over the sealed file and the
/// live chunk image, equals the calculator's.
#[test]
fn entry_span_histogram_by_status_matches_the_calculator() {
    let stored = store(400, 31);
    let grid = stored.grid;
    let sfst_grid = sfst::Grid::new(
        i64::from(grid.after_s) * 1_000_000_000,
        i64::from(grid.width_s) * 1_000_000_000,
        grid.buckets(),
    );
    let filter = sfst::Filter::new()
        .select(model::ROLE_FIELD, "root")
        .select(model::ROLE_FIELD, "inbound");

    let mut got = vec![calc::Bucket::default(); grid.buckets()];
    for bytes in [&stored.sealed, &stored.live_chunk] {
        let reader = sfst::IndexReader::open(bytes).unwrap();
        let scope = reader.compile_filter(&filter, None).unwrap();
        let timeline = reader
            .timeline(model::STATUS_FIELD, &scope, sfst_grid)
            .unwrap();
        for (index, bucket) in timeline.buckets.iter().enumerate() {
            for (dimension, count) in timeline.dimensions.iter().zip(&bucket.counts) {
                if *count > 0 {
                    *got[index].counts.entry(dimension.clone()).or_default() += count;
                }
            }
            got[index].unset += bucket.unset;
        }
    }

    let want = calc::histogram(
        &stored.oracle,
        &grid,
        &Scope::entry_spans(),
        model::STATUS_FIELD,
    );
    assert_eq!(got, want);

    let totals = calc::totals(&stored.oracle, &grid, &Scope::entry_spans());
    let spans: u64 = got
        .iter()
        .map(|b| b.counts.values().sum::<u64>() + b.unset)
        .sum();
    let errors: u64 = got
        .iter()
        .map(|b| b.counts.get("error").copied().unwrap_or(0))
        .sum();
    assert_eq!((spans, errors), (totals.spans, totals.errors));
    assert!(
        totals.errors > 0 && totals.spans > errors,
        "the corpus exercises both"
    );
}

/// The corpus requests the comparisons are built from stay representative: all
/// four roles and all six bands occur.
#[test]
fn corpus_reaches_every_role_and_band() {
    let stored = store(300, 41);
    let all: Vec<&OracleSpan> = stored.oracle.iter().collect();
    let roles: BTreeSet<String> = oracle_value_counts(&all, model::ROLE_FIELD)
        .into_keys()
        .collect();
    let bands: BTreeSet<String> = oracle_value_counts(&all, model::DURATION_BAND_FIELD)
        .into_keys()
        .collect();
    assert_eq!(roles.len(), 4, "{roles:?}");
    assert_eq!(bands.len(), 6, "{bands:?}");
}

/// How the live WAL is served to the engine.
#[derive(Clone, Copy, Debug)]
enum Live {
    Tail,
    Chunk,
    /// Chunks of at least this many spans, the rest as the tail.
    Split(u64),
    /// Chunks and a tail, whatever the corpus: the chunk size is the first
    /// frames' spans, the largest such size that still leaves a tail.
    Chunked,
}

/// The sealed file plus the live WAL served as `live`.
fn explore_sources(stored: &Stored, live: Live) -> Vec<TraceSource> {
    let sealed_path = stored._dir.path().join("sealed-copy.sfst");
    std::fs::write(&sealed_path, &stored.sealed).unwrap();
    let mut sources = vec![common::sealed_source_at(&sealed_path, "sealed")];
    let whole = common::whole_range(&stored.live_wal);
    match live {
        Live::Tail => sources.push(common::tail_source(&stored.live_wal, "live")),
        Live::Chunk => sources.push(common::memory_source(&stored.live_wal, "live")),
        Live::Split(_) | Live::Chunked => {
            let header = wal::HEADER_SIZE as u64;
            let frames = wal::scan_frame_boundaries(&stored.live_wal, whole).unwrap();
            let min_entries = match live {
                Live::Split(min_entries) => min_entries,
                _ => {
                    let mut sizes = Vec::new();
                    let mut sum = 0;
                    for frame in &frames {
                        sum += u64::from(frame.entry_count);
                        sizes.push(sum);
                    }
                    let leaves_a_tail = |size: u64| {
                        let chunks = wal::prefix::chunk_boundaries(&frames, header, size);
                        !chunks.is_empty() && wal::prefix::tail_start(&chunks, header) < whole.end()
                    };
                    sizes
                        .into_iter()
                        .rev()
                        .find(|&size| leaves_a_tail(size))
                        .expect("the live WAL holds at least two frames")
                }
            };
            let chunks = wal::prefix::chunk_boundaries(&frames, header, min_entries);
            assert!(!chunks.is_empty(), "the split makes at least one chunk");
            let wal_id: std::sync::Arc<str> = stored.live_wal.display().to_string().into();
            for chunk in &chunks {
                let (summary, bytes) =
                    ng_index::build_sfst_traces_range(&stored.live_wal, chunk.range).unwrap();
                sources.push(TraceSource::Sfst(TraceSfstCandidate {
                    source_id: SourceId::new(format!("live#chunk{}", chunk.index)),
                    summary,
                    source: Source::Memory(std::sync::Arc::new(bytes)),
                    coverage: Some(WalCoverage {
                        wal_id: wal_id.clone(),
                        range: chunk.range,
                    }),
                }));
            }
            let tail = wal::prefix::tail_start(&chunks, header);
            assert!(tail < whole.end(), "the split leaves a tail");
            sources.push(TraceSource::Tail(sfsq::traces::TraceWalTail {
                source_id: SourceId::new("live#tail".to_string()),
                path: stored.live_wal.clone(),
                coverage: WalCoverage {
                    wal_id,
                    range: wal::FrameRange::new(tail, whole.end()),
                },
            }));
        }
    }
    sources
}

fn explore_query(grid: &Grid, scope: &Scope, stack: &str) -> ExploreQuery {
    let mut filter = sfst::Filter::new();
    for (field, wanted) in &scope.terms {
        for value in &wanted.values {
            filter = filter.select(field.clone(), value.clone());
        }
        if wanted.absent {
            filter = filter.select_absent(field.clone());
        }
    }
    ExploreQuery {
        grid: sfst::Grid::new(
            i64::from(grid.after_s) * 1_000_000_000,
            i64::from(grid.width_s) * 1_000_000_000,
            grid.buckets(),
        ),
        scope: ExploreScope {
            filter,
            text: scope.text.as_deref().map(sfst::text::LiteralText::new),
            trace_ids: scope
                .trace_ids
                .iter()
                .map(|id| sfst::TraceId::from(*id))
                .collect(),
            duration: None,
        },
        selection: None,
        sections: Sections {
            histogram: Some(HistogramSpec {
                stack: stack.to_string(),
                percentiles: true,
                durations: false,
            }),
            facets: None,
            groups: false,
            rows: None,
            fields: false,
        },
    }
}

/// ORC-FILTER (F0, F1), ORC-HIST and ORC-TOTALS through the explorer engine:
/// for every way of serving the live WAL, every stack field and both scopes,
/// the stacked buckets and the totals equal the calculator's.
#[test]
fn explore_histogram_and_totals_match_the_calculator() {
    let stored = store(400, 51);
    let grid = stored.grid;
    let scopes = [
        ("F0 every span", Scope::default()),
        ("F1 entry spans", Scope::entry_spans()),
    ];
    let stacks = [
        model::STATUS_FIELD,
        model::DURATION_BAND_FIELD,
        model::SERVICE_FIELD,
    ];
    for live in [Live::Tail, Live::Chunk, Live::Split(100)] {
        for (scope_name, scope) in &scopes {
            for stack in stacks {
                let data = explore::explore(
                    explore_sources(&stored, live),
                    explore_query(&grid, scope, stack),
                    explore::ExploreOptions::default(),
                    tokio_util::sync::CancellationToken::new(),
                    std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                )
                .unwrap();
                let case = format!("{live:?} {scope_name} stack {stack}");
                assert!(data.status.is_complete(), "{case}: {:?}", data.status);
                let histogram = data.histogram.expect("histogram section");

                assert_eq!(
                    calc_buckets(&histogram),
                    calc::histogram(&stored.oracle, &grid, scope, stack),
                    "{case}"
                );

                // ORC-PCT: the fixed histogram's percentiles, per bucket and for the window,
                // equal the calculator's own, and each is within the documented bound of
                // the exact nearest-rank value.
                let per_bucket = calc::bucket_durations(&stored.oracle, &grid, scope);
                let mut window_durations = Vec::new();
                for (bucket, durations) in histogram.buckets.iter().zip(&per_bucket) {
                    let got = bucket.percentiles.map(|p| [p.p50_ns, p.p95_ns, p.p99_ns]);
                    assert_eq!(got, fixed_histogram::percentiles(durations), "{case}");
                    assert!(
                        fixed_histogram::within_bound(
                            got,
                            fixed_histogram::exact_percentiles(durations)
                        ),
                        "{case}: {got:?}"
                    );
                    window_durations.extend_from_slice(durations);
                }
                let window = histogram
                    .totals
                    .percentiles
                    .map(|p| [p.p50_ns, p.p95_ns, p.p99_ns]);
                assert_eq!(
                    window,
                    fixed_histogram::percentiles(&window_durations),
                    "{case}"
                );
                assert!(
                    fixed_histogram::within_bound(
                        window,
                        fixed_histogram::exact_percentiles(&window_durations)
                    ),
                    "{case}: {window:?}"
                );

                let totals = calc::totals(&stored.oracle, &grid, scope);
                assert_eq!(
                    (histogram.totals.count, histogram.totals.errors),
                    (totals.spans, totals.errors),
                    "{case}"
                );
            }
        }
    }
}

/// The engine's stacked buckets in the calculator's shape.
fn calc_buckets(histogram: &explore::HistogramData) -> Vec<calc::Bucket> {
    let mut out = Vec::with_capacity(histogram.buckets.len());
    for bucket in &histogram.buckets {
        let mut counts = BTreeMap::new();
        for (value, count) in histogram.dimensions.iter().zip(&bucket.counts) {
            if *count > 0 {
                counts.insert(value.clone(), *count);
            }
        }
        out.push(calc::Bucket {
            counts,
            unset: bucket.unset,
            other: bucket.other,
        });
    }
    out
}

/// ORC-FACET through the explorer engine: for every field the calculator
/// knows (events and links excepted, which it does not model yet), the facet
/// counts under a scope, each field's own chips excluded, equal the
/// calculator's; the default facet set holds no hidden field.
#[test]
fn explore_facets_match_the_calculator() {
    let stored = store(400, 71);
    let grid = stored.grid;
    let mut fields: Vec<String> = stored
        .oracle
        .iter()
        .flat_map(|span| span.fields.keys().map(str::to_string))
        .filter(|field| !HIDDEN_FIELDS.contains(&field.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    fields.retain(|field| !field.starts_with("events.") && !field.starts_with("links."));
    let scopes = [
        ("every span", Scope::default()),
        ("entry spans", Scope::entry_spans()),
        (
            "checkout entry spans",
            Scope::entry_spans().with(model::SERVICE_FIELD, &["checkout", "frontend"]),
        ),
    ];
    for live in [Live::Tail, Live::Split(100)] {
        for (scope_name, scope) in &scopes {
            let want = calc::facets(&stored.oracle, &grid, scope, Some(&fields));
            let mut query = explore_query(&grid, scope, model::STATUS_FIELD);
            query.sections.histogram = None;
            query.sections.facets = Some(FacetSpec {
                fields: Some(fields.clone()),
            });
            let data = explore::explore(
                explore_sources(&stored, live),
                query,
                explore::ExploreOptions::default(),
                tokio_util::sync::CancellationToken::new(),
                std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            )
            .unwrap();
            let case = format!("{live:?} {scope_name}");
            assert!(data.status.is_complete(), "{case}: {:?}", data.status);
            let facets = data.facets.expect("facets section");
            assert!(facets.unavailable.is_empty(), "{case}");
            assert_eq!(facets.fields.len(), fields.len(), "{case}");
            for (facet, want) in facets.fields.iter().zip(&want.fields) {
                assert_eq!(facet.field, want.field, "{case}");
                let got: Vec<(String, u64)> = facet
                    .values
                    .iter()
                    .map(|v| (v.value.clone(), v.count))
                    .collect();
                assert_eq!(got, want.values, "{case} field {}", facet.field);
            }
            let status = want
                .fields
                .iter()
                .find(|facet| facet.field == model::STATUS_FIELD)
                .expect("the status facet is requested");
            if scope.terms.is_empty() {
                assert!(
                    status.values.iter().any(|(value, _)| value == "unset"),
                    "{case}: every span includes some with an unset status"
                );
            }
        }
    }

    let mut query = explore_query(&grid, &Scope::entry_spans(), model::STATUS_FIELD);
    query.sections.facets = Some(FacetSpec { fields: None });
    let data = explore::explore(
        explore_sources(&stored, Live::Tail),
        query,
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    let default: BTreeSet<String> = data
        .facets
        .unwrap()
        .fields
        .into_iter()
        .map(|f| f.field)
        .collect();
    for field in &fields {
        assert!(default.contains(field), "default facets miss {field}");
    }
    for hidden in HIDDEN_FIELDS {
        assert!(!default.contains(hidden));
    }
}

/// ORC-FILTER F4 (literal text) and F5 (trace ids) through the explorer
/// engine: the stacked histogram and totals equal the calculator's.
#[test]
fn explore_text_and_trace_id_scopes_match_the_calculator() {
    let stored = store(400, 81);
    let mut ids: Vec<[u8; 16]> = stored.oracle.iter().filter_map(|s| s.trace_id).collect();
    ids.sort();
    ids.dedup();
    let chosen = [ids[3], ids[ids.len() / 2], ids[ids.len() - 2]];
    let scopes = [
        ("F4 text", Scope::entry_spans().with_text("PLACEORDER")),
        ("F4 text, every span", Scope::default().with_text("redis")),
        ("F5 trace ids", Scope::default().with_trace_ids(&chosen)),
    ];
    for live in [Live::Tail, Live::Split(100)] {
        for (name, scope) in &scopes {
            assert_histogram_matches(&stored, live, scope, &format!("{live:?} {name}"));
        }
    }
}

/// The status-stacked histogram and totals of `scope` equal the calculator's,
/// and the scope selects rows.
fn assert_histogram_matches(stored: &Stored, live: Live, scope: &Scope, case: &str) {
    let grid = stored.grid;
    let data = explore::explore(
        explore_sources(stored, live),
        explore_query(&grid, scope, model::STATUS_FIELD),
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    assert!(data.status.is_complete(), "{case}: {:?}", data.status);
    let histogram = data.histogram.expect("histogram");
    let totals = calc::totals(&stored.oracle, &grid, scope);
    assert!(totals.spans > 0, "{case}: the scenario selects rows");
    assert_eq!(
        (histogram.totals.count, histogram.totals.errors),
        (totals.spans, totals.errors),
        "{case}"
    );
    let want = calc::histogram(&stored.oracle, &grid, scope, model::STATUS_FIELD);
    assert_eq!(calc_buckets(&histogram), want, "{case}");
}

/// ORC-FILTER with absent chips (W-1): the stored unset status beside them,
/// spans without a multi-valued field, a field no row has, and a
/// high-cardinality field OR'd with a value (high in the sealed unit only);
/// the histogram and totals equal the calculator's however the live WAL is
/// served.
#[test]
fn explore_absent_chips_match_the_calculator() {
    let stored = store(400, 81);
    let scopes = [
        (
            "entry spans, unset status",
            Scope::entry_spans().with(model::STATUS_FIELD, &["unset"]),
        ),
        (
            "entry spans, errors or unset",
            Scope::entry_spans().with(model::STATUS_FIELD, &["error", "unset"]),
        ),
        (
            "spans without tags",
            Scope::default().with_absent("attributes.app.tags[]"),
        ),
        (
            "a field no row has",
            Scope::default().with_absent("attributes.nope"),
        ),
    ];
    for live in [Live::Tail, Live::Split(100)] {
        for (name, scope) in &scopes {
            assert_histogram_matches(&stored, live, scope, &format!("{live:?} {name}"));
        }
    }

    let high = store_high_card();
    let route = Scope::default()
        .with(ROUTE_FIELD, &["/r0007", "/1"])
        .with_absent(ROUTE_FIELD);
    for live in [Live::Tail, Live::Chunked] {
        assert_histogram_matches(&high, live, &route, &format!("{live:?} high route"));
    }
}

/// A multi-valued field some spans lack.
const TAGS_FIELD: &str = "attributes.app.tags[]";

/// Columns every rows comparison reads: a plain value, an array's values and a
/// field no row has.
const ROW_COLUMNS: [&str; 3] = [
    "attributes.http.route",
    "attributes.app.tags[]",
    "attributes.nope",
];

fn rows_query(grid: &Grid, scope: &Scope, order: RowOrder, limit: usize) -> ExploreQuery {
    let mut query = explore_query(grid, scope, model::STATUS_FIELD);
    query.sections.histogram = None;
    query.sections.rows = Some(RowsSpec {
        order,
        limit,
        columns: ROW_COLUMNS.iter().map(|c| c.to_string()).collect(),
    });
    query
}

fn run_rows(stored: &Stored, live: Live, query: ExploreQuery) -> RowsData {
    let data = explore::explore(
        explore_sources(stored, live),
        query,
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    assert!(data.status.is_complete(), "{:?}", data.status);
    data.rows.expect("rows section")
}

fn oracle_key(key: &RowKey) -> calc::RowKey {
    (
        key.start_ns,
        *key.trace_id.as_bytes(),
        *key.span_id.as_bytes(),
    )
}

fn engine_key(key: calc::RowKey) -> RowKey {
    RowKey {
        start_ns: key.0,
        trace_id: sfst::TraceId::from(key.1),
        span_id: sfst::SpanId::from(key.2),
    }
}

/// Every row of the page equals the calculator's row: key, duration, self
/// time, the shown fields and the asked columns. Copies of a resent row share
/// its key but not always its self time (their children may sit in other
/// files), and neither side orders them, so self times compare per key.
fn assert_rows_match(got: &RowsData, want: &[&OracleSpan], case: &str) {
    assert_eq!(got.items.len(), want.len(), "{case}");
    let mut got_self: BTreeMap<calc::RowKey, Vec<Option<i64>>> = BTreeMap::new();
    for row in &got.items {
        let copies = got_self.entry(oracle_key(&row.key)).or_default();
        copies.push(row.self_duration_ns);
        copies.sort_unstable();
    }
    let mut want_self: BTreeMap<calc::RowKey, Vec<Option<i64>>> = BTreeMap::new();
    for span in want {
        let copies = want_self.entry(calc::row_key(span)).or_default();
        copies.push(span.self_ns);
        copies.sort_unstable();
    }
    assert_eq!(got_self, want_self, "{case}");
    for (row, span) in got.items.iter().zip(want) {
        assert_eq!(oracle_key(&row.key), calc::row_key(span), "{case}");
        assert_eq!(row.duration_ns, span.duration_ns, "{case}");
        let first = |field: &str| {
            span.fields
                .get(field)
                .and_then(|values| values.iter().next().map(str::to_string))
        };
        assert_eq!(row.service, first(model::SERVICE_FIELD), "{case}");
        assert_eq!(row.name, first("name"), "{case}");
        assert_eq!(row.role, first(model::ROLE_FIELD), "{case}");
        assert_eq!(row.status, first(model::STATUS_FIELD), "{case}");
        let mut columns = Vec::new();
        for column in ROW_COLUMNS {
            let values: Vec<String> = match span.fields.get(column) {
                Some(values) => values.iter().map(str::to_string).collect(),
                None => Vec::new(),
            };
            columns.push(values);
        }
        assert_eq!(row.columns, columns, "{case}");
    }
}

/// ORC-ROWS: walking the newest-first list older from the top, and newer from
/// the oldest row, visits every scope row once in key order; every page, its
/// has_older/has_newer and each row's fields equal the calculator's, and a
/// group of resent rows is never split across pages.
#[test]
fn explore_rows_pages_match_the_calculator() {
    const LIMIT: usize = 25;
    let stored = store_resending(60, 91, Some(3));
    let grid = stored.grid;
    let mut ids: Vec<[u8; 16]> = stored.oracle.iter().filter_map(|s| s.trace_id).collect();
    ids.sort();
    ids.dedup();
    let chosen = [ids[1], ids[ids.len() / 2]];
    // The last flag: the scope is large enough for some page to end inside a
    // resent group.
    let scopes = [
        ("F0 every span", Scope::default(), true),
        ("F1 entry spans", Scope::entry_spans(), true),
        (
            "F5 trace ids",
            Scope::default().with_trace_ids(&chosen),
            false,
        ),
    ];
    for live in [Live::Tail, Live::Chunked] {
        for (name, scope, splits_groups) in &scopes {
            let totals = calc::totals(&stored.oracle, &grid, scope);
            let mut all = calc::newest_page(
                &stored.oracle,
                &grid,
                scope,
                usize::MAX,
                None,
                calc::Walk::Older,
            )
            .rows;
            assert_eq!(all.len() as u64, totals.spans);

            let mut anchor = None;
            let mut walked = Vec::new();
            let mut grown = 0;
            loop {
                let case = format!("{live:?} {name} older from {anchor:?}");
                let order = RowOrder::Newest {
                    anchor,
                    direction: RowDirection::Older,
                };
                let got = run_rows(&stored, live, rows_query(&grid, scope, order, LIMIT));
                let want = calc::newest_page(
                    &stored.oracle,
                    &grid,
                    scope,
                    LIMIT,
                    anchor.as_ref().map(oracle_key),
                    calc::Walk::Older,
                );
                assert_rows_match(&got, &want.rows, &case);
                let more = got.more.expect("a newest page says what lies beyond");
                assert_eq!(
                    (more.older, more.newer),
                    (want.has_older, want.has_newer),
                    "{case}"
                );
                assert_eq!(got.matched, totals.spans, "{case}");
                if got.items.len() > LIMIT {
                    grown += 1;
                }
                walked.extend(got.items.iter().map(|row| oracle_key(&row.key)));
                match got.items.last() {
                    Some(last) if more.older => anchor = Some(last.key),
                    _ => break,
                }
            }
            let expected: Vec<calc::RowKey> = all.iter().map(|span| calc::row_key(span)).collect();
            assert_eq!(walked, expected, "{live:?} {name}: the older walk");
            assert_eq!(
                grown > 0,
                *splits_groups,
                "{live:?} {name}: some page ends inside a resent group"
            );

            let oldest = calc::row_key(all.pop().expect("the scope has rows"));
            let mut anchor = engine_key(oldest);
            let mut pages = Vec::new();
            loop {
                let case = format!("{live:?} {name} newer from {anchor:?}");
                let order = RowOrder::Newest {
                    anchor: Some(anchor),
                    direction: RowDirection::Newer,
                };
                let got = run_rows(&stored, live, rows_query(&grid, scope, order, LIMIT));
                let want = calc::newest_page(
                    &stored.oracle,
                    &grid,
                    scope,
                    LIMIT,
                    Some(oracle_key(&anchor)),
                    calc::Walk::Newer,
                );
                assert_rows_match(&got, &want.rows, &case);
                let more = got.more.expect("a newest page says what lies beyond");
                assert_eq!(
                    (more.older, more.newer),
                    (want.has_older, want.has_newer),
                    "{case}"
                );
                let keys: Vec<calc::RowKey> =
                    got.items.iter().map(|row| oracle_key(&row.key)).collect();
                pages.push(keys);
                match got.items.first() {
                    Some(first) if more.newer => anchor = first.key,
                    _ => break,
                }
            }
            let mut walked = Vec::new();
            for page in pages.into_iter().rev() {
                walked.extend(page);
            }
            let expected: Vec<calc::RowKey> = all
                .iter()
                .map(|span| calc::row_key(span))
                .filter(|key| *key > oldest)
                .collect();
            assert_eq!(walked, expected, "{live:?} {name}: the newer walk");
        }
    }

    // The live WAL served differently on each page, as when its tail is cut into
    // chunks between two requests: the cursor is content, so the walk is unchanged.
    let servings = [Live::Tail, Live::Chunked, Live::Chunk];
    let scope = Scope::default();
    let mut anchor = None;
    let mut walked = Vec::new();
    for page in 0.. {
        let order = RowOrder::Newest {
            anchor,
            direction: RowDirection::Older,
        };
        let serving = servings[page % servings.len()];
        let got = run_rows(&stored, serving, rows_query(&grid, &scope, order, LIMIT));
        walked.extend(got.items.iter().map(|row| oracle_key(&row.key)));
        match (got.items.last(), got.more) {
            (Some(last), Some(more)) if more.older => anchor = Some(last.key),
            _ => break,
        }
    }
    let all = calc::newest_page(
        &stored.oracle,
        &grid,
        &scope,
        usize::MAX,
        None,
        calc::Walk::Older,
    );
    let expected: Vec<calc::RowKey> = all.rows.iter().map(|span| calc::row_key(span)).collect();
    assert_eq!(walked, expected, "a walk across changing servings");
}

/// ORC-TOPK: the slowest K rows, ties broken by start then ids, equal the
/// calculator's for K smaller and larger than the scope.
#[test]
fn explore_slowest_rows_match_the_calculator() {
    let stored = store_resending(60, 92, Some(4));
    let grid = stored.grid;
    for live in [Live::Tail, Live::Chunked] {
        for (name, scope) in [
            ("F0 every span", Scope::default()),
            ("F1 entry spans", Scope::entry_spans()),
        ] {
            for k in [1, 7, 1000] {
                let case = format!("{live:?} {name} k {k}");
                let got = run_rows(
                    &stored,
                    live,
                    rows_query(&grid, &scope, RowOrder::Slowest, k),
                );
                assert_eq!(got.more, None, "{case}");
                let want = calc::slowest(&stored.oracle, &grid, &scope, k);
                assert!(!want.is_empty(), "{case}");
                assert_rows_match(&got, &want, &case);
            }
        }
    }
}

/// ORC-FIELDS: the field list names exactly the calculator's fields; with the
/// live WAL read as one unit, each field's tier (and so what it supports)
/// equals the calculator's too.
#[test]
fn explore_field_list_matches_the_calculator() {
    let stored = store(2400, 101);
    let grid = stored.grid;
    let want = calc::field_list(&stored.oracle);
    let tiers: BTreeSet<calc::Tier> = want.values().copied().collect();
    assert!(
        tiers.contains(&calc::Tier::Mid) && tiers.contains(&calc::Tier::High),
        "the corpus reaches every tier: {tiers:?}"
    );
    for live in [Live::Tail, Live::Chunk, Live::Split(100)] {
        let mut query = explore_query(&grid, &Scope::default(), model::STATUS_FIELD);
        query.sections.histogram = None;
        query.sections.fields = true;
        let data = explore::explore(
            explore_sources(&stored, live),
            query,
            explore::ExploreOptions::default(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .unwrap();
        let fields = data.fields.expect("fields section");
        assert!(fields.status.is_complete(), "{live:?}");
        assert!(
            fields.items.iter().any(|f| !f.column),
            "events and links fields exist"
        );
        let names: Vec<&str> = fields.items.iter().map(|f| f.name.as_str()).collect();
        let want_names: Vec<&str> = want.keys().map(String::as_str).collect();
        assert_eq!(names, want_names, "{live:?}");
        if matches!(live, Live::Split(_)) {
            continue;
        }
        for field in &fields.items {
            let tier = match field.tier {
                sfst::FieldTier::Low => calc::Tier::Low,
                sfst::FieldTier::Mid => calc::Tier::Mid,
                sfst::FieldTier::High => calc::Tier::High,
            };
            assert_eq!(tier, want[&field.name], "{live:?} {}", field.name);
            assert_eq!(field.facet, tier != calc::Tier::High, "{}", field.name);
            let per_event = field.name.starts_with("events.") || field.name.starts_with("links.");
            assert_eq!(field.column, !per_event, "{}", field.name);
        }
    }
}

fn run_values(stored: &Stored, live: Live, query: explore::ValuesQuery) -> explore::ValuesData {
    let data = explore::field_values(
        explore_sources(stored, live),
        query,
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    assert!(data.status.is_complete(), "{:?}", data.status);
    data
}

/// ORC-VALUES: over the whole window the suggestions are the calculator's
/// values with the prefix, the first `limit` in byte order with `truncated`
/// exact; over a narrower window they include every value of its rows (a file
/// is read whole, so values of its rows outside the window may come too).
#[test]
fn explore_values_match_the_calculator() {
    let stored = store(2400, 111);
    let grid = stored.grid;
    let window_ns = |grid: &Grid| {
        i64::from(grid.after_s) * 1_000_000_000..i64::from(grid.before_s) * 1_000_000_000
    };
    let cases = [
        ("name", "", 1000),
        ("name", "GET", 1000),
        (model::SERVICE_FIELD, "c", 2),
        ("attributes.request.id", "", 10),
        ("attributes.request.id", "a1", 1000),
        ("events.attributes.exception.type", "", 1000),
        (model::ERR_ORIGIN_FIELD, "", 10),
        ("attributes.nope", "", 5),
    ];
    for live in [Live::Tail, Live::Split(100)] {
        for (field, prefix, limit) in cases {
            let case = format!("{live:?} {field} {prefix:?} {limit}");
            let got = run_values(
                &stored,
                live,
                explore::ValuesQuery {
                    window: window_ns(&grid),
                    field: field.to_string(),
                    prefix: prefix.to_string(),
                    limit,
                },
            );
            let all = calc::field_values(&stored.oracle, &grid, field, prefix);
            let want: Vec<String> = all.iter().take(limit).cloned().collect();
            assert_eq!(got.values, want, "{case}");
            assert_eq!(got.truncated, all.len() > limit, "{case}");
        }
    }

    let narrow = Grid::for_window(
        grid.after_s + (grid.before_s - grid.after_s) / 3,
        grid.after_s + (grid.before_s - grid.after_s) / 2,
    );
    let got = run_values(
        &stored,
        Live::Tail,
        explore::ValuesQuery {
            window: window_ns(&narrow),
            field: "attributes.request.id".to_string(),
            prefix: "a".to_string(),
            limit: 1000,
        },
    );
    let within = calc::field_values(&stored.oracle, &narrow, "attributes.request.id", "a");
    assert!(!within.is_empty() && !got.truncated);
    let got: BTreeSet<String> = got.values.into_iter().collect();
    assert!(
        got.is_superset(&within),
        "a narrower window keeps its rows' values"
    );
}

/// The keys of a walk through every newest page, older from the top, with the
/// pages' boundaries.
fn walk_older(
    stored: &Stored,
    live: Live,
    scope: &Scope,
    limit: usize,
) -> (Vec<calc::RowKey>, Vec<usize>) {
    let mut anchor = None;
    let mut walked = Vec::new();
    let mut ends = Vec::new();
    loop {
        let order = RowOrder::Newest {
            anchor,
            direction: RowDirection::Older,
        };
        let got = run_rows(stored, live, rows_query(&stored.grid, scope, order, limit));
        let want = calc::newest_page(
            &stored.oracle,
            &stored.grid,
            scope,
            limit,
            anchor.as_ref().map(oracle_key),
            calc::Walk::Older,
        );
        assert_rows_match(&got, &want.rows, &format!("{live:?} from {anchor:?}"));
        walked.extend(got.items.iter().map(|row| oracle_key(&row.key)));
        ends.push(walked.len());
        match (got.items.last(), got.more) {
            (Some(last), Some(more)) if more.older => anchor = Some(last.key),
            _ => break,
        }
    }
    (walked, ends)
}

/// ORC-ROWS with a coarse exporter clock: many spans share a start with
/// distinct ids, so pages end inside start groups; the walk still visits every
/// row once in key order.
#[test]
fn explore_rows_walk_through_shared_starts() {
    let stored = store_shaped(60, 93, None, Some(10_000_000));
    let scope = Scope::default();
    let all: Vec<calc::RowKey> = calc::newest_page(
        &stored.oracle,
        &stored.grid,
        &scope,
        usize::MAX,
        None,
        calc::Walk::Older,
    )
    .rows
    .iter()
    .map(|span| calc::row_key(span))
    .collect();
    for live in [Live::Tail, Live::Chunked] {
        let (walked, ends) = walk_older(&stored, live, &scope, 7);
        assert_eq!(walked, all, "{live:?}");
        let inside_a_group = ends
            .iter()
            .filter(|&&end| end < walked.len())
            .filter(|&&end| walked[end - 1].0 == walked[end].0 && walked[end - 1] != walked[end])
            .count();
        assert!(
            inside_a_group > 0,
            "{live:?}: some page ends inside a start group"
        );
    }
}

/// ORC-ROWS: a resent request's copies landed in different units; a one-row
/// page reaching that key holds both copies, whichever unit serves each.
#[test]
fn explore_rows_keep_a_group_split_across_units_together() {
    let stored = store_resending(60, 94, Some(5));
    let scope = Scope::default();
    let all = calc::newest_page(
        &stored.oracle,
        &stored.grid,
        &scope,
        usize::MAX,
        None,
        calc::Walk::Older,
    )
    .rows;
    let split = (1..all.len())
        .find(|&i| {
            calc::row_key(all[i]) == calc::row_key(all[i - 1]) && all[i].unit != all[i - 1].unit
        })
        .expect("the cut splits a resent request between the units");
    let group = calc::row_key(all[split]);
    let first = (0..all.len())
        .find(|&i| calc::row_key(all[i]) == group)
        .unwrap();
    assert!(first > 0, "a newer row gives the anchor");
    let anchor = engine_key(calc::row_key(all[first - 1]));
    for live in [Live::Tail, Live::Chunk, Live::Chunked] {
        let order = RowOrder::Newest {
            anchor: Some(anchor),
            direction: RowDirection::Older,
        };
        let got = run_rows(&stored, live, rows_query(&stored.grid, &scope, order, 1));
        let keys: Vec<calc::RowKey> = got.items.iter().map(|row| oracle_key(&row.key)).collect();
        assert_eq!(keys, [group, group], "{live:?}");
        assert_eq!(
            got.more,
            Some(explore::MoreRows {
                older: true,
                newer: true
            }),
            "{live:?}"
        );
    }
}

/// ORC-ROWS at the edges: Newer without an anchor (the oldest page), an anchor
/// that is no stored row, and anchors before and after the window.
#[test]
fn explore_rows_anchor_edges_match_the_calculator() {
    let stored = store_resending(60, 95, Some(4));
    let grid = stored.grid;
    let scope = Scope::entry_spans();
    let all = calc::newest_page(
        &stored.oracle,
        &grid,
        &scope,
        usize::MAX,
        None,
        calc::Walk::Older,
    )
    .rows;
    let middle = calc::row_key(all[all.len() / 2]);
    let window_start = i64::from(grid.after_s) * 1_000_000_000;
    let window_end = i64::from(grid.before_s) * 1_000_000_000;
    let cases: [(&str, Option<calc::RowKey>, calc::Walk); 6] = [
        ("oldest page", None, calc::Walk::Newer),
        (
            "between rows, older",
            Some((middle.0, [0xff; 16], [0xff; 8])),
            calc::Walk::Older,
        ),
        (
            "between rows, newer",
            Some((middle.0, [0; 16], [0; 8])),
            calc::Walk::Newer,
        ),
        (
            "before the window, older",
            Some((window_start - 1, [0; 16], [0; 8])),
            calc::Walk::Older,
        ),
        (
            "before the window, newer",
            Some((window_start - 1, [0; 16], [0; 8])),
            calc::Walk::Newer,
        ),
        (
            "after the window, older",
            Some((window_end, [0; 16], [0; 8])),
            calc::Walk::Older,
        ),
    ];
    for live in [Live::Tail, Live::Chunked] {
        for (name, anchor, walk) in cases {
            let case = format!("{live:?} {name}");
            let direction = match walk {
                calc::Walk::Older => RowDirection::Older,
                calc::Walk::Newer => RowDirection::Newer,
            };
            let order = RowOrder::Newest {
                anchor: anchor.map(engine_key),
                direction,
            };
            let got = run_rows(&stored, live, rows_query(&grid, &scope, order, 5));
            let want = calc::newest_page(&stored.oracle, &grid, &scope, 5, anchor, walk);
            assert_rows_match(&got, &want.rows, &case);
            let more = got.more.expect("newest");
            assert_eq!(
                (more.older, more.newer),
                (want.has_older, want.has_newer),
                "{case}"
            );
        }
    }
}

/// QRY-31 for rows: a file whose rows' fields cannot be read (its one
/// high-cardinality column is corrupt; everything the selection reads is
/// intact) is counted as failed and the page is selected again from the other
/// units, so it stays contiguous with a cursor to continue from.
#[test]
fn explore_rows_leave_out_a_file_whose_fields_fail() {
    let stored = store(2400, 96);
    let high: Vec<String> = sfst::IndexReader::open(&stored.sealed)
        .unwrap()
        .field_table()
        .iter()
        .filter(|entry| entry.is_high_card())
        .map(|entry| entry.name.clone())
        .collect();
    assert_eq!(
        high,
        ["attributes.request.id"],
        "the corrupted chunk is this column"
    );
    let path = stored._dir.path().join("sealed-corrupt.sfst");
    std::fs::write(&path, &stored.sealed).unwrap();
    common::corrupt_chunk(&path, [b'H', b'F', 0, 0]);
    let sources = vec![
        common::sealed_source_at(&path, "sealed"),
        common::tail_source(&stored.live_wal, "live"),
    ];

    // The oldest page lies in the sealed file.
    let scope = Scope::default();
    let order = RowOrder::Newest {
        anchor: None,
        direction: RowDirection::Newer,
    };
    let mut query = rows_query(&stored.grid, &scope, order, 5);
    query.sections.rows.as_mut().unwrap().columns = vec!["attributes.request.id".to_string()];
    let data = explore::explore(
        sources,
        query,
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    let rows = data.rows.expect("rows section");
    let failed = rows
        .status
        .count(sfsq::traces::PartialReason::SourceFailure)
        .map(|r| (r.count, r.of));
    assert_eq!(failed, Some((1, Some(2))));
    assert!(data.histogram.is_none());

    let live: Vec<OracleSpan> = stored
        .oracle
        .iter()
        .filter(|s| s.unit == LIVE)
        .cloned()
        .collect();
    let want = calc::newest_page(&live, &stored.grid, &scope, 5, None, calc::Walk::Newer);
    let keys: Vec<calc::RowKey> = rows.items.iter().map(|row| oracle_key(&row.key)).collect();
    let want_keys: Vec<calc::RowKey> = want.rows.iter().map(|span| calc::row_key(span)).collect();
    assert_eq!(keys, want_keys, "the page comes from the live unit alone");
    let more = rows.more.expect("newest");
    assert_eq!((more.older, more.newer), (want.has_older, want.has_newer));
    assert!(
        !rows.items.is_empty() && more.newer,
        "a cursor to continue from"
    );
}

const ROUTE_FIELD: &str = "attributes.http.route";
const ITEM_FIELD: &str = "attributes.item";

/// Spans with a route and an item: the sealed file holds 1,100 distinct routes
/// (high there) and 600 items, the live WAL 3 routes and 600 other items, so
/// only the sealed file makes the route high, and only the two units together
/// take the items past the facet cap.
fn store_high_card() -> Stored {
    let span = |i: u64, route: String, item: String| {
        let mut trace_id = vec![0u8; 16];
        trace_id[8..].copy_from_slice(&(i / 10 + 1).to_be_bytes());
        let start = T0_S * 1_000_000_000 + i * 10_000_000;
        Span {
            trace_id,
            span_id: (i + 1).to_be_bytes().to_vec(),
            name: "work".to_string(),
            kind: 2,
            start_time_unix_nano: start,
            end_time_unix_nano: start + 1_000_000,
            attributes: vec![
                common::kv_str("http.route", &route),
                common::kv_str("item", &item),
            ],
            ..Default::default()
        }
    };
    let mut spans = Vec::new();
    for i in 0..1_100u64 {
        spans.push(span(i, format!("/r{i:04}"), format!("s{:03}", i % 600)));
    }
    for i in 1_100..1_700u64 {
        spans.push(span(i, format!("/{}", i % 3), format!("l{:03}", i % 600)));
    }
    let requests: Vec<ExportTraceServiceRequest> = spans
        .chunks(50)
        .map(|batch| ExportTraceServiceRequest {
            resource_spans: vec![ResourceSpans {
                resource: Some(Resource {
                    attributes: vec![common::kv_str("service.name", "svc")],
                    ..Default::default()
                }),
                scope_spans: vec![ScopeSpans {
                    spans: batch.to_vec(),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        })
        .collect();
    store_requests(&requests, 1_100 / 50)
}

/// One of the engine's partial reasons in the calculator's shape.
fn stated(status: &QueryStatus, reason: PartialReason, name: &'static str) -> Option<calc::Reason> {
    status.count(reason).map(|count| calc::Reason {
        reason: name,
        count: count.count,
        of: count.of,
        detail: count.detail.clone(),
    })
}

/// ORC-HIST and ORC-STATUS for a stack field that is high in one unit: that
/// unit's scope rows count as `other` in their buckets, the rest by value, and
/// the histogram names the field as partial, out of every source it read.
#[test]
fn explore_counts_a_stack_field_high_in_one_unit_as_other() {
    let stored = store_high_card();
    let grid = stored.grid;
    for live in [Live::Tail, Live::Chunked] {
        let sources = explore_sources(&stored, live);
        let candidates = sources.len() as u64;
        // Every other section asked too: the reason is the histogram's alone.
        let mut query = explore_query(&grid, &Scope::default(), ROUTE_FIELD);
        query.sections.facets = Some(FacetSpec {
            fields: Some(vec![model::STATUS_FIELD.to_string()]),
        });
        query.sections.groups = true;
        query.sections.rows = Some(RowsSpec {
            order: RowOrder::Slowest,
            limit: 5,
            columns: Vec::new(),
        });
        query.sections.fields = true;
        let data = explore::explore(
            sources,
            query,
            explore::ExploreOptions::default(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .unwrap();
        let case = format!("{live:?}");
        let histogram = data.histogram.expect("histogram section");

        let want = calc::histogram(&stored.oracle, &grid, &Scope::default(), ROUTE_FIELD);
        assert_eq!(calc_buckets(&histogram), want, "{case}");
        assert_eq!(want.iter().map(|b| b.other).sum::<u64>(), 1_100, "{case}");
        assert_eq!(
            stated(
                &histogram.status,
                PartialReason::StackFieldHighCard,
                "stack_field_high_card"
            ),
            calc::histogram_reasons(&stored.oracle, ROUTE_FIELD, candidates).pop(),
            "{case}"
        );
        assert_eq!(
            histogram.status.reasons(),
            BTreeSet::from([PartialReason::StackFieldHighCard]),
            "{case}"
        );
        let others = [
            ("facets", data.facets.expect("facets").status),
            ("groups", data.groups.expect("groups").status),
            ("rows", data.rows.expect("rows").status),
            ("fields", data.fields.expect("fields").status),
        ];
        for (section, status) in others {
            assert!(status.is_complete(), "{case}: {section} {status:?}");
        }
    }
}

/// ORC-FACET and ORC-STATUS at the edges: a requested field high in one unit is
/// left out and named, a field whose values pass the cap only across units
/// lists the calculator's top values with what it omitted, and both reasons
/// carry the calculator's counts and fields.
#[test]
fn explore_names_high_and_capped_facets() {
    let stored = store_high_card();
    let grid = stored.grid;
    let requested = [
        ROUTE_FIELD.to_string(),
        ITEM_FIELD.to_string(),
        model::ROLE_FIELD.to_string(),
    ];
    let want = calc::facets(&stored.oracle, &grid, &Scope::default(), Some(&requested));
    assert_eq!(want.unavailable, vec![ROUTE_FIELD.to_string()]);
    assert_eq!(want.fields[0].omitted_values, 200);

    for live in [Live::Tail, Live::Chunked] {
        let mut query = explore_query(&grid, &Scope::default(), model::STATUS_FIELD);
        query.sections.histogram = None;
        query.sections.facets = Some(FacetSpec {
            fields: Some(requested.to_vec()),
        });
        let data = explore::explore(
            explore_sources(&stored, live),
            query,
            explore::ExploreOptions::default(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .unwrap();
        let case = format!("{live:?}");
        let facets = data.facets.expect("facets section");

        let unavailable: Vec<String> = facets.unavailable.iter().map(|(f, _)| f.clone()).collect();
        assert_eq!(unavailable, want.unavailable, "{case}");
        let got: Vec<calc::Facet> = facets
            .fields
            .iter()
            .map(|facet| calc::Facet {
                field: facet.field.clone(),
                values: facet
                    .values
                    .iter()
                    .map(|v| (v.value.clone(), v.count))
                    .collect(),
                omitted_values: facet.omitted_values,
                omitted_rows: facet.omitted_rows,
            })
            .collect();
        assert_eq!(got, want.fields, "{case}");

        let mut reasons = Vec::new();
        for (reason, name) in [
            (PartialReason::FacetHighCard, "facet_high_card"),
            (PartialReason::FacetValueCap, "facet_value_cap"),
        ] {
            reasons.extend(stated(&facets.status, reason, name));
        }
        assert_eq!(reasons, calc::facet_reasons(&want), "{case}");
    }
}

/// A span of trace `trace` with id `id` under `parent`, starting `start_ms`
/// after T0 and lasting `duration_ms`, with status ERROR when `error`.
fn family_span(
    trace: u8,
    id: u8,
    parent: Option<u8>,
    (start_ms, duration_ms): (u64, u64),
    error: bool,
) -> Span {
    let start = T0_S * 1_000_000_000 + start_ms * 1_000_000;
    Span {
        trace_id: vec![trace; 16],
        span_id: vec![id; 8],
        parent_span_id: parent.map_or_else(Vec::new, |parent| vec![parent; 8]),
        name: format!("op{id}"),
        kind: 2,
        start_time_unix_nano: start,
        end_time_unix_nano: start + duration_ms * 1_000_000,
        status: error.then(|| Status {
            code: 2,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// One export request (one WAL frame) holding `spans` of one service.
fn frame_of(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![common::kv_str("service.name", "svc")],
                ..Default::default()
            }),
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// The ids of the error origins in [`family_store`]: C, the ERROR child Y of
/// an OK root, and the lone ERROR root Z.
const FAMILY_ORIGINS: [(u8, u8); 3] = [(1, 13), (2, 22), (3, 31)];

/// A filler span sealed, then a live WAL of four frames (3, 2, 2 and 1 spans)
/// whose families cross frames: frame 0 holds root R and its ERROR child P,
/// frame 1 P's ERROR child C, frame 2 R's OK child D, so an image of one
/// frame alone would take P for an origin and miss the time D covers.
fn family_store() -> Stored {
    family_store_cut(1)
}

/// [`family_store`] with the first `cut` export requests sealed (the filler,
/// then the live frames in order).
fn family_store_cut(cut: usize) -> Stored {
    let requests = vec![
        frame_of(vec![family_span(9, 90, None, (0, 5), false)]),
        frame_of(vec![
            family_span(1, 11, None, (10, 1_000), true),
            family_span(1, 12, Some(11), (20, 500), true),
            family_span(2, 21, None, (30, 400), false),
        ]),
        frame_of(vec![
            family_span(1, 13, Some(12), (40, 100), true),
            family_span(2, 22, Some(21), (50, 100), true),
        ]),
        frame_of(vec![
            family_span(1, 14, Some(11), (600, 300), false),
            family_span(3, 31, None, (700, 50), true),
        ]),
        frame_of(vec![family_span(4, 41, None, (800, 10), false)]),
    ];
    store_requests(&requests, cut)
}

/// What the explorer answers about error origins: the `_err_origin` facet,
/// the histogram stacked by it, and the ids of the rows it scopes to.
type OriginAnswers = (BTreeMap<String, u64>, Vec<calc::Bucket>, Vec<(u8, u8)>);

/// With the statuses of both requests and of each of their sections.
fn origin_answers(sources: Vec<TraceSource>, grid: &Grid) -> (OriginAnswers, Vec<QueryStatus>) {
    let run = |sources: Vec<TraceSource>, query: ExploreQuery| {
        explore::explore(
            sources,
            query,
            explore::ExploreOptions::default(),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .unwrap()
    };
    let mut query = explore_query(grid, &Scope::default(), model::ERR_ORIGIN_FIELD);
    query.sections.facets = Some(FacetSpec {
        fields: Some(vec![model::ERR_ORIGIN_FIELD.to_string()]),
    });
    let data = run(sources.clone(), query);
    let facets = data.facets.expect("facets section");
    let histogram = data.histogram.expect("histogram section");
    let mut statuses = vec![data.status, facets.status, histogram.status.clone()];
    let facet: BTreeMap<String, u64> = facets
        .fields
        .into_iter()
        .flat_map(|facet| facet.values)
        .map(|value| (value.value, value.count))
        .collect();
    let buckets = calc_buckets(&histogram);

    let origins = Scope::default().with(model::ERR_ORIGIN_FIELD, &["true"]);
    let order = RowOrder::Newest {
        anchor: None,
        direction: RowDirection::Older,
    };
    let data = run(sources, rows_query(grid, &origins, order, 50));
    let rows = data.rows.expect("rows section");
    statuses.extend([data.status, rows.status.clone()]);
    let mut ids: Vec<(u8, u8)> = rows
        .items
        .iter()
        .map(|row| {
            (
                row.key.trace_id.as_bytes()[0],
                row.key.span_id.as_bytes()[0],
            )
        })
        .collect();
    ids.sort_unstable();
    ((facet, buckets, ids), statuses)
}

/// A row's (trace, span) id bytes and its self time.
type SelfTime = ((u8, u8), Option<i64>);

/// Every row's ids and self time, with the rows section's status.
fn self_times(sources: Vec<TraceSource>, grid: &Grid) -> (Vec<SelfTime>, QueryStatus) {
    let order = RowOrder::Newest {
        anchor: None,
        direction: RowDirection::Older,
    };
    let data = explore::explore(
        sources,
        rows_query(grid, &Scope::default(), order, 50),
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    let rows = data.rows.expect("rows section");
    let mut out: Vec<SelfTime> = rows
        .items
        .iter()
        .map(|row| {
            let ids = (
                row.key.trace_id.as_bytes()[0],
                row.key.span_id.as_bytes()[0],
            );
            (ids, row.self_duration_ns)
        })
        .collect();
    out.sort_unstable();
    (out, rows.status)
}

/// ORC-LIVE: however the live WAL is served (all tail, one image, one chunk
/// and a tail, two chunks and a tail), the explorer answers about error
/// origins and self time exactly as it does once the WAL is sealed, and as
/// the calculator does over the WAL as a whole.
#[test]
fn live_split_equals_sealed() {
    let stored = family_store();
    let grid = stored.grid;
    let sealed_live = stored._dir.path().join("live.sfst");
    ng_index::build_sfst_traces_file(&stored.live_wal, &sealed_live, &ng_index::Metrics::new())
        .unwrap();
    let sealed_sources = vec![
        common::sealed_source_at(&stored._dir.path().join("sealed.sfst"), "sealed"),
        common::sealed_source_at(&sealed_live, "live"),
    ];
    let (sealed, statuses) = origin_answers(sealed_sources, &grid);
    assert!(
        statuses.iter().all(QueryStatus::is_complete),
        "{statuses:?}"
    );
    assert_eq!(sealed.2, FAMILY_ORIGINS);
    let facet = calc::facet_counts(
        &stored.oracle,
        &grid,
        &Scope::default(),
        model::ERR_ORIGIN_FIELD,
    );
    assert_eq!(sealed.0, facet);
    let histogram = calc::histogram(
        &stored.oracle,
        &grid,
        &Scope::default(),
        model::ERR_ORIGIN_FIELD,
    );
    assert_eq!(sealed.1, histogram);

    let sealed_sources = vec![
        common::sealed_source_at(&stored._dir.path().join("sealed.sfst"), "sealed"),
        common::sealed_source_at(&sealed_live, "live"),
    ];
    let (sealed_self, status) = self_times(sealed_sources, &grid);
    assert!(status.is_complete(), "{status:?}");
    let mut want: Vec<SelfTime> = stored
        .oracle
        .iter()
        .map(|span| {
            let ids = (span.trace_id.unwrap()[0], span.span_id.unwrap()[0]);
            (ids, span.self_ns)
        })
        .collect();
    want.sort_unstable();
    assert_eq!(sealed_self, want);
    // R's children P and D cover 800 of its 1,000 ms, in different frames.
    assert!(sealed_self.contains(&((1, 11), Some(200_000_000))));
    let sealed_sources = vec![
        common::sealed_source_at(&stored._dir.path().join("sealed.sfst"), "sealed"),
        common::sealed_source_at(&sealed_live, "live"),
    ];
    let (sealed_groups, status) = run_groups(sealed_sources, &grid, &Scope::entry_spans(), None);
    assert!(status.is_complete(), "{status:?}");
    assert_eq!(
        sealed_groups,
        calc::groups(&stored.oracle, &grid, &Scope::entry_spans(), None)
    );

    for live in [Live::Tail, Live::Chunk, Live::Split(5), Live::Split(3)] {
        let (answers, statuses) = origin_answers(explore_sources(&stored, live), &grid);
        let complete = statuses.iter().all(QueryStatus::is_complete);
        assert!(complete, "{live:?}: {statuses:?}");
        assert_eq!(answers, sealed, "{live:?}");
        let (live_self, status) = self_times(explore_sources(&stored, live), &grid);
        assert!(status.is_complete(), "{live:?}: {status:?}");
        assert_eq!(live_self, sealed_self, "{live:?}");
        let entry = Scope::entry_spans();
        let (groups, status) = run_groups(explore_sources(&stored, live), &grid, &entry, None);
        assert!(status.is_complete(), "{live:?}: {status:?}");
        assert_eq!(groups, sealed_groups, "{live:?}");
    }
}

/// ORC-LIVE: a live WAL whose captured ranges leave a gap (a chunk missing)
/// cannot be derived: its rows carry no error origin and no self time, and
/// every section that counts origins says so, as does the rows section
/// whatever its scope (every row carries self time), out of the live WALs
/// captured, and each request (once, however many of its sections name it).
#[test]
fn a_live_wal_with_a_gap_fails_its_live_pass() {
    let stored = family_store();
    let grid = stored.grid;
    let mut sources = explore_sources(&stored, Live::Split(3));
    assert_eq!(sources.len(), 4, "the sealed file, two chunks and a tail");
    sources.remove(2);
    let ((facet, _, ids), statuses) = origin_answers(sources.clone(), &grid);
    assert!(ids.is_empty(), "{ids:?}");
    assert!(!facet.contains_key("true"), "{facet:?}");
    for status in &statuses {
        let failed = status
            .count(PartialReason::LivePassFailed)
            .map(|count| (count.count, count.of));
        assert_eq!(failed, Some((1, Some(1))), "{statuses:?}");
    }

    let (rows, status) = self_times(sources, &grid);
    let failed = status
        .count(PartialReason::LivePassFailed)
        .map(|count| (count.count, count.of));
    assert_eq!(failed, Some((1, Some(1))), "{status:?}");
    assert!(rows.contains(&((9, 90), Some(5_000_000))), "{rows:?}");
    for (ids, self_ns) in &rows {
        assert!(ids.0 == 9 || self_ns.is_none(), "{rows:?}");
    }
}

/// ORC-TRACE (origin and self time, D26): a trace split across a sealed file
/// and the live WAL gets each span's error origin and self time over the
/// whole assembled trace, as the calculator derives them over all its spans,
/// however the live WAL is served. The sealed file alone stores P as an
/// origin (its ERROR child C is in the live WAL); the trace does not.
#[test]
fn trace_by_id_derives_over_the_assembled_trace() {
    let stored = family_store_cut(2);
    let trace: Vec<OracleSpan> = stored
        .oracle
        .iter()
        .filter(|span| span.trace_id == Some([1; 16]))
        .cloned()
        .collect();
    let values = calc::derived_in(&trace, &|_| Some(0));
    let mut want = BTreeMap::new();
    for (span, value) in trace.iter().zip(values) {
        let child_ns = value.child_ns;
        let self_ns = span.duration_ns - child_ns;
        want.insert(span.span_id.unwrap()[0], (value.error_origin, self_ns));
    }
    assert_eq!(want[&12], (false, 400_000_000), "P, in the sealed file");
    assert_eq!(want[&13], (true, 100_000_000), "C, in the live WAL");
    let p = stored
        .files
        .iter()
        .find(|span| span.span_id == Some([12; 8]));
    assert!(p.unwrap().has(model::ERR_ORIGIN_FIELD, "true"));

    for live in [Live::Tail, Live::Chunk, Live::Split(2)] {
        let data = sfsq::traces::trace_by_id(
            explore_sources(&stored, live),
            sfsq::traces::TraceQuery::new(sfst::TraceId::from([1; 16])),
            tokio_util::sync::CancellationToken::new(),
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
        .unwrap();
        assert!(data.status.is_complete(), "{live:?}");
        let mut got = BTreeMap::new();
        for (index, span) in data.trace.spans.iter().enumerate() {
            let origin = data.family.error_origin[index];
            let self_ns = span.duration_ns - data.family.child_ns[index];
            got.insert(span.span_id.as_bytes()[0], (origin, self_ns));
        }
        assert_eq!(got, want, "{live:?}");
    }
}

/// QRY-38: the live WAL served as a tail, as one chunk image, or as chunks and
/// a tail answers every section, a newest page and the slowest rows exactly as
/// the same WAL sealed into a file, beside the same sealed file (seeded
/// corpora).
#[test]
fn live_layouts_answer_like_the_sealed_wal() {
    for seed in [7, 8] {
        let stored = store(300, seed);
        let grid = stored.grid;
        let dir = stored._dir.path();
        let sealed_file = dir.join("sealed-again.sfst");
        std::fs::write(&sealed_file, &stored.sealed).unwrap();
        let sealed_live = dir.join("live-sealed.sfst");
        ng_index::build_sfst_traces_file(&stored.live_wal, &sealed_live, &ng_index::Metrics::new())
            .unwrap();
        let sealed_sources = vec![
            common::sealed_source_at(&sealed_file, "sealed"),
            common::sealed_source_at(&sealed_live, "live"),
        ];
        let orders = [
            RowOrder::Newest {
                anchor: None,
                direction: RowDirection::Older,
            },
            RowOrder::Slowest,
        ];
        for order in orders {
            let query = || {
                let mut query = explore_query(&grid, &Scope::default(), model::STATUS_FIELD);
                query.sections.facets = Some(FacetSpec { fields: None });
                query.sections.groups = true;
                query.sections.fields = true;
                query.sections.rows = Some(RowsSpec {
                    order,
                    limit: 25,
                    columns: ROW_COLUMNS.iter().map(|c| c.to_string()).collect(),
                });
                query
            };
            let answer = |sources: Vec<TraceSource>| {
                explore::explore(
                    sources,
                    query(),
                    explore::ExploreOptions::default(),
                    tokio_util::sync::CancellationToken::new(),
                    std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                )
                .unwrap()
            };
            let want = answer(sealed_sources.clone());
            assert!(want.status.is_complete(), "{:?}", want.status);
            for live in [Live::Tail, Live::Chunk, Live::Chunked] {
                let got = answer(explore_sources(&stored, live));
                let case = format!("seed {seed} {order:?} {live:?}");
                assert_eq!(got.histogram, want.histogram, "{case}");
                assert_eq!(got.facets, want.facets, "{case}");
                assert_eq!(got.groups, want.groups, "{case}");
                assert_eq!(got.rows, want.rows, "{case}");
                assert_eq!(got.fields, want.fields, "{case}");
            }
        }
    }
}

/// The engine's Groups section in the calculator's shape.
fn calc_groups(data: &explore::GroupsData) -> calc::Groups {
    let numbers = |n: &explore::GroupNumbers| calc::GroupNumbers {
        spans: n.spans,
        errors: n.errors,
        errors_originated: n.errors_originated,
        p95_ns: n.p95_ns,
        self_ns: n.self_ns,
    };
    let mut out = calc::Groups {
        self_ns_total: data.self_ns_total,
        total: data.rows.len() as u64,
        ..calc::Groups::default()
    };
    for row in &data.rows {
        let key = calc::GroupKey {
            service: row.key.service.clone(),
            operation: row.key.operation.clone(),
        };
        out.rows.push((key, numbers(&row.numbers)));
    }
    if let Some(other) = &data.other {
        out.other = Some((other.groups, numbers(&other.numbers)));
        out.total += other.groups;
    }
    if let Some(delta) = &data.delta {
        let side = |s: &explore::SideNumbers| calc::Side {
            spans: s.spans,
            errors_originated: s.errors_originated,
            self_ns: s.self_ns,
        };
        let sides = |sides: Option<explore::GroupSides>| {
            let sides = sides.expect("sides on every group under a selection");
            (side(&sides.selection), side(&sides.baseline))
        };
        let mut rows = Vec::new();
        for row in &data.rows {
            rows.push(sides(row.delta));
        }
        out.delta = Some(calc::Delta {
            selection_traces: delta.selection_traces,
            baseline_traces: delta.baseline_traces,
            selection_self_ns_total: delta.selection_self_ns_total,
            baseline_self_ns_total: delta.baseline_self_ns_total,
            rows,
            other: data.other.as_ref().map(|other| sides(other.delta)),
        });
    }
    out
}

/// The Groups section of `sources` for `scope` and `selection`, with its
/// status.
fn run_groups(
    sources: Vec<TraceSource>,
    grid: &Grid,
    scope: &Scope,
    selection: Option<ExploreSelection>,
) -> (calc::Groups, QueryStatus) {
    let mut query = explore_query(grid, scope, model::STATUS_FIELD);
    query.sections.histogram = None;
    query.sections.groups = true;
    query.selection = selection;
    let data = explore::explore(
        sources,
        query,
        explore::ExploreOptions::default(),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap();
    let groups = data.groups.expect("groups section");
    (calc_groups(&groups), groups.status)
}

/// ORC-GROUPS (D23, D24, D41): for every way of serving the live WAL and
/// every scope, the Groups section equals the calculator's: every window row
/// of the scope's traces across both units (resent copies counted as
/// stored), grouped, ranked and summed exactly, p95 from the same fixed
/// histogram; error origins and self time as the explorer derives them.
#[test]
fn explore_groups_match_the_calculator() {
    let stored = store_resending(300, 71, Some(7));
    let grid = stored.grid;
    let entry = Scope::entry_spans();
    let entry_traces = calc::scope_traces(&stored.oracle, &grid, &entry);
    let mut units_of: BTreeMap<[u8; 16], BTreeSet<usize>> = BTreeMap::new();
    for span in &stored.oracle {
        if let Some(trace) = span.trace_id {
            units_of.entry(trace).or_default().insert(span.unit);
        }
    }
    let straddling = entry_traces
        .iter()
        .filter(|trace| units_of[*trace].len() == 2)
        .count();
    assert!(straddling > 0, "some scope trace has rows in both units");

    let mut ids: Vec<[u8; 16]> = units_of.keys().copied().collect();
    ids.sort();
    let chosen = [ids[3], ids[ids.len() / 2], ids[ids.len() - 2]];
    let scopes = [
        ("F0 every span", Scope::default()),
        ("F1 entry spans", Scope::entry_spans()),
        (
            "F2 checkout entry spans",
            Scope::entry_spans().with(model::SERVICE_FIELD, &["checkout"]),
        ),
        ("F4 text", Scope::entry_spans().with_text("PLACEORDER")),
        ("F5 trace ids", Scope::default().with_trace_ids(&chosen)),
    ];
    for live in [Live::Tail, Live::Chunk, Live::Split(100), Live::Chunked] {
        for (name, scope) in &scopes {
            let case = format!("{live:?} {name}");
            let (got, status) = run_groups(explore_sources(&stored, live), &grid, scope, None);
            let want = calc::groups(&stored.oracle, &grid, scope, None);
            assert!(!want.rows.is_empty(), "{case}: the scenario selects rows");
            assert_eq!(got, want, "{case}");
            assert!(status.is_complete(), "{case}: {status:?}");
        }
    }
}

/// ORC-GROUPS and ORC-STATUS past the cap: 520 operations list the 500 with
/// the most rows (ties by name) and fold 20 into `other`, which the section
/// names as `groups_cap`, 20 of 520.
#[test]
fn explore_groups_cap_matches_the_calculator() {
    let mut spans = Vec::new();
    for i in 0..520u64 {
        for copy in 0..(1 + i % 3) {
            let mut trace_id = vec![0u8; 16];
            trace_id[8..].copy_from_slice(&(i * 4 + copy + 1).to_be_bytes());
            let start = T0_S * 1_000_000_000 + (i * 4 + copy) * 10_000_000;
            spans.push(Span {
                trace_id,
                span_id: (i * 4 + copy + 1).to_be_bytes().to_vec(),
                name: format!("op{i:03}"),
                kind: 2,
                start_time_unix_nano: start,
                end_time_unix_nano: start + 1_000_000 + i,
                ..Default::default()
            });
        }
    }
    let requests: Vec<ExportTraceServiceRequest> = spans
        .chunks(100)
        .map(|batch| frame_of(batch.to_vec()))
        .collect();
    let stored = store_requests(&requests, requests.len() / 2);
    let grid = stored.grid;
    for live in [Live::Tail, Live::Split(100)] {
        let (got, status) = run_groups(
            explore_sources(&stored, live),
            &grid,
            &Scope::default(),
            None,
        );
        let want = calc::groups(&stored.oracle, &grid, &Scope::default(), None);
        assert_eq!(want.rows.len(), 500);
        assert_eq!(want.other.as_ref().map(|(folded, _)| *folded), Some(20));
        assert_eq!(got, want, "{live:?}");
        let reasons: Vec<calc::Reason> = stated(&status, PartialReason::GroupsCap, "groups_cap")
            .into_iter()
            .collect();
        assert_eq!(reasons, calc::groups_reasons(&want), "{live:?}");

        // ORC-DELTA past the cap: operations 500 and up last longest; some
        // of them are listed and some folded, so `other` has both sides.
        let slow = 1_000_500;
        let (got, _) = run_groups(
            explore_sources(&stored, live),
            &grid,
            &Scope::default(),
            Some(ExploreSelection {
                filter: sfst::Filter::new(),
                duration: Some(sfst::DurationRange {
                    min_ns: Some(slow),
                    max_ns: None,
                }),
                time_ns: None,
            }),
        );
        let selection = calc::Selection {
            duration: Some((Some(slow), None)),
            ..calc::Selection::default()
        };
        let want = calc::groups(&stored.oracle, &grid, &Scope::default(), Some(&selection));
        let (selected, rest) = want.delta.as_ref().unwrap().other.unwrap();
        assert!(selected.spans > 0 && rest.spans > 0, "{live:?}");
        assert_eq!(got, want, "{live:?}");
    }
}

/// ORC-DELTA (QRY-20, D24): for every way of serving the live WAL, three
/// scopes and five selections, the Groups Δ equals the calculator's: a trace
/// is on the side its window scope rows put it, across both units, and every
/// one of its window rows follows it into its group; trace counts and self
/// time per side are exact.
#[test]
fn explore_delta_matches_the_calculator() {
    let stored = store_resending(300, 71, Some(7));
    let grid = stored.grid;
    let mut units_of: BTreeMap<[u8; 16], BTreeSet<usize>> = BTreeMap::new();
    for span in &stored.oracle {
        if let Some(trace) = span.trace_id {
            units_of.entry(trace).or_default().insert(span.unit);
        }
    }
    let scopes = [
        ("F0 every span", Scope::default()),
        ("F1 entry spans", Scope::entry_spans()),
        (
            "F2 checkout entry spans",
            Scope::entry_spans().with(model::SERVICE_FIELD, &["checkout", "frontend"]),
        ),
    ];
    let mut straddling = 0;
    let mut split = 0;
    for live in [Live::Tail, Live::Chunk, Live::Split(100), Live::Chunked] {
        for (scope_name, scope) in &scopes {
            for (selection_name, engine, oracle) in selections(&grid, &stored.oracle, scope) {
                let case = format!("{live:?} {scope_name} {selection_name}");
                let (got, status) =
                    run_groups(explore_sources(&stored, live), &grid, scope, Some(engine));
                let want = calc::groups(&stored.oracle, &grid, scope, Some(&oracle));
                let delta = want.delta.as_ref().expect("a delta under a selection");
                if delta.selection_traces > 0 && delta.baseline_traces > 0 {
                    split += 1;
                }
                assert_eq!(got, want, "{case}");
                assert!(status.is_complete(), "{case}: {status:?}");
                if matches!(live, Live::Tail) {
                    for span in &stored.oracle {
                        if let Some(trace) = span.trace_id
                            && grid.bucket_of(span.start_ns).is_some()
                            && scope.matches(span)
                            && oracle.matches(span)
                            && units_of[&trace].len() == 2
                        {
                            straddling += 1;
                        }
                    }
                }
            }
        }
    }
    assert!(
        straddling > 0,
        "some selection trace has rows in both units"
    );
    assert!(split > 0, "some case has traces on both sides");
}

/// The engine's compared facets in the calculator's shape; a field the
/// selection is made of carries no comparison at all.
fn engine_comparison(facets: &explore::FacetsData) -> calc::Comparison {
    let totals = facets.comparison.expect("a comparison under a selection");
    let fraction = |diff: explore::ShareDiff| calc::reduced(diff.num, diff.den);
    let mut out = calc::Comparison {
        scope: totals.scope,
        selection: totals.selection,
        fields: Vec::new(),
    };
    for facet in &facets.fields {
        if facet.in_selection {
            assert!(
                facet.comparison.is_none(),
                "{} is not compared",
                facet.field
            );
            assert!(facet.values.iter().all(|v| v.comparison.is_none()));
            out.fields
                .push(calc::ComparedField::InSelection(calc::Facet {
                    field: facet.field.clone(),
                    values: facet
                        .values
                        .iter()
                        .map(|v| (v.value.clone(), v.count))
                        .collect(),
                    omitted_values: facet.omitted_values,
                    omitted_rows: facet.omitted_rows,
                }));
            continue;
        }
        let compared = facet
            .comparison
            .as_ref()
            .expect("every other field compared");
        let mut values = Vec::new();
        for value in &facet.values {
            let c = value.comparison.as_ref().expect("every value compared");
            values.push(calc::ValueComparison {
                value: value.value.clone(),
                count: value.count,
                selection: c.selection,
                baseline: c.baseline,
                eligible: c.eligible,
                rank: c.rank,
                diff: c.diff.map(fraction),
            });
        }
        out.fields
            .push(calc::ComparedField::Compared(calc::FieldComparison {
                field: facet.field.clone(),
                scope: compared.totals.scope,
                selection: compared.totals.selection,
                rank: compared.rank,
                best: compared.best.map(fraction),
                values,
                omitted_values: facet.omitted_values,
                omitted_rows: facet.omitted_rows,
            }));
    }
    out
}

/// The same selection for the engine and the calculator.
fn selections(
    grid: &Grid,
    oracle: &[OracleSpan],
    scope: &Scope,
) -> Vec<(&'static str, ExploreSelection, calc::Selection)> {
    let third = (i64::from(grid.before_s) - i64::from(grid.after_s)) * 1_000_000_000 / 3;
    let start = i64::from(grid.after_s) * 1_000_000_000;
    let middle = (start + third, start + 2 * third);
    let mut durations: Vec<i64> = oracle
        .iter()
        .filter(|span| scope.matches(span) && grid.bucket_of(span.start_ns).is_some())
        .map(|span| span.duration_ns)
        .collect();
    durations.sort_unstable();
    let p95 = durations[(durations.len() * 95).div_ceil(100) - 1];
    let chips = |pairs: &[(&str, &[&str])]| {
        let mut filter = sfst::Filter::new();
        let mut terms = Scope::default();
        for (field, values) in pairs {
            for value in *values {
                filter = filter.select(*field, *value);
            }
            terms = terms.with(field, values);
        }
        (filter, terms)
    };
    let mut out = Vec::new();
    let (filter, terms) = chips(&[(model::STATUS_FIELD, &["error"])]);
    out.push((
        "E1 errors",
        ExploreSelection {
            filter,
            duration: None,
            time_ns: None,
        },
        calc::Selection {
            terms,
            ..calc::Selection::default()
        },
    ));
    let (filter, terms) = chips(&[(model::DURATION_BAND_FIELD, &["100ms-1s", "1-10s"])]);
    out.push((
        "E2 slow bands",
        ExploreSelection {
            filter,
            duration: None,
            time_ns: None,
        },
        calc::Selection {
            terms,
            ..calc::Selection::default()
        },
    ));
    out.push((
        "E3 middle third",
        ExploreSelection {
            filter: sfst::Filter::new(),
            duration: None,
            time_ns: Some(middle.0..middle.1),
        },
        calc::Selection {
            time_ns: Some(middle),
            ..calc::Selection::default()
        },
    ));
    out.push((
        "E4 at least the p95",
        ExploreSelection {
            filter: sfst::Filter::new(),
            duration: Some(sfst::DurationRange {
                min_ns: Some(p95),
                max_ns: None,
            }),
            time_ns: None,
        },
        calc::Selection {
            duration: Some((Some(p95), None)),
            ..calc::Selection::default()
        },
    ));
    out.push((
        "E5 errors or unset",
        ExploreSelection {
            filter: sfst::Filter::new()
                .select(model::STATUS_FIELD, "error")
                .select(model::STATUS_FIELD, "unset"),
            duration: None,
            time_ns: None,
        },
        calc::Selection {
            terms: Scope::default().with(model::STATUS_FIELD, &["error", "unset"]),
            ..calc::Selection::default()
        },
    ));
    out.push((
        "E6 catalog tags or none",
        ExploreSelection {
            filter: sfst::Filter::new()
                .select(TAGS_FIELD, "catalog")
                .select_absent(TAGS_FIELD),
            duration: None,
            time_ns: None,
        },
        calc::Selection {
            terms: Scope::default()
                .with(TAGS_FIELD, &["catalog"])
                .with_absent(TAGS_FIELD),
            ..calc::Selection::default()
        },
    ));
    let (filter, terms) = chips(&[(model::STATUS_FIELD, &["error"])]);
    out.push((
        "E1+E3",
        ExploreSelection {
            filter,
            duration: None,
            time_ns: Some(middle.0..middle.1),
        },
        calc::Selection {
            terms,
            time_ns: Some(middle),
            ..calc::Selection::default()
        },
    ));
    out
}

/// ORC-CMP (QRY-07, QRY-15, QRY-04): for every way of serving the live WAL,
/// four scopes and every selection of [`selections`] (one mixes a value chip
/// with a chip for the rows without the field), the compared facets equal the
/// calculator's exactly — totals, per-value selection and baseline rows,
/// eligibility, exact differences, ranks and the order they give — and the
/// rows follow the selection.
#[test]
fn explore_comparison_matches_the_calculator() {
    let stored = store(400, 61);
    let grid = stored.grid;
    let requested: Vec<String> = [
        model::SERVICE_FIELD,
        "name",
        model::STATUS_FIELD,
        model::DURATION_BAND_FIELD,
        model::ROLE_FIELD,
        TAGS_FIELD,
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let scopes = [
        ("F0 every span", Scope::default()),
        ("F1 entry spans", Scope::entry_spans()),
        (
            "F2 checkout entry spans",
            Scope::entry_spans().with(model::SERVICE_FIELD, &["checkout", "frontend"]),
        ),
        ("F4 text", Scope::default().with_text("redis")),
    ];
    let (mut eligible, mut below, mut in_selection) = (0, 0, 0);
    for live in [Live::Tail, Live::Chunk, Live::Split(100), Live::Chunked] {
        for (scope_name, scope) in &scopes {
            for (selection_name, engine, oracle) in selections(&grid, &stored.oracle, scope) {
                let case = format!("{live:?} {scope_name} {selection_name}");
                let mut query = explore_query(&grid, scope, model::STATUS_FIELD);
                query.sections.histogram = None;
                query.sections.facets = Some(FacetSpec {
                    fields: Some(requested.clone()),
                });
                query.sections.rows = Some(RowsSpec {
                    order: RowOrder::Newest {
                        anchor: None,
                        direction: RowDirection::Older,
                    },
                    limit: 20,
                    columns: Vec::new(),
                });
                query.selection = Some(engine);
                let data = explore::explore(
                    explore_sources(&stored, live),
                    query,
                    explore::ExploreOptions::default(),
                    tokio_util::sync::CancellationToken::new(),
                    std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                )
                .unwrap();
                assert!(data.status.is_complete(), "{case}: {:?}", data.status);
                let want = calc::comparison(&stored.oracle, &grid, scope, &oracle, &requested);
                assert_eq!(engine_comparison(&data.facets.unwrap()), want, "{case}");
                for field in &want.fields {
                    let calc::ComparedField::Compared(field) = field else {
                        in_selection += 1;
                        continue;
                    };
                    for value in &field.values {
                        if value.eligible {
                            eligible += 1;
                        } else if value.selection > 0 {
                            below += 1;
                        }
                    }
                }

                let selected: Vec<OracleSpan> = stored
                    .oracle
                    .iter()
                    .filter(|span| oracle.matches(span))
                    .cloned()
                    .collect();
                let page = calc::newest_page(&selected, &grid, scope, 20, None, calc::Walk::Older);
                let rows = data.rows.unwrap();
                assert_eq!(rows.matched, want.selection, "{case}");
                let keys: Vec<calc::RowKey> =
                    rows.items.iter().map(|row| oracle_key(&row.key)).collect();
                let want_keys: Vec<calc::RowKey> =
                    page.rows.iter().map(|span| calc::row_key(span)).collect();
                assert_eq!(keys, want_keys, "{case}");
            }
        }
    }
    assert!(
        eligible > 0 && below > 0,
        "values on both sides of the minimum support"
    );
    assert!(in_selection > 0, "some selection is made of a listed field");
}

// ── ORC-TRACE: trace-by-id assembles like the calculator ─────────────────

/// The engine's trace-by-id answer as the calculator compares it. Its fields
/// are taken as served: only the calculator's side drops the event, link and
/// error-origin tokens, as the documented answer does.
fn engine_view(data: &sfsq::traces::TraceData) -> TraceView {
    let id = |bytes: &[u8; 8]| (*bytes != [0; 8]).then_some(*bytes);
    let mut view = TraceView {
        spans: Vec::new(),
        self_ns: Vec::new(),
        error_origin: Vec::new(),
        roots: data.trace.roots.clone(),
        children: data.trace.children.clone(),
        summary_root: data.trace.summary_root(),
        truncated: data.status.count(PartialReason::SizeCap).is_some(),
    };
    for (index, span) in data.trace.spans.iter().enumerate() {
        let mut events = Vec::new();
        for event in &span.events {
            let mut attributes = event.attributes.clone();
            attributes.sort();
            events.push(model::SpanEvent {
                time_unix_nano: event.time_unix_nano,
                name: event.name.clone(),
                dropped_attributes_count: event.dropped_attributes_count,
                attributes,
            });
        }
        let mut links = Vec::new();
        for link in &span.links {
            let mut attributes = link.attributes.clone();
            attributes.sort();
            links.push(model::SpanLink {
                trace_id: *link.trace_id.as_bytes(),
                span_id: *link.span_id.as_bytes(),
                trace_state: link.trace_state.clone(),
                flags: link.flags,
                dropped_attributes_count: link.dropped_attributes_count,
                attributes,
            });
        }
        view.spans.push(SpanContent {
            span_id: id(span.span_id.as_bytes()),
            parent_span_id: id(span.parent_span_id.as_bytes()),
            start_ns: span.start_ns,
            duration_ns: span.duration_ns,
            detail: model::SpanDetail {
                kind: span.kind,
                flags: span.flags,
                dropped_attributes_count: span.dropped_attributes_count,
                dropped_events_count: span.dropped_events_count,
                dropped_links_count: span.dropped_links_count,
                events,
                links,
            },
            fields: span.fields.iter().cloned().collect(),
        });
        view.self_ns
            .push(Some(span.duration_ns - data.family.child_ns[index]));
        view.error_origin
            .push(Some(data.family.error_origin[index]));
    }
    view
}

fn trace_answer(sources: &[TraceSource], id: [u8; 16], cap: usize) -> sfsq::traces::TraceData {
    sfsq::traces::trace_by_id(
        sources.to_vec(),
        sfsq::traces::TraceQuery::new(sfst::TraceId::from(id)).span_cap(cap),
        tokio_util::sync::CancellationToken::new(),
        std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    )
    .unwrap()
}

/// ORC-TRACE on the mesh corpus with resent requests, one resend straddling
/// the sealed file and the live WAL: every trace with rows in both units,
/// with a stored span twice, and every tenth, however the live WAL is served.
#[test]
fn trace_by_id_assembles_like_the_calculator() {
    for seed in [11, 12] {
        let stored = store_resending(300, seed, Some(4));
        let mut units: BTreeMap<[u8; 16], BTreeSet<usize>> = BTreeMap::new();
        let mut copies: BTreeMap<([u8; 16], [u8; 8], i32), BTreeSet<usize>> = BTreeMap::new();
        let mut twice: BTreeSet<[u8; 16]> = BTreeSet::new();
        for span in &stored.oracle {
            let (Some(trace), Some(id)) = (span.trace_id, span.span_id) else {
                continue;
            };
            units.entry(trace).or_default().insert(span.unit);
            let key = (trace, id, span.detail.kind);
            if copies.contains_key(&key) {
                twice.insert(trace);
            }
            copies.entry(key).or_default().insert(span.unit);
        }
        let mut judged = BTreeSet::new();
        for (at, (trace, seen)) in units.iter().enumerate() {
            if seen.len() > 1 || twice.contains(trace) || at % 10 == 0 {
                judged.insert(*trace);
            }
        }
        let straddling = copies
            .iter()
            .filter(|(key, seen)| seen.len() > 1 && judged.contains(&key.0))
            .count();
        assert!(
            straddling > 0,
            "seed {seed}: a resent span sits in both units"
        );

        for live in [Live::Tail, Live::Chunk, Live::Chunked] {
            let sources = explore_sources(&stored, live);
            for &trace in &judged {
                let data = trace_answer(&sources, trace, 65_536);
                let case = format!("seed {seed} {live:?} trace {:02x}", trace[0]);
                assert!(data.status.is_complete(), "{case}");
                let want = assembly::assemble_trace(&stored.oracle, trace, 65_536, &|_| true);
                assert_eq!(
                    assembly::trace_diff(&want, &engine_view(&data)),
                    None,
                    "{case}"
                );
            }
        }
    }
}

fn kv_list(
    key: &str,
    pairs: Vec<opentelemetry_proto::tonic::common::v1::KeyValue>,
) -> opentelemetry_proto::tonic::common::v1::KeyValue {
    opentelemetry_proto::tonic::common::v1::KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: pairs,
            })),
        }),
    }
}

/// A span of the edge trace (`0x51`): `id` 0 has no id, `parent` 0 none;
/// times in ms after T0 + 5 s.
fn edge_span(
    id: u8,
    parent: u8,
    kind: i32,
    (start_ms, duration_ms): (u64, u64),
    name: &str,
) -> Span {
    let start = (T0_S + 5) * 1_000_000_000 + start_ms * 1_000_000;
    Span {
        trace_id: vec![0x51; 16],
        span_id: if id == 0 { Vec::new() } else { vec![id; 8] },
        parent_span_id: if parent == 0 {
            Vec::new()
        } else {
            vec![parent; 8]
        },
        name: name.into(),
        kind,
        start_time_unix_nano: start,
        end_time_unix_nano: start + duration_ms * 1_000_000,
        ..Default::default()
    }
}

/// ORC-TRACE on the corner cases: a span whose late copy is sealed and early
/// copy live; an identical copy in both units; a CLIENT and a SERVER sharing an
/// id with an ERROR child; spans without an id tied on start; a self parent, an
/// orphan and a two-span cycle; timed events with nested attributes; links with
/// ids, trace state, flags and drops, one of the wrong length. Every live
/// layout, and caps around the trace's size.
#[test]
fn trace_by_id_edge_cases_match_the_calculator() {
    let mut root = edge_span(1, 0, 2, (0, 1_000), "root");
    root.flags = 257;
    root.dropped_attributes_count = 2;
    root.dropped_events_count = 1;
    root.events = vec![
        Event {
            time_unix_nano: 0,
            name: "boot".into(),
            attributes: vec![kv_list("db", vec![common::kv_int("rows", 3)])],
            dropped_attributes_count: 1,
        },
        Event {
            time_unix_nano: (T0_S + 5) * 1_000_000_000 + 10,
            name: String::new(),
            ..Default::default()
        },
    ];
    root.links = vec![
        Link {
            trace_id: vec![0x61; 16],
            span_id: vec![0x62; 8],
            trace_state: "k=v".into(),
            attributes: vec![common::kv_str("reason", "follows")],
            dropped_attributes_count: 3,
            flags: 1,
        },
        Link {
            trace_id: vec![0x63; 15],
            span_id: vec![0x64; 8],
            ..Default::default()
        },
    ];
    let mut child = edge_span(6, 5, 3, (150, 100), "charge");
    child.status = Some(Status {
        code: 2,
        ..Default::default()
    });
    let requests = vec![
        frame_of(vec![
            root,
            edge_span(2, 1, 3, (300, 100), "late copy"),
            edge_span(3, 1, 1, (400, 10), "same"),
        ]),
        frame_of(vec![
            edge_span(0, 1, 1, (500, 5), "anonymous a"),
            edge_span(0, 1, 1, (500, 5), "anonymous b"),
            edge_span(7, 7, 1, (600, 5), "self parent"),
            edge_span(8, 99, 1, (610, 5), "orphan"),
            edge_span(9, 10, 1, (620, 5), "cycle a"),
            edge_span(10, 9, 1, (630, 5), "cycle b"),
        ]),
        frame_of(vec![
            edge_span(2, 1, 3, (200, 50), "early copy"),
            edge_span(3, 1, 1, (400, 10), "same"),
            edge_span(5, 1, 3, (100, 600), "client"),
            edge_span(5, 1, 2, (120, 400), "server"),
            child,
        ]),
    ];
    let stored = store_requests(&requests, 1);
    let trace = [0x51; 16];
    let size = assembly::assemble_trace(&stored.oracle, trace, usize::MAX, &|_| true)
        .items
        .len();
    assert_eq!(size, 12);

    // Live frames of 6 and 5 spans: chunks of 6 leave the second as the tail.
    for live in [Live::Tail, Live::Chunk, Live::Split(6), Live::Chunked] {
        let sources = explore_sources(&stored, live);
        for cap in [65_536, 1, size - 1, size, size + 1] {
            let data = trace_answer(&sources, trace, cap);
            let want = assembly::assemble_trace(&stored.oracle, trace, cap, &|_| true);
            let case = format!("{live:?} cap {cap}");
            assert_eq!(
                assembly::trace_diff(&want, &engine_view(&data)),
                None,
                "{case}"
            );
        }
    }
}
