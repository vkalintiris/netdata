//! Tier-1 comparison with the reference calculator (`otel-oracle`): a seeded
//! multi-service corpus goes through the plugin's own ingest in-process (flatten
//! → WAL frames → a sealed file, and an unsealed WAL read as a chunk image and
//! as a tail), and what the store answers is compared with the calculator's
//! brute-force numbers over the same spans.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use otel_oracle::calc::{self, Grid, Scope, fixed_histogram};
use otel_oracle::corpus::{self, MeshParams};
use otel_oracle::model::{self, OracleSpan};
use sfsq::Source;
use sfsq::traces::explore::{
    self, ExploreQuery, ExploreScope, FacetSpec, HIDDEN_FIELDS, HistogramSpec, RowDirection,
    RowKey, RowOrder, RowsData, RowsSpec, Sections,
};
use sfsq::traces::{SourceId, TraceSfstCandidate, TraceSource, TraceWalScan, WalCoverage};

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
    oracle: Vec<OracleSpan>,
    grid: Grid,
}

/// The first two thirds of the export requests seal into a file; the rest stay
/// in a WAL that is read both as a chunk image and as a tail.
fn store(traces: usize, seed: u64) -> Stored {
    store_resending(traces, seed, None)
}

/// [`store`], with every `resend_every`-th export request sent twice in a row,
/// as an exporter retrying after a lost acknowledgement would: its rows are
/// stored twice with identical content.
fn store_resending(traces: usize, seed: u64, resend_every: Option<usize>) -> Stored {
    let dir = tempfile::tempdir().unwrap();
    let spans = corpus::generate(&MeshParams {
        traces,
        start_ns: T0_S * 1_000_000_000,
        trace_spacing_ns: TRACE_SPACING_NS,
        seed,
    });
    let mut requests = Vec::new();
    for (i, request) in corpus::build_requests(&spans, 50).into_iter().enumerate() {
        if resend_every.is_some_and(|every| i % every == 0) {
            requests.push(request.clone());
        }
        requests.push(request);
    }
    let cut = requests.len() * 2 / 3;

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

    let last_start_s = oracle.iter().map(|s| s.start_ns).max().unwrap() / 1_000_000_000;
    let grid = Grid::for_window(T0_S as u32, last_start_s as u32 + 1);

    Stored {
        _dir: dir,
        sealed: std::fs::read(&sealed_path).unwrap(),
        live_wal,
        live_chunk,
        oracle,
        grid,
    }
}

fn unit_spans(stored: &Stored, unit: usize) -> Vec<&OracleSpan> {
    stored.oracle.iter().filter(|s| s.unit == unit).collect()
}

/// value → rows carrying it, per field, as the calculator sees a unit.
fn oracle_value_counts(spans: &[&OracleSpan], field: &str) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    for span in spans {
        if let Some(values) = span.fields.get(field) {
            for value in values {
                *counts.entry(value.clone()).or_default() += 1;
            }
        }
    }
    counts
}

fn stored_value_counts(bytes: &[u8], field: &str) -> BTreeMap<String, u64> {
    let reader = sfst::IndexReader::open(bytes).unwrap();
    let all = reader.compile_filter(&sfst::Filter::new(), None).unwrap();
    let facets = reader.facets(&[field], &all, i64::MIN..i64::MAX).unwrap();
    let mut counts = BTreeMap::new();
    for facet in facets {
        for (value, count) in facet.values {
            counts.insert(value, u64::from(count));
        }
    }
    counts
}

