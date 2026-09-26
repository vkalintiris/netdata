//! Oracle for the traces seal + trace-by-id read (SOW-20260630 Step 4).
//!
//! Drives the real pipeline — `ng-ingest::write_trace_request` (flatten + fill
//! hashes + WAL) → `ng_index::build_sfst_traces_file` (seal) →
//! `sfst::IndexReader::trace_by_id` (lookup + tree). Two layers, per DECISION 13:
//!  1. hand-built fixtures pin the tree-build edge cases (missing parents, multiple
//!     roots, duplicate span ids, clock skew, large fan-out) — these can't be
//!     triggered reliably in real data;
//!  2. a `#[ignore]`d self-consistency check runs against the re-captured real WAL:
//!     independently decode it, then assert every trace reconstructs to exactly its
//!     decoded span-id set.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use file_registry::{ByteSize, MonotonicClock, TimestampNs};
use ng_index::{Metrics, build_sfst_traces_file};
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{
    AnyValue, ArrayValue, KeyValue, KeyValueList, any_value::Value as Av,
};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use sfst::{IndexReader, SpanId, TraceId};

fn kv(k: &str, v: &str) -> KeyValue {
    KeyValue {
        key: k.into(),
        value: Some(AnyValue {
            value: Some(Av::StringValue(v.into())),
        }),
    }
}

/// A key with an arbitrary (possibly nested) OTLP value.
fn kv_any(k: &str, v: Av) -> KeyValue {
    KeyValue {
        key: k.into(),
        value: Some(AnyValue { value: Some(v) }),
    }
}

fn span(trace: [u8; 16], id: [u8; 8], parent: [u8; 8], start: u64, end: u64, name: &str) -> Span {
    Span {
        trace_id: trace.to_vec(),
        span_id: id.to_vec(),
        parent_span_id: parent.to_vec(),
        start_time_unix_nano: start,
        end_time_unix_nano: end,
        name: name.into(),
        ..Default::default()
    }
}

/// Wrap spans in a single-resource/single-scope export request.
fn req(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: Some(Resource {
                attributes: vec![kv("service.name", "svc")],
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

fn count_spans(req: &ExportTraceServiceRequest) -> usize {
    req.resource_spans
        .iter()
        .flat_map(|rs| rs.scope_spans.iter())
        .map(|ss| ss.spans.len())
        .sum()
}

/// Ingest the requests into a traces WAL, then seal it into an SFST and return the
/// sealed bytes. Mirrors `ng-ingest::write_trace_request` inline (normalize → flatten
/// → emit-time hashes → encode → WAL frame), keeping the test self-contained; the
/// real `write_trace_request` is exercised end-to-end by the `#[ignore]`d real-WAL
/// oracle below.
fn seal(reqs: Vec<ExportTraceServiceRequest>) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let wal_path = write_wal(dir.path(), reqs);
    let out = dir.path().join("traces.sfst");
    build_sfst_traces_file(&wal_path, &out, &Metrics::new()).unwrap();
    std::fs::read(&out).unwrap()
}

/// Ingest the requests into a traces WAL in `dir` and return its path.
fn write_wal(dir: &std::path::Path, reqs: Vec<ExportTraceServiceRequest>) -> std::path::PathBuf {
    let mut clock = MonotonicClock::new();
    let mut frames = Vec::new();
    for mut r in reqs {
        let count = count_spans(&r);
        if count == 0 {
            continue;
        }
        let base = clock.now_ns().as_u64();
        ng_flatten::normalize_trace_request(&mut r, base, None);
        let (flat, _) = ng_flatten::flatten_trace_request(r);
        frames.push((ng_flatten::encode_trace_frame(&flat).unwrap(), count));
    }
    write_wal_frames(dir, frames)
}

/// Writes encoded traces frames, each with its span count, into a traces WAL
/// in `dir` and returns its path.
fn write_wal_frames(dir: &std::path::Path, frames: Vec<(Vec<u8>, usize)>) -> std::path::PathBuf {
    let seq = Arc::new(wal::SeqAllocator::ephemeral(0));
    let config = wal::Config {
        rotation: wal::RotationConfig {
            max_entries: usize::MAX,
            max_file_size: ByteSize(u64::MAX),
            max_duration: None,
        },
        crc_enabled: true,
        compression_enabled: true,
    };
    let mut writer = wal::Writer::new(
        dir,
        config,
        seq,
        wal::FileStamp { pipeline_id: 1, payload_format: /* traces pipeline */
        ng_flatten::TRACE_FRAME_PAYLOAD_FORMAT },
        wal::test_identity(),
    )
    .unwrap();
    let mut clock = MonotonicClock::new();
    for (data, count) in frames {
        let ingestion_ns = clock.now_ns();
        // The production content_meta: the version-tagged empty-ServiceStream
        // blob the unattributed stream carries (not a bare empty slice), so
        // the seal is exercised against production-shaped WAL headers.
        let content_meta =
            otel_logs_identity::encode_content_meta(&otel_logs_identity::ServiceStream::new(
                "", "",
            ))
            .expect("the empty identity always encodes");
        writer
            .write_frame(
                0,
                &content_meta,
                &data,
                wal::FrameMeta {
                    entry_count: count,
                    ingestion_ns,
                    log_ts_range: None,
                },
            )
            .unwrap();
    }
    writer.shutdown_all().unwrap();

    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "wal"))
        .expect("a wal file was written")
}

/// The seal and the chunk image come from the same populate function, so they
/// pin the same fields: 1,500 distinct span names keep `name` Mid (its facets
/// and charts work) in both, while an unpinned field that many values is High.
/// FLAT-09: every sealed row's `_duration_band` is the band of the duration the
/// file stores for it: an unset end and an end before the start store 0, an
/// overlong span saturates, and the edges fall on their band's first value.
#[test]
fn every_row_s_band_is_the_band_of_its_stored_duration() {
    let t = [7u8; 16];
    let base: u64 = 1_700_000_000_000_000_000;
    let at = |id: u8, offset: u64, length: Option<u64>, name: &str| {
        let start = base + offset;
        let end = length.map_or(0, |length| start + length);
        span(t, [id; 8], [1; 8], start, end, name)
    };
    let spans = vec![
        span(t, [1; 8], [0; 8], base, base + 20_000_000_000, "root"),
        at(2, 10, None, "unset end"),
        span(t, [3; 8], [1; 8], base + 20, base + 5, "end before start"),
        at(4, 30, Some(999_999), "just under 1ms"),
        at(5, 40, Some(1_000_000), "1ms"),
        at(6, 50, Some(9_999_999_999), "just under 10s"),
        at(8, 60, Some(10_000_000_000), "10s"),
        span(t, [9; 8], [1; 8], 1, u64::MAX, "overlong"),
    ];
    let bytes = seal(vec![req(spans)]);
    let reader = IndexReader::open(&bytes).unwrap();
    let rows = reader.summary().record_count;
    assert_eq!(rows, 8);
    let durations = reader.durations().unwrap();
    let bands = reader
        .row_values(ng_flatten::DURATION_BAND_FIELD, 0..rows)
        .unwrap();
    let mut seen = std::collections::BTreeMap::new();
    for position in 0..rows {
        let stored = durations.0[position as usize];
        let label = bands
            .value_at(position)
            .map(|at| bands.values[at as usize].as_str());
        let want = ng_flatten::DURATION_BAND_LABELS[ng_flatten::duration_band(stored)];
        assert_eq!(label, Some(want), "row {position}, duration {stored}");
        *seen.entry(stored).or_insert(0) += 1;
    }
    assert_eq!(
        seen.get(&0),
        Some(&2),
        "the unset and the earlier end store 0"
    );
    assert_eq!(seen.get(&i64::MAX), Some(&1), "the overlong span saturates");
}

