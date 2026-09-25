//! Tier-1 comparison with the reference calculator (`otel-oracle`): a seeded
//! multi-service corpus goes through the plugin's own ingest in-process (flatten
//! → WAL frames → a sealed file, and an unsealed WAL read as a chunk image and
//! as a tail), and what the store answers is compared with the calculator's
//! brute-force numbers over the same spans.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use otel_oracle::calc::{self, Grid, Scope};
use otel_oracle::corpus::{self, MeshParams};
use otel_oracle::model::{self, OracleSpan};
use sfsq::traces::TraceWalScan;

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
    let dir = tempfile::tempdir().unwrap();
    let spans = corpus::generate(&MeshParams {
        traces,
        start_ns: T0_S * 1_000_000_000,
        trace_spacing_ns: TRACE_SPACING_NS,
        seed,
    });
    let requests = corpus::build_requests(&spans, 50);
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