/// ORC-TOKENS: every unit stores exactly the calculator's rows and, per core
/// field, the same number of rows per value; `_role` is on every row (FLAT-15).
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
            for field in CORE_FIELDS {
                assert_eq!(
                    stored_value_counts(bytes, field),
                    oracle_value_counts(&spans, field),
                    "seed {seed} unit {unit} field {field}"
                );
            }
            let roles: u64 = stored_value_counts(bytes, model::ROLE_FIELD).values().sum();
            assert_eq!(roles as usize, spans.len(), "one role per row");
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
                core.insert(field.to_string(), values.clone());
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
        .map(|b| b.counts.get("ERROR").copied().unwrap_or(0))
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
        Live::Split(min_entries) => {
            let header = wal::HEADER_SIZE as u64;
            let frames = wal::scan_frame_boundaries(&stored.live_wal, whole).unwrap();
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
    for (field, values) in &scope.terms {
        for value in values {
            filter = filter.select(field.clone(), value.clone());
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
        },
        sections: Sections {
            histogram: Some(HistogramSpec {
                stack: stack.to_string(),
                percentiles: true,
            }),
            facets: None,
            rows: None,
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
                    tokio_util::sync::CancellationToken::new(),
                    std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                )
                .unwrap();
                let case = format!("{live:?} {scope_name} stack {stack}");
                assert!(data.status.is_complete(), "{case}: {:?}", data.status);
                let histogram = data.histogram.expect("histogram section");

                let mut got = Vec::with_capacity(histogram.buckets.len());
                for bucket in &histogram.buckets {
                    assert_eq!(bucket.other, 0, "{case}");
                    let mut counts = BTreeMap::new();
                    for (value, count) in histogram.dimensions.iter().zip(&bucket.counts) {
                        if *count > 0 {
                            counts.insert(value.clone(), *count);
                        }
                    }
                    got.push(calc::Bucket {
                        counts,
                        unset: bucket.unset,
                    });
                }
                assert_eq!(
                    got,
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
                    within_bound(got, fixed_histogram::exact_percentiles(durations), &case);
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
                within_bound(
                    window,
                    fixed_histogram::exact_percentiles(&window_durations),
                    &case,
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

fn within_bound(approximate: Option<[i64; 3]>, exact: Option<[i64; 3]>, case: &str) {
    assert_eq!(approximate.is_some(), exact.is_some(), "{case}");
    let (Some(approximate), Some(exact)) = (approximate, exact) else {
        return;
    };
    for (a, e) in approximate.into_iter().zip(exact) {
        if e == 0 {
            assert_eq!(a, 0, "{case}");
        } else {
            let error = (a - e).abs() as f64 / e as f64;
            assert!(
                error <= fixed_histogram::MAX_RELATIVE_ERROR,
                "{case}: {a} vs {e}"
            );
        }
    }
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
        .flat_map(|span| span.fields.keys().cloned())
        .filter(|field| !HIDDEN_FIELDS.contains(&field.as_str()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    fields.retain(|field| !field.starts_with("events.") && !field.starts_with("links."));
    let scopes = [
        ("entry spans", Scope::entry_spans()),
        (
            "checkout entry spans",
            Scope::entry_spans().with(model::SERVICE_FIELD, &["checkout", "frontend"]),
        ),
    ];
    for live in [Live::Tail, Live::Split(100)] {
        for (scope_name, scope) in &scopes {
            let mut query = explore_query(&grid, scope, model::STATUS_FIELD);
            query.sections.histogram = None;
            query.sections.facets = Some(FacetSpec {
                fields: Some(fields.clone()),
            });
            let data = explore::explore(
                explore_sources(&stored, live),
                query,
                tokio_util::sync::CancellationToken::new(),
                std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            )
            .unwrap();
            let case = format!("{live:?} {scope_name}");
            assert!(data.status.is_complete(), "{case}: {:?}", data.status);
            let facets = data.facets.expect("facets section");
            assert!(facets.unavailable.is_empty(), "{case}");
            assert_eq!(facets.fields.len(), fields.len(), "{case}");
            for facet in &facets.fields {
                let got: BTreeMap<String, u64> = facet
                    .values
                    .iter()
                    .map(|v| (v.value.clone(), v.count))
                    .collect();
                assert_eq!(
                    got,
                    calc::facet_counts(&stored.oracle, &grid, scope, &facet.field),
                    "{case} field {}",
                    facet.field
                );
            }
        }
    }

    let mut query = explore_query(&grid, &Scope::entry_spans(), model::STATUS_FIELD);
    query.sections.facets = Some(FacetSpec { fields: None });
    let data = explore::explore(
        explore_sources(&stored, Live::Tail),
        query,
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
    let grid = stored.grid;
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
            let data = explore::explore(
                explore_sources(&stored, live),
                explore_query(&grid, scope, model::STATUS_FIELD),
                tokio_util::sync::CancellationToken::new(),
                std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            )
            .unwrap();
            let case = format!("{live:?} {name}");
            let histogram = data.histogram.expect("histogram");
            let totals = calc::totals(&stored.oracle, &grid, scope);
            assert!(totals.spans > 0, "{case}: the scenario selects rows");
            assert_eq!(
                (histogram.totals.count, histogram.totals.errors),
                (totals.spans, totals.errors),
                "{case}"
            );
            let want = calc::histogram(&stored.oracle, &grid, scope, model::STATUS_FIELD);
            let mut got = Vec::new();
            for bucket in &histogram.buckets {
                let mut counts = BTreeMap::new();
                for (value, count) in histogram.dimensions.iter().zip(&bucket.counts) {
                    if *count > 0 {
                        counts.insert(value.clone(), *count);
                    }
                }
                got.push(calc::Bucket {
                    counts,
                    unset: bucket.unset,
                });
            }
            assert_eq!(got, want, "{case}");
        }
    }
}

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

/// Every row of the page equals the calculator's row: key, duration, the
/// shown fields and the asked columns.
fn assert_rows_match(got: &RowsData, want: &[&OracleSpan], case: &str) {
    assert_eq!(got.items.len(), want.len(), "{case}");
    for (row, span) in got.items.iter().zip(want) {
        assert_eq!(oracle_key(&row.key), calc::row_key(span), "{case}");
        assert_eq!(row.duration_ns, span.duration_ns, "{case}");
        let first = |field: &str| {
            span.fields
                .get(field)
                .and_then(|values| values.iter().next().cloned())
        };
        assert_eq!(row.service, first(model::SERVICE_FIELD), "{case}");
        assert_eq!(row.name, first("name"), "{case}");
        assert_eq!(row.role, first(model::ROLE_FIELD), "{case}");
        assert_eq!(row.status, first(model::STATUS_FIELD), "{case}");
        let mut columns = Vec::new();
        for column in ROW_COLUMNS {
            let values: Vec<String> = match span.fields.get(column) {
                Some(values) => values.iter().cloned().collect(),
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
    let scopes = [
        ("F0 every span", Scope::default()),
        ("F1 entry spans", Scope::entry_spans()),
    ];
    for live in [Live::Tail, Live::Split(120)] {
        for (name, scope) in &scopes {
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
            assert!(
                grown > 0,
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
    let servings = [Live::Tail, Live::Split(120), Live::Chunk];
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
    for live in [Live::Tail, Live::Split(60)] {
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