#[test]
fn chunk_image_and_seal_pin_the_same_fields() {
    let spans = (0..1_500u32)
        .map(|i| {
            let mut trace = [0u8; 16];
            trace[..4].copy_from_slice(&i.to_be_bytes());
            trace[15] = 1;
            let mut id = [0u8; 8];
            id[..4].copy_from_slice(&i.to_be_bytes());
            id[7] = 1;
            let start = 1_700_000_000_000_000_000 + u64::from(i) * 1_000_000;
            let mut span = span(trace, id, [0; 8], start, start + 1_000, &format!("op-{i}"));
            span.attributes = vec![kv("request.id", &format!("r-{i}"))];
            span
        })
        .collect();
    let dir = tempfile::tempdir().unwrap();
    let wal_path = write_wal(dir.path(), vec![req(spans)]);
    let out = dir.path().join("traces.sfst");
    build_sfst_traces_file(&wal_path, &out, &Metrics::new()).unwrap();
    let sealed = std::fs::read(&out).unwrap();
    let file_len = std::fs::metadata(&wal_path).unwrap().len();
    let (_, image) = ng_index::build_sfst_traces_range(
        &wal_path,
        wal::FrameRange::new(wal::HEADER_SIZE as u64, file_len),
    )
    .unwrap();

    let table = |bytes: &[u8]| {
        let reader = IndexReader::open(bytes).unwrap();
        let mut out = Vec::new();
        for entry in reader.field_table().iter() {
            out.push((entry.name.clone(), entry.cardinality, entry.tier));
        }
        out
    };
    let sealed_table = table(&sealed);
    assert_eq!(sealed_table, table(&image));
    let tier = |name: &str| {
        sealed_table
            .iter()
            .find(|(field, _, _)| field == name)
            .map(|(_, cardinality, tier)| (*cardinality, *tier))
    };
    assert_eq!(tier("name"), Some((1_500, sfst::FieldTier::Mid)));
    assert_eq!(
        tier("attributes.request.id"),
        Some((1_500, sfst::FieldTier::High))
    );
}

#[test]
fn traces_build_refuses_logs_payload_format() {
    // The TRACES build must refuse a WAL stamped with the logs frame codec —
    // the cross-signal mixup the per-file format tag exists to catch.
    let dir = tempfile::tempdir().unwrap();
    let seq = Arc::new(wal::SeqAllocator::ephemeral(0));
    let mut writer = wal::Writer::new(
        dir.path(),
        wal::Config::default(),
        seq,
        wal::FileStamp {
            pipeline_id: 1,
            payload_format: ng_flatten::LOG_FRAME_PAYLOAD_FORMAT,
        },
        wal::test_identity(),
    )
    .unwrap();
    writer
        .write_frame(
            0,
            &[],
            b"x",
            wal::FrameMeta {
                entry_count: 1,
                ingestion_ns: TimestampNs(1),
                log_ts_range: None,
            },
        )
        .unwrap();
    writer.shutdown_all().unwrap();
    let wal_path = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "wal"))
        .unwrap();
    match build_sfst_traces_file(&wal_path, &dir.path().join("t.sfst"), &Metrics::new()) {
        Err(ng_index::Error::PayloadFormat { found, expected }) => {
            assert_eq!(found, ng_flatten::LOG_FRAME_PAYLOAD_FORMAT);
            assert_eq!(expected, ng_flatten::TRACE_FRAME_PAYLOAD_FORMAT);
        }
        other => panic!("expected PayloadFormat rejection, got {other:?}"),
    }
}

const ROOT_PARENT: [u8; 8] = [0u8; 8]; // unset parent = root

#[test]
fn trace_by_id_builds_linear_tree() {
    let t = [0xA1u8; 16];
    let (root, child, grand) = ([1u8; 8], [2u8; 8], [3u8; 8]);
    let bytes = seal(vec![req(vec![
        span(t, root, ROOT_PARENT, 100, 200, "root"),
        span(t, child, root, 110, 180, "child"),
        span(t, grand, child, 120, 160, "grand"),
    ])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();

    assert_eq!(tr.spans.len(), 3);
    assert_eq!(tr.roots.len(), 1);
    // Sorted by start time: root(100), child(110), grand(120).
    assert_eq!(tr.spans[0].span_id, SpanId::from(root));
    assert_eq!(tr.spans[tr.roots[0]].span_id, SpanId::from(root));
    let kids = |sid: [u8; 8]| {
        let idx = tr
            .spans
            .iter()
            .position(|s| s.span_id == SpanId::from(sid))
            .expect("span present");
        tr.children[idx].clone()
    };
    assert_eq!(kids(root).len(), 1);
    assert_eq!(tr.spans[kids(root)[0]].span_id, SpanId::from(child));
    assert_eq!(tr.spans[kids(child)[0]].span_id, SpanId::from(grand));
    assert!(kids(grand).is_empty());
    // The `name` facet materialized onto the span.
    assert_eq!(
        tr.spans[tr.roots[0]]
            .fields
            .iter()
            .find(|(k, _)| k == "name")
            .map(|(_, v)| v.as_str()),
        Some("root"),
    );
}

#[test]
fn trace_by_id_collapses_duplicate_span_ids() {
    // A resent span (same trace_id + span_id) must collapse to one node.
    let t = [0xB2u8; 16];
    let (root, dup) = ([1u8; 8], [2u8; 8]);
    let bytes = seal(vec![req(vec![
        span(t, root, ROOT_PARENT, 100, 200, "root"),
        span(t, dup, root, 110, 150, "a"),
        span(t, dup, root, 110, 150, "a"),
    ])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans.len(), 2, "duplicate (trace_id, span_id) collapsed");
    let root_idx = tr
        .spans
        .iter()
        .position(|s| s.span_id == SpanId::from(root))
        .unwrap();
    assert_eq!(tr.children[root_idx].len(), 1);
}

#[test]
fn trace_by_id_forms_a_forest_from_orphans_and_multiple_roots() {
    // Two explicit roots (unset parent) + one orphan (parent absent from the file).
    let t = [0xC3u8; 16];
    let (r1, r2, orphan, missing) = ([1u8; 8], [2u8; 8], [3u8; 8], [9u8; 8]);
    let bytes = seal(vec![req(vec![
        span(t, r1, ROOT_PARENT, 100, 200, "root1"),
        span(t, r2, ROOT_PARENT, 105, 150, "root2"),
        span(t, orphan, missing, 110, 140, "orphan"),
    ])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans.len(), 3);
    assert_eq!(
        tr.roots.len(),
        3,
        "two unset-parent roots + one orphan-as-root"
    );
    assert!(
        tr.children.iter().all(|kids| kids.is_empty()),
        "no edges: no in-file parent has children"
    );
}

#[test]
fn trace_by_id_handles_clock_skew() {
    // Child starts before its parent (skew): sorted order puts the child first, but
    // the parent/child edge must still be built from the ids.
    let t = [0xD4u8; 16];
    let (root, child) = ([1u8; 8], [2u8; 8]);
    let bytes = seal(vec![req(vec![
        span(t, root, ROOT_PARENT, 200, 300, "root"),
        span(t, child, root, 100, 150, "child"),
    ])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(
        tr.spans[0].span_id,
        SpanId::from(child),
        "earliest start sorts first"
    );
    assert_eq!(tr.roots.len(), 1);
    assert_eq!(tr.spans[tr.roots[0]].span_id, SpanId::from(root));
    let root_idx = tr
        .spans
        .iter()
        .position(|s| s.span_id == SpanId::from(root))
        .unwrap();
    let kids = &tr.children[root_idx];
    assert_eq!(tr.spans[kids[0]].span_id, SpanId::from(child));
}

#[test]
fn trace_by_id_handles_large_fan_out() {
    // One root with 200 direct children — the iterative build must handle wide
    // fan-out without recursion.
    let t = [0xE5u8; 16];
    let root = [1u8; 8];
    let mut spans = vec![span(t, root, ROOT_PARENT, 100, 999, "root")];
    for i in 0..200u64 {
        let sid = (i + 2).to_be_bytes(); // unique, never all-zero, never == root
        spans.push(span(t, sid, root, 100 + i, 200, "leaf"));
    }
    let bytes = seal(vec![req(spans)]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans.len(), 201);
    assert_eq!(tr.roots.len(), 1);
    let root_idx = tr
        .spans
        .iter()
        .position(|s| s.span_id == SpanId::from(root))
        .unwrap();
    assert_eq!(tr.children[root_idx].len(), 200);
}

#[test]
fn trace_by_id_cycle_surfaces_all_spans_under_a_root() {
    // Pathological parent cycle A<->B: neither has an unset/absent parent, so there
    // is no natural root. The guard must still surface a root (the earliest span) so
    // no span is lost / unreachable.
    let t = [0x7cu8; 16];
    let (a, b) = ([1u8; 8], [2u8; 8]);
    let bytes = seal(vec![req(vec![
        span(t, a, b, 100, 200, "a"), // a's parent is b
        span(t, b, a, 110, 190, "b"), // b's parent is a
    ])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans.len(), 2);
    assert_eq!(
        tr.roots.len(),
        1,
        "cycle guard promotes the earliest span as a root"
    );
    assert_eq!(
        tr.spans[tr.roots[0]].span_id,
        SpanId::from(a),
        "earliest (start 100) is the root"
    );
}

#[test]
fn trace_by_id_keeps_distinct_unset_span_ids() {
    // Two spans that both lack a span_id (unset) are distinct spans, not a resend —
    // they must NOT be collapsed by the span-id dedup.
    let t = [0xafu8; 16];
    let mk = |name: &str, start: u64| Span {
        trace_id: t.to_vec(),
        span_id: vec![],        // unset
        parent_span_id: vec![], // root
        start_time_unix_nano: start,
        end_time_unix_nano: start + 10,
        name: name.into(),
        ..Default::default()
    };
    let bytes = seal(vec![req(vec![mk("a", 100), mk("b", 110)])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(
        tr.spans.len(),
        2,
        "distinct unset-span-id spans are not collapsed"
    );
    assert!(tr.spans.iter().all(|s| s.span_id == SpanId::UNSET));
    assert_eq!(tr.roots.len(), 2, "both are roots (unset parent)");
}

#[test]
fn trace_by_id_reaches_all_spans_despite_a_cyclic_component() {
    // A valid rooted pair (R->C) coexisting with a disjoint parent cycle (X<->Y).
    // `roots` is non-empty (R), so a naive "promote only when roots empty" guard
    // would leave X,Y unreachable. The reachability guard must surface them.
    let t = [0x5bu8; 16];
    let (r, c, x, y) = ([1u8; 8], [2u8; 8], [3u8; 8], [4u8; 8]);
    let bytes = seal(vec![req(vec![
        span(t, r, ROOT_PARENT, 100, 200, "root"),
        span(t, c, r, 110, 150, "child"),
        span(t, x, y, 120, 160, "x"), // x's parent is y
        span(t, y, x, 130, 170, "y"), // y's parent is x (cycle)
    ])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans.len(), 4);
    // A revisit-guarded walk from the roots must reach every span.
    let mut seen: HashSet<usize> = HashSet::new();
    let mut stack: Vec<usize> = tr.roots.clone();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        stack.extend(tr.children[i].iter().copied());
    }
    assert_eq!(seen.len(), 4, "every span reachable from a root");
}

#[test]
fn trace_by_id_self_parent_is_a_root() {
    // A span that is its own parent must be a root (not a self-child), and carry no
    // self-edge in `children`.
    let t = [0x9eu8; 16];
    let s = [1u8; 8];
    let bytes = seal(vec![req(vec![span(t, s, s, 100, 200, "self")])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans.len(), 1);
    assert_eq!(tr.roots.len(), 1, "self-parent treated as root");
    assert!(tr.children.iter().all(|kids| kids.is_empty()), "no self-edge");
}

#[test]
fn trace_by_id_surfaces_flags_and_dropped_count() {
    // The per-row scalars (flags, dropped_attributes_count) reconstruct onto the span.
    let t = [0x8du8; 16];
    let mut s = span(t, [1u8; 8], ROOT_PARENT, 100, 200, "x");
    s.flags = 0x1;
    s.dropped_attributes_count = 3;
    let bytes = seal(vec![req(vec![s])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from(t))
        .unwrap();
    assert_eq!(tr.spans[0].flags, 0x1);
    assert_eq!(tr.spans[0].dropped_attributes_count, 3);
}

#[test]
fn trace_by_id_absent_is_empty() {
    let t = [0xF6u8; 16];
    let bytes = seal(vec![req(vec![span(
        t,
        [1u8; 8],
        ROOT_PARENT,
        100,
        200,
        "x",
    )])]);
    let tr = IndexReader::open(&bytes)
        .unwrap()
        .trace_by_id(TraceId::from([0x11u8; 16]))
        .unwrap();
    assert!(tr.spans.is_empty() && tr.roots.is_empty() && tr.children.is_empty());
}

#[test]
fn trace_by_id_separates_interleaved_traces() {
    // Two traces interleaved across one batch: each reconstructs only its own spans.
    let (ta, tb) = ([0x1au8; 16], [0x2bu8; 16]);
    let bytes = seal(vec![req(vec![
        span(ta, [1u8; 8], ROOT_PARENT, 100, 200, "a-root"),
        span(tb, [1u8; 8], ROOT_PARENT, 105, 210, "b-root"),
        span(ta, [2u8; 8], [1u8; 8], 110, 180, "a-child"),
    ])]);
    let reader = IndexReader::open(&bytes).unwrap();
    let a = reader.trace_by_id(TraceId::from(ta)).unwrap();
    let b = reader.trace_by_id(TraceId::from(tb)).unwrap();
    assert_eq!(a.spans.len(), 2);
    assert_eq!(b.spans.len(), 1);
}

/// Self-consistency oracle against the re-captured real traces WAL (DECISION 13).
/// Ignored by default (CI has no WAL); run with:
///   `cargo test -p ng-index --test traces_seal -- --ignored`
/// after re-capturing with `ng-ingest-traces`.
#[test]
#[ignore = "requires the re-captured traces WAL under $HOME/repos/tmp/ng"]
fn oracle_real_wal_self_consistency() {
    let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join("repos/tmp/ng");
    let wal_path = std::fs::read_dir(&dir)
        .expect("~/repos/tmp/ng exists")
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|x| x == "wal"))
        .expect("a traces wal file under ~/repos/tmp/ng");

    // Ground truth: independently decode the WAL → trace_id → set of span_ids.
    // Unset (all-zero) trace ids are not indexed, so they are excluded on both sides.
    let mut truth: HashMap<[u8; 16], HashSet<[u8; 8]>> = HashMap::new();
    let mut reader = wal::Reader::open(&wal_path).unwrap();
    while let Some(frame) = reader.next_frame().unwrap() {
        let flat = ng_flatten::decode_trace_frame(frame.data).unwrap();
        for rg in &flat.resources {
            for sg in &rg.scopes {
                for span in &sg.spans {
                    let tid = *span.trace_id.as_bytes();
                    if tid == [0u8; 16] {
                        continue;
                    }
                    truth
                        .entry(tid)
                        .or_default()
                        .insert(*span.span_id.as_bytes());
                }
            }
        }
    }
    assert!(!truth.is_empty(), "the WAL had no trace-bearing spans");

    // Seal + reopen.
    let out = tempfile::tempdir().unwrap();
    let sfst_path = out.path().join("traces.sfst");
    let (summary, _size) = build_sfst_traces_file(&wal_path, &sfst_path, &Metrics::new()).unwrap();
    let bytes = std::fs::read(&sfst_path).unwrap();
    let index = IndexReader::open(&bytes).unwrap();

    // Check a bounded, deterministic sample of traces: `trace_by_id` rebuilds the
    // reverse string table per call, so checking every distinct id would be far too
    // slow at 500K. A sorted sample of a few hundred still surfaces any systemic
    // seal/index/materialize bug (they'd fail uniformly, not per-trace).
    const SAMPLE: usize = 500;
    let mut ids: Vec<[u8; 16]> = truth.keys().copied().collect();
    ids.sort_unstable();
    let checked = ids.len().min(SAMPLE);
    for tid in &ids[..checked] {
        let tr = index.trace_by_id(TraceId::from(*tid)).unwrap();
        let got: HashSet<[u8; 8]> = tr.spans.iter().map(|s| *s.span_id.as_bytes()).collect();
        assert_eq!(
            &got,
            &truth[tid],
            "trace {} span-set mismatch",
            TraceId::from(*tid)
        );
    }
    println!(
        "oracle OK: {} spans, {} distinct traces; {} sampled traces reconstructed consistently",
        summary.record_count,
        truth.len(),
        checked,
    );
}

/// Round-trip oracle for the lossless fields (events, links,
/// trace_state, status_message, dropped counts): OTLP → WAL frame → seal →
/// `trace_by_id` returns every field, with per-event grouping/order intact,
/// while the flat `events.`/`links.` search tokens stay out of the span's
/// facet list (they are represented structurally).
#[test]
fn events_links_and_deferred_scalars_round_trip() {
    use opentelemetry_proto::tonic::trace::v1::Status;
    use opentelemetry_proto::tonic::trace::v1::span::{Event, Link};

    let trace = [0xE1u8; 16];
    let mut root = span(trace, [1; 8], [0; 8], 2_000, 2_500, "root");
    root.trace_state = "ot=th:8".into();
    root.status = Some(Status {
        code: 2,
        message: "disk full".into(),
    });
    root.dropped_events_count = 4;
    root.dropped_links_count = 1;
    root.events = vec![
        Event {
            time_unix_nano: 2_100,
            name: "exception".into(),
            attributes: vec![
                kv("exception.type", "IOError"),
                kv("exception.stacktrace", "at main()\n  at run()"),
            ],
            dropped_attributes_count: 7,
        },
        Event {
            time_unix_nano: 2_200,
            name: "retry".into(),
            attributes: vec![
                kv("policy", "backoff"),
                // Nested containers: platform fidelity = leaf tokens under the
                // collapsed paths (kvlist keys dotted, array elems as `[]`).
                kv_any(
                    "ctx",
                    Av::KvlistValue(KeyValueList {
                        values: vec![
                            kv("user", "u1"),
                            kv_any(
                                "ids",
                                Av::ArrayValue(ArrayValue {
                                    values: vec![
                                        AnyValue {
                                            value: Some(Av::IntValue(1)),
                                        },
                                        AnyValue {
                                            value: Some(Av::IntValue(2)),
                                        },
                                    ],
                                }),
                            ),
                        ],
                    }),
                ),
            ],
            dropped_attributes_count: 0,
        },
    ];
    root.links = vec![Link {
        trace_id: vec![0xB2; 16],
        span_id: vec![0xB3; 8],
        trace_state: "vendor=x".into(),
        attributes: vec![
            kv("messaging.operation", "publish"),
            kv_any(
                "route",
                Av::KvlistValue(KeyValueList {
                    values: vec![kv("queue", "q1")],
                }),
            ),
        ],
        dropped_attributes_count: 2,
        flags: 0x300,
    }];
    // A SPAN attribute deliberately named like the reserved event namespace:
    // it flattens under `attributes.events.name`, so the reader's `events.`
    // facet filter must NOT drop it.
    root.attributes.push(kv("events.name", "decoy"));
    // A second span, chronologically EARLIER but ingested after: the seal's
    // time remap reorders rows, and each span's events must stay its own.
    let mut early = span(trace, [2; 8], [1; 8], 1_000, 1_400, "child");
    early.events = vec![Event {
        time_unix_nano: 1_100,
        name: "cache-miss".into(),
        attributes: vec![],
        dropped_attributes_count: 0,
    }];

    let bytes = seal(vec![req(vec![root, early])]);
    let index = IndexReader::open(&bytes).unwrap();
    assert!(index.has_event_index(), "EVNB present");
    assert!(index.has_link_index(), "LNKB present");

    let tr = index.trace_by_id(TraceId::from(trace)).unwrap();
    assert_eq!(tr.spans.len(), 2);
    // Spans sort by start_ns: [child(1000), root(2000)].
    let child = &tr.spans[0];
    let root = &tr.spans[1];
    assert_eq!(root.span_id, SpanId::from([1; 8]));

    // Scalars (Decision 2A) are row facets.
    let field = |s: &sfst::TraceSpan, k: &str| -> Vec<String> {
        s.fields
            .iter()
            .filter(|(key, _)| key == k)
            .map(|(_, v)| v.clone())
            .collect()
    };
    assert_eq!(field(root, "trace_state"), ["ot=th:8"]);
    assert_eq!(field(root, "status_message"), ["disk full"]);
    assert_eq!(field(root, "status_code"), ["ERROR"]);

    // Structured events: grouping, order, per-event scalars, stripped attr keys.
    assert_eq!(root.dropped_events_count, 4);
    assert_eq!(root.dropped_links_count, 1);
    assert_eq!(root.events.len(), 2);
    let ev = &root.events[0];
    assert_eq!(
        (ev.time_unix_nano, ev.name.as_str(), ev.dropped_attributes_count),
        (2_100, "exception", 7)
    );
    assert_eq!(
        ev.attributes,
        vec![
            ("exception.type".to_string(), "IOError".to_string()),
            (
                "exception.stacktrace".to_string(),
                "at main()\n  at run()".to_string()
            ),
        ]
    );
    assert_eq!(root.events[1].name, "retry");
    // Nested containers regroup at platform fidelity: kvlist keys dotted,
    // array elements under the collapsed `[]` path, in original order,
    // attached to THIS event.
    assert_eq!(
        root.events[1].attributes,
        vec![
            ("policy".to_string(), "backoff".to_string()),
            ("ctx.user".to_string(), "u1".to_string()),
            ("ctx.ids[]".to_string(), "1".to_string()),
            ("ctx.ids[]".to_string(), "2".to_string()),
        ]
    );

    // Structured link: ids, verbatim trace_state, flags, dropped, attrs.
    assert_eq!(root.links.len(), 1);
    let link = &root.links[0];
    assert_eq!(link.trace_id, TraceId::from([0xB2; 16]));
    assert_eq!(link.span_id, SpanId::from([0xB3; 8]));
    assert_eq!(link.trace_state, "vendor=x");
    assert_eq!(link.flags, 0x300);
    assert_eq!(link.dropped_attributes_count, 2);
    assert_eq!(
        link.attributes,
        vec![
            ("messaging.operation".to_string(), "publish".to_string()),
            ("route.queue".to_string(), "q1".to_string()),
        ]
    );

    // The remapped earlier span kept ITS event (no cross-row bleed).
    assert_eq!(child.events.len(), 1);
    assert_eq!(child.events[0].name, "cache-miss");
    assert!(child.links.is_empty());
    assert_eq!((child.dropped_events_count, child.dropped_links_count), (0, 0));

    // Flat search tokens exist in the field table (searchable) but are excluded
    // from the reconstructed span's facet list (represented structurally).
    let fields = index.field_table();
    assert!(fields.iter().any(|f| f.name == "events.name"));
    assert!(
        fields
            .iter()
            .any(|f| f.name == "events.attributes.exception.stacktrace")
    );
    assert!(fields.iter().any(|f| f.name == "links.attributes.messaging.operation"));
    assert!(
        root.fields.iter().all(|(k, _)| !k.starts_with("events.")),
        "flat events.* tokens excluded from the structured span"
    );
    assert!(root.fields.iter().all(|(k, _)| !k.starts_with("links.")));
    // ...but a span ATTRIBUTE named like the reserved namespace lives under
    // `attributes.events.name` and must survive the filter.
    assert_eq!(field(root, "attributes.events.name"), ["decoy"]);
}

/// A corpus with no events/links (and zero dropped counts) writes neither
/// chunk — the additive-absence contract.
#[test]
fn no_events_no_links_no_chunks() {
    let trace = [0xE2u8; 16];
    let bytes = seal(vec![req(vec![span(
        trace,
        [1; 8],
        [0; 8],
        1_000,
        2_000,
        "plain",
    )])]);
    let index = IndexReader::open(&bytes).unwrap();
    assert!(!index.has_event_index());
    assert!(!index.has_link_index());
    let tr = index.trace_by_id(TraceId::from(trace)).unwrap();
    assert_eq!(tr.spans.len(), 1);
    assert!(tr.spans[0].events.is_empty());
    assert!(tr.spans[0].links.is_empty());
    assert_eq!(tr.spans[0].dropped_events_count, 0);
}

/// `Span.dropped_events_count > 0` with zero surviving events still writes the
/// chunk — the count must not silently vanish.
#[test]
fn dropped_count_alone_preserves_the_chunk() {
    let trace = [0xE3u8; 16];
    let mut s = span(trace, [1; 8], [0; 8], 1_000, 2_000, "lossy");
    s.dropped_events_count = 9;
    let bytes = seal(vec![req(vec![s])]);
    let index = IndexReader::open(&bytes).unwrap();
    assert!(index.has_event_index(), "EVNB carries the dropped count");
    assert!(!index.has_link_index());
    let tr = index.trace_by_id(TraceId::from(trace)).unwrap();
    assert!(tr.spans[0].events.is_empty());
    assert_eq!(tr.spans[0].dropped_events_count, 9);
}

/// Phase-2 bloom: the traces seal writes TBLM; membership answers have no
/// false negatives, and an absent id resolves to an empty trace via the bloom
/// pre-check (same observable result as before, cheaper path).
#[test]
fn seal_writes_trace_id_bloom() {
    let traces: Vec<[u8; 16]> = (1..=40u8).map(|i| [i; 16]).collect();
    let spans: Vec<Span> = traces
        .iter()
        .enumerate()
        .flat_map(|(i, t)| {
            let base = 1_000 + (i as u64) * 100;
            vec![
                span(*t, [1; 8], [0; 8], base, base + 50, "root"),
                span(*t, [2; 8], [1; 8], base + 10, base + 40, "child"),
            ]
        })
        .collect();
    let bytes = seal(vec![req(spans)]);
    let index = IndexReader::open(&bytes).unwrap();
    assert!(index.has_trace_id_bloom(), "TBLM present after the seal");

    let bloom = index.trace_id_bloom().unwrap();
    assert_eq!(bloom.distinct_ids(), 40);
    for t in &traces {
        assert!(bloom.might_contain(TraceId::from(*t)), "no false negatives");
        // The exact lookup still resolves the trace (bloom is a pre-check).
        assert_eq!(index.trace_by_id(TraceId::from(*t)).unwrap().spans.len(), 2);
    }

    // An absent id returns an empty trace through the bloom short-circuit.
    let absent = TraceId::from([0xEEu8; 16]);
    let tr = index.trace_by_id(absent).unwrap();
    assert!(tr.spans.is_empty() && tr.roots.is_empty());
}

// ── The trace rollup (TRSU) ─────────────────────────────────────────

#[test]
fn seal_writes_the_trace_rollup_with_honest_roots_and_stored_counts() {
    // Two traces: A has a true root (unset parent) + a child; B has NO
    // unset-parent span (a broken/partial trace) — its root columns must
    // be sentinels, never a synthesized guess. A's child span is stored
    // twice (a resend) and counts twice — stored-row semantics.
    let a_root = span([0xA; 16], [1; 8], [0; 8], 1_000, 2_000, "root-op");
    let a_child = span([0xA; 16], [2; 8], [1; 8], 1_200, 1_500, "child-op");
    let b_orphan = span([0xB; 16], [3; 8], [9; 8], 5_000, 6_000, "orphan-op");
    let bytes = seal(vec![req(vec![
        a_root,
        a_child.clone(),
        a_child,
        b_orphan,
    ])]);

    let reader = IndexReader::open(&bytes).unwrap();
    assert!(reader.has_trace_rollup());
    let rollup = reader.trace_rollup().unwrap();
    assert_eq!(rollup.len(), 2);

    // Rows sort by trace id: A (0x0A…) then B (0x0B…).
    assert_eq!(rollup.trace_ids.get(0), TraceId::from([0xA; 16]));
    assert_eq!(rollup.span_counts[0], 3, "the resent span counts twice");
    assert_eq!(rollup.min_start_ns[0], 1_000);
    assert_eq!(rollup.max_end_ns[0], 2_000);
    assert_eq!(rollup.root_is_true_root[0], 1);
    assert_eq!(rollup.root_span_ids.get(0), SpanId::from([1; 8]));
    // The root refs resolve through the file interner to the real values.
    let strings = reader.build_string_table(reader.field_table()).unwrap();
    let resolve = |id: u32| strings[id as usize].clone();
    assert_eq!(
        resolve(rollup.root_service_refs[0]),
        "resource.attributes.service.name=svc"
    );
    assert_eq!(resolve(rollup.root_name_refs[0]), "name=root-op");

    assert_eq!(rollup.trace_ids.get(1), TraceId::from([0xB; 16]));
    assert_eq!(rollup.root_is_true_root[1], 0, "no true root → honest absence");
    assert!(rollup.root_span_ids.get(1).is_unset());
    assert_eq!(rollup.root_service_refs[1], sfst::ROLLUP_NO_REF);

    // And the file stays fully readable by the pre-rollup paths — the
    // additive-chunk contract (assembly ignores TRSU entirely).
    let tr = reader.trace_by_id(TraceId::from([0xA; 16])).unwrap();
    assert_eq!(tr.spans.len(), 2, "assembly still dedups the resend");
}

#[test]
fn rollup_captures_error_status_kind_and_service_absence() {
    // Coverage for every capture arm: an ERROR-status root with a set
    // kind, plus a second resource group WITHOUT service.name (the
    // service ref must be the sentinel, not a neighbor's value).
    use opentelemetry_proto::tonic::trace::v1 as otlp;
    let mut root = span([0xC; 16], [1; 8], [0; 8], 1_000, 2_000, "err-root");
    root.kind = 2; // SERVER
    root.status = Some(otlp::Status {
        code: 2, // STATUS_CODE_ERROR
        message: "boom".into(),
    });
    let child = span([0xC; 16], [2; 8], [1; 8], 1_100, 1_200, "ok-child");

    let mut svcless = req(vec![span([0xD; 16], [3; 8], [0; 8], 9_000, 9_100, "svcless-root")]);
    svcless.resource_spans[0].resource = Some(Resource::default());

    let bytes = seal(vec![req(vec![root, child]), svcless]);
    let reader = IndexReader::open(&bytes).unwrap();
    let rollup = reader.trace_rollup().unwrap();
    assert_eq!(rollup.len(), 2);

    // Trace C: ERROR counted once (the child is OK), kind captured raw.
    assert_eq!(rollup.trace_ids.get(0), TraceId::from([0xC; 16]));
    assert_eq!(rollup.span_counts[0], 2);
    assert_eq!(rollup.error_counts[0], 1);
    assert_eq!(rollup.root_kinds[0], 2);
    assert_eq!(rollup.root_is_true_root[0], 1);

    // Trace D: a true root whose resource has NO service.name — the ref
    // is the sentinel, the name ref still resolves.
    assert_eq!(rollup.trace_ids.get(1), TraceId::from([0xD; 16]));
    assert_eq!(rollup.root_is_true_root[1], 1);
    assert_eq!(rollup.root_service_refs[1], sfst::ROLLUP_NO_REF);
    let strings = reader.build_string_table(reader.field_table()).unwrap();
    assert_eq!(
        strings[rollup.root_name_refs[1] as usize],
        "name=svcless-root"
    );
}

#[test]
fn all_unset_trace_ids_seal_without_a_rollup_chunk() {
    // The is-meaningful rule: a file whose spans all carry the unset
    // trace id (not a trace) writes no TRSU chunk at all.
    let s = span([0; 16], [1; 8], [0; 8], 1_000, 2_000, "no-trace");
    let bytes = seal(vec![req(vec![s])]);
    let reader = IndexReader::open(&bytes).unwrap();
    assert!(!reader.has_trace_rollup());
    assert!(reader.trace_rollup().is_err(), "no chunk to read");
}

const DERIVED_TRACE: [u8; 16] = [0x5A; 16];
const DERIVED_BASE: u64 = 1_700_000_000_000_000_000;

/// A span of the derivation fixtures: ids as repeated bytes (parent 0 =
/// root), times relative to `DERIVED_BASE`, ERROR when `error`.
fn family_span(id: u8, parent: u8, start: u64, end: u64, error: bool) -> Span {
    let mut s = span(
        DERIVED_TRACE,
        [id; 8],
        [parent; 8],
        DERIVED_BASE + start,
        DERIVED_BASE + end,
        &format!("op-{id}"),
    );
    if error {
        s.status = Some(opentelemetry_proto::tonic::trace::v1::Status {
            code: 2,
            message: String::new(),
        });
    }
    s
}

/// A propagated chain (1 → 2 → 3, all ERROR), a handled error (ERROR 4 with
/// OK children 5, 8, 9 and a zero-length 10), a lone ERROR 6 and an OK 7.
fn family() -> Vec<Span> {
    vec![
        family_span(1, 0, 0, 100, true),
        family_span(2, 1, 10, 60, true),
        family_span(3, 2, 20, 30, true),
        family_span(4, 0, 200, 300, true),
        family_span(5, 4, 210, 250, false),
        family_span(8, 4, 240, 280, false),
        family_span(9, 4, 290, 350, false),
        family_span(10, 4, 260, 260, false),
        family_span(6, 0, 400, 410, true),
        family_span(7, 0, 500, 510, false),
    ]
}

/// The span-id bytes of the rows carrying `_err_origin=true`, and every
/// row's child duration by span-id byte.
fn derived_by_span(bytes: &[u8]) -> (Vec<u8>, HashMap<u8, i64>) {
    let reader = IndexReader::open(bytes).unwrap();
    let span_ids = reader.span_ids().unwrap();
    let filter = reader
        .compile_filter(
            &sfst::Filter::new().select(sfst::ERR_ORIGIN_FIELD, "true"),
            None,
        )
        .unwrap();
    let mut origins = Vec::new();
    for position in reader.matched_positions(&filter, 0..i64::MAX).unwrap() {
        origins.push(span_ids.get(position as usize).as_bytes()[0]);
    }
    origins.sort();
    let children = reader.child_durations().unwrap();
    let mut child = HashMap::new();
    for (position, value) in children.0.iter().enumerate() {
        child.insert(span_ids.get(position).as_bytes()[0], *value);
    }
    (origins, child)
}

#[test]
fn seal_marks_error_origins() {
    let dir = tempfile::tempdir().unwrap();
    let wal_path = write_wal(dir.path(), vec![req(family())]);
    let out = dir.path().join("traces.sfst");
    build_sfst_traces_file(&wal_path, &out, &Metrics::new()).unwrap();
    let sealed = std::fs::read(&out).unwrap();
    let reader = IndexReader::open(&sealed).unwrap();

    let entry = reader.field_table().get(sfst::ERR_ORIGIN_FIELD).unwrap();
    assert_eq!((entry.cardinality, entry.tier), (1, sfst::FieldTier::Low));
    assert!(
        reader
            .tree()
            .derive_scalar_kinds()
            .contains(&(sfst::ERR_ORIGIN_FIELD.to_string(), sfst::ValueKind::Bool))
    );
    let filter = reader.compile_filter(&sfst::Filter::new(), None).unwrap();
    let facets = reader
        .facets(&[sfst::ERR_ORIGIN_FIELD], &filter, 0..i64::MAX)
        .unwrap();
    assert_eq!(facets[0].values, [("true".to_string(), 3)]);
    assert_eq!(derived_by_span(&sealed).0, [3, 4, 6]);

    // Every row materializes the same fields as the chunk image (which has
    // no token) apart from the token itself: the token's tree leaf keeps the
    // id-to-field mapping of every other field intact.
    let file_len = std::fs::metadata(&wal_path).unwrap().len();
    let (_, image) = ng_index::build_sfst_traces_range(
        &wal_path,
        wal::FrameRange::new(wal::HEADER_SIZE as u64, file_len),
    )
    .unwrap();
    let image = IndexReader::open(&image).unwrap();
    let all: Vec<u32> = (0..10).collect();
    let sealed_rows = reader.materialize_rows(&all).unwrap();
    let image_rows = image.materialize_rows(&all).unwrap();
    for (sealed_row, image_row) in sealed_rows.iter().zip(&image_rows) {
        let mut fields = sealed_row.fields.clone();
        fields.retain(|(field, _)| field != sfst::ERR_ORIGIN_FIELD);
        assert_eq!(fields, image_row.fields);
    }
}

#[test]
fn seal_without_errors_has_no_err_origin_field() {
    let spans = vec![
        family_span(1, 0, 0, 100, false),
        family_span(2, 1, 10, 60, false),
    ];
    let bytes = seal(vec![req(spans)]);
    let reader = IndexReader::open(&bytes).unwrap();

    assert!(reader.field_table().get(sfst::ERR_ORIGIN_FIELD).is_none());
    assert!(reader.field_table().get("_role").is_some());
    assert_eq!(reader.child_durations().unwrap().0, [50, 0]);
}

#[test]
fn seal_writes_child_durations() {
    let bytes = seal(vec![req(family())]);
    let (_, child) = derived_by_span(&bytes);

    let expected = HashMap::from([
        (1, 50),
        (2, 10),
        (3, 0),
        (4, 80),
        (5, 0),
        (8, 0),
        (9, 0),
        (10, 0),
        (6, 0),
        (7, 0),
    ]);
    assert_eq!(child, expected);
    let reader = IndexReader::open(&bytes).unwrap();
    let durations = reader.durations().unwrap();
    let children = reader.child_durations().unwrap();
    for (child, duration) in children.0.iter().zip(&durations.0) {
        assert!((0..=*duration).contains(child), "{child} of {duration}");
    }
}

#[test]
fn derived_values_do_not_depend_on_frame_order() {
    let spans = family();
    let parents_first = seal(vec![req(spans[..4].to_vec()), req(spans[4..].to_vec())]);
    let mut children_first = vec![req(spans[4..].to_vec())];
    children_first.push(req(spans[..4].to_vec()));
    let children_first = seal(children_first);

    assert_eq!(
        derived_by_span(&parents_first),
        derived_by_span(&children_first)
    );
}

#[test]
fn chunk_images_carry_no_derived_values() {
    let dir = tempfile::tempdir().unwrap();
    let wal_path = write_wal(dir.path(), vec![req(family())]);
    let file_len = std::fs::metadata(&wal_path).unwrap().len();
    let (_, image) = ng_index::build_sfst_traces_range(
        &wal_path,
        wal::FrameRange::new(wal::HEADER_SIZE as u64, file_len),
    )
    .unwrap();
    let out = dir.path().join("traces.sfst");
    build_sfst_traces_file(&wal_path, &out, &Metrics::new()).unwrap();
    let sealed = std::fs::read(&out).unwrap();

    let image = IndexReader::open(&image).unwrap();
    let sealed = IndexReader::open(&sealed).unwrap();
    assert!(image.field_table().get(sfst::ERR_ORIGIN_FIELD).is_none());
    assert!(
        !image
            .columns_table()
            .names()
            .any(|name| name == "child_duration")
    );
    assert!(
        sealed
            .columns_table()
            .names()
            .any(|name| name == "child_duration")
    );
    let entries = |reader: &IndexReader<'_>| {
        let mut out = Vec::new();
        for entry in reader.field_table().iter() {
            if entry.name != sfst::ERR_ORIGIN_FIELD {
                out.push((entry.name.clone(), entry.cardinality, entry.tier));
            }
        }
        out
    };
    assert_eq!(entries(&image), entries(&sealed));
}

/// A parent and its only child in two different WALs: each file derives its
/// values alone, so the parent keeps its whole duration as self time and is
/// an origin there, and the child is an origin in its own file.
#[test]
fn straddling_parent_keeps_in_file_values() {
    let first = seal(vec![req(vec![family_span(1, 0, 0, 100, true)])]);
    let second = seal(vec![req(vec![family_span(2, 1, 10, 60, true)])]);

    assert_eq!(derived_by_span(&first), (vec![1], HashMap::from([(1, 0)])));
    assert_eq!(derived_by_span(&second), (vec![2], HashMap::from([(2, 0)])));
}

/// Writes a lab-size traces WAL (50,000 spans: 5,000 traces of ten, one in
/// twenty ERROR) under `$SEAL_PERF_DIR`, for timing the seal with
/// `ng-index-traces seal` under `/usr/bin/time -v`. Run it on its own:
/// `SEAL_PERF_DIR=<dir> cargo test --release -p ng-index --test traces_seal
/// -- --ignored write_a_lab_size_wal`.
#[test]
#[ignore]
fn write_a_lab_size_wal() {
    let dir = std::path::PathBuf::from(std::env::var("SEAL_PERF_DIR").unwrap());
    let mut requests = Vec::new();
    for t in 0..5_000u32 {
        let mut trace = [0u8; 16];
        trace[..4].copy_from_slice(&t.to_be_bytes());
        trace[15] = 1;
        let start = DERIVED_BASE + u64::from(t) * 10_000_000;
        let mut spans = Vec::new();
        for s in 0..10u8 {
            let mut id = [0u8; 8];
            id[..4].copy_from_slice(&t.to_be_bytes());
            id[7] = s + 1;
            let mut parent = [0u8; 8];
            if s > 0 {
                parent = id;
                parent[7] = s;
            }
            let begin = start + u64::from(s) * 100_000;
            let mut span = span(
                trace,
                id,
                parent,
                begin,
                begin + 5_000_000,
                &format!("op-{s}"),
            );
            if (t + u32::from(s)) % 20 == 0 {
                span.status = Some(opentelemetry_proto::tonic::trace::v1::Status {
                    code: 2,
                    message: String::new(),
                });
            }
            spans.push(span);
        }
        requests.push(req(spans));
    }
    let path = write_wal(&dir, requests);
    println!("{}", path.display());
}

/// A chunk image with the values the seal stored attached reads like the
/// sealed file of the same frames (field table, facets, chips, timeline,
/// value lists, materialized rows and fields, child durations); every other
/// field reads as it did without the overlay; a sealed file refuses one, and
/// malformed values are refused.
#[test]
fn an_overlaid_chunk_image_reads_like_the_sealed_file() {
    let dir = tempfile::tempdir().unwrap();
    let wal_path = write_wal(dir.path(), vec![req(family())]);
    let out = dir.path().join("traces.sfst");
    build_sfst_traces_file(&wal_path, &out, &Metrics::new()).unwrap();
    let sealed_bytes = std::fs::read(&out).unwrap();
    let file_len = std::fs::metadata(&wal_path).unwrap().len();
    let (_, image_bytes) = ng_index::build_sfst_traces_range(
        &wal_path,
        wal::FrameRange::new(wal::HEADER_SIZE as u64, file_len),
    )
    .unwrap();
    let sealed = IndexReader::open(&sealed_bytes).unwrap();
    let plain = IndexReader::open(&image_bytes).unwrap();

    let origin_chip = sfst::Filter::new().select(sfst::ERR_ORIGIN_FIELD, "true");
    let err_origin = sealed
        .matched_positions(
            &sealed.compile_filter(&origin_chip, None).unwrap(),
            0..i64::MAX,
        )
        .unwrap();
    let derived = Arc::new(sfst::DerivedValues {
        err_origin: err_origin.clone(),
        child_duration: sealed.child_durations().unwrap().clone(),
    });
    let overlaid = IndexReader::open(&image_bytes)
        .unwrap()
        .with_derived(derived.clone())
        .unwrap();

    let table = |reader: &IndexReader<'_>| {
        let mut out = Vec::new();
        for entry in reader.field_table().iter() {
            out.push((entry.name.clone(), entry.cardinality, entry.tier));
        }
        out
    };
    assert_eq!(table(&overlaid), table(&sealed));
    assert_eq!(
        overlaid.child_durations().unwrap(),
        sealed.child_durations().unwrap()
    );

    let rows = sealed.summary().record_count;
    let origins = |reader: &IndexReader<'_>| {
        let values = reader.row_values(sfst::ERR_ORIGIN_FIELD, 0..rows).unwrap();
        let mut out = Vec::new();
        for position in 0..rows {
            out.push(
                values
                    .value_at(position)
                    .map(|index| values.value(index).to_string()),
            );
        }
        out
    };
    assert_eq!(origins(&overlaid), origins(&sealed));
    assert!(origins(&sealed).iter().any(Option::is_some));
    assert!(origins(&plain).iter().all(Option::is_none));

    let all: Vec<u32> = (0..sealed.summary().record_count).collect();
    assert_eq!(
        overlaid.materialize_rows(&all).unwrap(),
        sealed.materialize_rows(&all).unwrap()
    );
    let names: Vec<String> = table(&sealed)
        .into_iter()
        .map(|(name, _, _)| name)
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_eq!(
        overlaid.materialize_fields(&names, &all).unwrap(),
        sealed.materialize_fields(&names, &all).unwrap()
    );
    let everything = sfst::Filter::new();
    let (over_all, sealed_all) = (
        overlaid.compile_filter(&everything, None).unwrap(),
        sealed.compile_filter(&everything, None).unwrap(),
    );
    assert_eq!(
        overlaid.facets(&names, &over_all, 0..i64::MAX).unwrap(),
        sealed.facets(&names, &sealed_all, 0..i64::MAX).unwrap()
    );
    let grid = sfst::Grid::new(DERIVED_BASE as i64, 100, 6);
    assert_eq!(
        overlaid
            .timeline(sfst::ERR_ORIGIN_FIELD, &over_all, grid)
            .unwrap(),
        sealed
            .timeline(sfst::ERR_ORIGIN_FIELD, &sealed_all, grid)
            .unwrap()
    );
    let chip = overlaid.compile_filter(&origin_chip, None).unwrap();
    assert_eq!(
        overlaid.matched_positions(&chip, 0..i64::MAX).unwrap(),
        err_origin
    );
    let pattern = sfst::Filter::new().select_pattern(sfst::ERR_ORIGIN_FIELD, "t.*");
    let chip = overlaid.compile_filter(&pattern, None).unwrap();
    assert_eq!(
        overlaid.matched_positions(&chip, 0..i64::MAX).unwrap(),
        err_origin
    );
    assert_eq!(
        overlaid.field_values(sfst::ERR_ORIGIN_FIELD).unwrap(),
        ["true"]
    );
    assert_eq!(
        overlaid
            .field_values_with_prefix(sfst::ERR_ORIGIN_FIELD, "t", 10)
            .unwrap(),
        ["true"]
    );

    // Every other field reads as it did: the token has no KvId, so the
    // stored ids still map to their own fields.
    let plain_rows = plain.materialize_rows(&all).unwrap();
    let over_rows = overlaid.materialize_rows(&all).unwrap();
    for (plain_row, over_row) in plain_rows.iter().zip(&over_rows) {
        let mut fields = over_row.fields.clone();
        fields.retain(|(field, _)| field != sfst::ERR_ORIGIN_FIELD);
        assert_eq!(fields, plain_row.fields);
    }

    assert!(matches!(
        IndexReader::open(&sealed_bytes)
            .unwrap()
            .with_derived(derived.clone()),
        Err(sfst::Error::DerivedConflict)
    ));
    let rows = all.len();
    let values = |err_origin: Vec<u32>, rows: usize| {
        Arc::new(sfst::DerivedValues {
            err_origin,
            child_duration: sfst::ChildDurations(vec![0; rows]),
        })
    };
    let attach = |values| {
        IndexReader::open(&image_bytes)
            .unwrap()
            .with_derived(values)
    };
    assert!(matches!(
        attach(values(vec![], rows - 1)),
        Err(sfst::Error::ColumnLengthMismatch { .. })
    ));
    for bad in [vec![3, 1], vec![2, 2], vec![rows as u32]] {
        assert!(
            matches!(
                attach(values(bad.clone(), rows)),
                Err(sfst::Error::CorruptIndex(_))
            ),
            "{bad:?}"
        );
    }
    let calm = attach(values(vec![], rows)).unwrap();
    assert_eq!(table(&calm), table(&plain));
    assert_eq!(calm.child_durations().unwrap().0, vec![0; rows]);
}

/// `row_values` gives each row of a range its value of a field: Low (the
/// service), Mid (the pinned `name` with 150 values), the lowest value of a
/// multi-valued field, none for an absent field or a row outside the range;
/// a High field is refused.
#[test]
fn row_values_give_each_row_its_value() {
    const ROWS: u64 = 1_100;
    let mut spans = Vec::new();
    for i in 0..ROWS {
        let mut s = span(
            [1; 16],
            (i + 1).to_be_bytes(),
            [0; 8],
            DERIVED_BASE + i * 1_000,
            DERIVED_BASE + i * 1_000 + 10,
            &format!("op-{:03}", i % 150),
        );
        s.attributes.push(kv("request.id", &format!("r{i:05}")));
        let tags: &[&str] = match i {
            0 => &["b", "a"],
            1 => &["c"],
            _ => &[],
        };
        if !tags.is_empty() {
            let values = tags
                .iter()
                .map(|tag| AnyValue {
                    value: Some(Av::StringValue(tag.to_string())),
                })
                .collect();
            s.attributes
                .push(kv_any("tags", Av::ArrayValue(ArrayValue { values })));
        }
        spans.push(s);
    }
    let bytes = seal(vec![req(spans)]);
    let reader = IndexReader::open(&bytes).unwrap();
    let rows = reader.summary().record_count;
    assert_eq!(u64::from(rows), ROWS);
    let tier = |field: &str| reader.field_table().get(field).map(|entry| entry.tier);
    assert_eq!(
        tier("resource.attributes.service.name"),
        Some(sfst::FieldTier::Low)
    );
    assert_eq!(tier("name"), Some(sfst::FieldTier::Mid));
    assert_eq!(tier("attributes.request.id"), Some(sfst::FieldTier::High));

    let at = |values: &sfst::RowValues, position: u32| {
        values
            .value_at(position)
            .map(|index| values.value(index).to_string())
    };
    let service = reader
        .row_values("resource.attributes.service.name", 0..rows)
        .unwrap();
    assert_eq!(service.values, vec!["svc".to_string()]);
    for position in 0..rows {
        assert_eq!(at(&service, position).as_deref(), Some("svc"));
    }

    let names = reader.row_values("name", 0..rows).unwrap();
    assert_eq!(names.values.len(), 150);
    for position in 0..rows {
        let want = format!("op-{:03}", position % 150);
        assert_eq!(at(&names, position), Some(want), "row {position}");
    }

    let window = reader.row_values("name", 10..20).unwrap();
    assert_eq!(window.values.len(), 10);
    assert_eq!(at(&window, 9), None);
    assert_eq!(at(&window, 10).as_deref(), Some("op-010"));
    assert_eq!(at(&window, 19).as_deref(), Some("op-019"));
    assert_eq!(at(&window, 20), None);

    let tags = reader.row_values("attributes.tags[]", 0..rows).unwrap();
    assert_eq!(at(&tags, 0).as_deref(), Some("a"), "the lowest of b, a");
    assert_eq!(at(&tags, 1).as_deref(), Some("c"));
    assert_eq!(at(&tags, 2), None);

    let absent = reader.row_values("attributes.nope", 0..rows).unwrap();
    assert!(absent.values.is_empty());
    assert_eq!(at(&absent, 0), None);

    assert!(matches!(
        reader.row_values("attributes.request.id", 0..rows),
        Err(sfst::Error::HighCardFacet(_))
    ));
}

/// Ten spans one second apart from `DERIVED_BASE`, the i-th lasting
/// (i + 1) × 100 ms, named "a" when i is even and "b" otherwise.
fn ten_spans() -> Vec<u8> {
    let mut spans = Vec::new();
    for i in 0..10u64 {
        let start = DERIVED_BASE + i * 1_000_000_000;
        spans.push(span(
            [7; 16],
            (i + 1).to_be_bytes(),
            [0; 8],
            start,
            start + (i + 1) * 100_000_000,
            if i % 2 == 0 { "a" } else { "b" },
        ));
    }
    seal(vec![req(spans)])
}

const EVERYTHING: std::ops::Range<i64> = 0..i64::MAX;

/// QRY-04: a duration term, inclusive at both edges and open on either side,
/// narrows every statistic it is conjoined into; a field's facet keeps it.
#[test]
fn duration_term_scopes_every_statistic() {
    let bytes = ten_spans();
    let reader = IndexReader::open(&bytes).unwrap();
    let ms = |n: i64| n * 1_000_000;
    let range = |min_ns, max_ns| sfst::DurationRange { min_ns, max_ns };

    let middle = reader
        .compile_duration(range(Some(ms(300)), Some(ms(500))))
        .unwrap();
    assert_eq!(
        reader.matched_positions(&middle, EVERYTHING).unwrap(),
        [2, 3, 4]
    );
    assert_eq!(reader.matched_count(&middle, EVERYTHING).unwrap(), 3);
    for (bounds, want) in [
        (range(Some(ms(800)), None), 3),
        (range(None, Some(ms(150))), 1),
        (range(None, None), 10),
        (range(Some(ms(600)), Some(ms(599))), 0),
    ] {
        let term = reader.compile_duration(bounds).unwrap();
        assert_eq!(
            reader.matched_count(&term, EVERYTHING).unwrap(),
            want,
            "{bounds:?}"
        );
    }

    let chip = reader
        .compile_filter(&sfst::Filter::new().select("name", "a"), None)
        .unwrap();
    let scoped = chip.conjoin(&middle);
    assert_eq!(
        reader.matched_positions(&scoped, EVERYTHING).unwrap(),
        [2, 4]
    );
    let facets = reader.facets(&["name"], &scoped, EVERYTHING).unwrap();
    let counts: Vec<(String, u32)> = facets[0].values.clone();
    assert_eq!(counts, [("a".to_string(), 2), ("b".to_string(), 1)]);
    let grid = sfst::Grid::new(DERIVED_BASE as i64, 1_000_000_000, 10);
    let timeline = reader.timeline("name", &middle, grid).unwrap();
    let per_bucket: Vec<u64> = timeline
        .buckets
        .iter()
        .map(|bucket| bucket.counts.iter().sum::<u64>() + bucket.unset)
        .collect();
    assert_eq!(per_bucket, [0, 0, 1, 1, 1, 0, 0, 0, 0, 0]);
}

/// A time term keeps the rows starting in `[start, end)`.
#[test]
fn time_range_term_clips_by_start() {
    let bytes = ten_spans();
    let reader = IndexReader::open(&bytes).unwrap();
    let second = |n: u64| (DERIVED_BASE + n * 1_000_000_000) as i64;
    let term = reader.compile_time_range(second(3)..second(6)).unwrap();
    assert_eq!(
        reader.matched_positions(&term, EVERYTHING).unwrap(),
        [3, 4, 5]
    );
    let outside = reader.compile_time_range(second(20)..second(30)).unwrap();
    assert_eq!(reader.matched_count(&outside, EVERYTHING).unwrap(), 0);
}

/// Counting without a field drops that field's chips from both operands of a
/// conjunction and keeps the global terms.
#[test]
fn count_without_drops_a_fields_chips_from_both_operands() {
    let bytes = ten_spans();
    let reader = IndexReader::open(&bytes).unwrap();
    let chip = |value| {
        reader
            .compile_filter(&sfst::Filter::new().select("name", value), None)
            .unwrap()
    };
    let long = reader
        .compile_duration(sfst::DurationRange {
            min_ns: Some(300_000_000),
            max_ns: None,
        })
        .unwrap();
    let scope = chip("a");
    let both = scope.conjoin(&chip("b").conjoin(&long));
    assert_eq!(reader.matched_count(&both, EVERYTHING).unwrap(), 0);
    assert_eq!(reader.count_without(&both, "name", EVERYTHING).unwrap(), 8);
    assert_eq!(
        reader.count_without(&scope, "name", EVERYTHING).unwrap(),
        10
    );
    assert_eq!(reader.count_without(&both, "other", EVERYTHING).unwrap(), 0);
}

/// A traces frame as the flattener wrote it before the per-span `_role` and
/// `_duration_band` tokens: the same payload format and entries, less those two.
fn token_free_frame(mut request: ExportTraceServiceRequest) -> (Vec<u8>, usize) {
    let count = count_spans(&request);
    ng_flatten::normalize_trace_request(&mut request, 1, None);
    let mut flattener = ng_flatten::Flattener::new();
    let mut resources = Vec::new();
    for rs in request.resource_spans {
        let resource = rs
            .resource
            .map(|r| flattener.flatten_resource(r))
            .unwrap_or_default();
        let mut scopes = Vec::new();
        for ss in rs.scope_spans {
            let scope = ss
                .scope
                .map(|s| flattener.flatten_scope(s))
                .unwrap_or_default();
            let mut spans = Vec::new();
            for sp in ss.spans {
                let end = sp.end_time_unix_nano.max(sp.start_time_unix_nano);
                let record = ng_flatten::SpanRecord {
                    ts: i64::try_from(sp.start_time_unix_nano).unwrap(),
                    duration: i64::try_from(end - sp.start_time_unix_nano).unwrap(),
                    trace_id: ng_flatten::TraceId::from_bytes(&sp.trace_id).unwrap_or_default(),
                    span_id: ng_flatten::SpanId::from_bytes(&sp.span_id).unwrap_or_default(),
                    parent_span_id: ng_flatten::SpanId::from_bytes(&sp.parent_span_id)
                        .unwrap_or_default(),
                    flags: sp.flags,
                    dropped_attributes_count: sp.dropped_attributes_count,
                    dropped_events_count: sp.dropped_events_count,
                    dropped_links_count: sp.dropped_links_count,
                    entries: Vec::new(),
                    events: Vec::new(),
                    links: Vec::new(),
                };
                let flat = flattener.flatten_span(sp);
                spans.push(ng_flatten::SpanRecord {
                    entries: flat.entries,
                    events: flat.events,
                    links: flat.links,
                    ..record
                });
            }
            scopes.push(ng_flatten::SpanScopeGroup { scope, spans });
        }
        resources.push(ng_flatten::SpanResourceGroup { resource, scopes });
    }
    let flat = ng_flatten::FlattenedTraceRequest {
        tree: flattener.into_tree(),
        resources,
    };
    (ng_flatten::encode_trace_frame(&flat).unwrap(), count)
}

/// FLAT-15: the frames kept their payload format, so a WAL written before the
/// tokens still seals, into a file with no `_role` field (the query's legacy
/// test); a WAL written now gives every sealed row exactly one role.
#[test]
fn a_wal_from_before_the_tokens_seals_without_roles() {
    assert_eq!(ng_flatten::TRACE_FRAME_PAYLOAD_FORMAT, 3);
    let spans = || {
        vec![
            family_span(1, 0, 0, 100, false),
            family_span(2, 1, 10, 60, true),
        ]
    };

    let dir = tempfile::tempdir().unwrap();
    let wal_path = write_wal_frames(dir.path(), vec![token_free_frame(req(spans()))]);
    let out = dir.path().join("old.sfst");
    build_sfst_traces_file(&wal_path, &out, &Metrics::new()).unwrap();
    let old = std::fs::read(&out).unwrap();
    let reader = IndexReader::open(&old).unwrap();
    assert_eq!(reader.summary().record_count, 2);
    assert!(reader.field_table().get(ng_flatten::ROLE_FIELD).is_none());
    assert!(
        reader
            .field_table()
            .get(ng_flatten::DURATION_BAND_FIELD)
            .is_none()
    );
    assert!(reader.field_table().get("name").is_some());

    let new = seal(vec![req(spans())]);
    let reader = IndexReader::open(&new).unwrap();
    let rows = reader.summary().record_count;
    let roles = reader.row_values(ng_flatten::ROLE_FIELD, 0..rows).unwrap();
    let mut with_a_role = 0;
    for position in 0..rows {
        if roles.value_at(position).is_some() {
            with_a_role += 1;
        }
    }
    assert_eq!(with_a_role, rows);
}
