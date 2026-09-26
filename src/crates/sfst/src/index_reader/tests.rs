//! Tests for `IndexReader::trace_by_id` trace reconstruction: span dedup,
//! root detection, and the cycle -> forest reachability guard.

use crate::writer::{ChunkCounts, ChunkWriter, ColumnsPresent};
use crate::{
    BitmapValue, ColumnEntry, ColumnsTable, DroppedAttributeCounts, Durations, Flags, Histogram,
    IdRanges, IndexReader, KvId, Metadata, ParentSpanIds, SpanId, SpanIds, StreamBatch, Summary,
    TraceId, TraceIdIndex, TraceIds,
};

const TRACE: [u8; 16] = [7u8; 16];

fn sid(b: u8) -> SpanId {
    SpanId::from([b; 8])
}

/// Build a minimal traces SFST whose rows all share one `trace_id`, from a list
/// of `(span_id, parent_span_id)` pairs in chronological (row) order. Fields are
/// empty (the tree logic under test reads ids/timestamps, not attributes).
fn trace_file(rows: &[(SpanId, SpanId)]) -> Vec<u8> {
    trace_file_with_bloom(rows, None)
}

/// Like [`trace_file`], optionally carrying a caller-supplied `TBLM` payload
/// (which may be deliberately malformed — the writer packs, it does not vet).
fn trace_file_with_bloom(
    rows: &[(SpanId, SpanId)],
    bloom: Option<&crate::TraceIdBloom>,
) -> Vec<u8> {
    let n = rows.len();
    let mut trace = TraceIds::with_capacity(n);
    let mut span = SpanIds::with_capacity(n);
    let mut parent = ParentSpanIds::with_capacity(n);
    for &(s, p) in rows {
        trace.push(TraceId::from(TRACE));
        span.push(s);
        parent.push(p);
    }
    let flags = Flags(vec![0u32; n]);
    let drac = DroppedAttributeCounts(vec![0u32; n]);
    let durations = Durations(vec![0i64; n]);
    let index = TraceIdIndex::build(&trace);

    let columns = ColumnsPresent {
        observed_ts: false,
        trace_id: true,
        span_id: true,
        flags: true,
        dropped_attributes_count: true,
        parent_span_id: true,
        duration: true,
        child_duration: false,
    };
    let counts = ChunkCounts {
        columns,
        trace_id_index: true,
        trace_id_bloom: bloom.is_some(),
        event_index: false,
        link_index: false,
        mid_fields: 0,
        high_fields: 0,
        stream_batches: 1,
    };
    let summary = Summary {
        min_timestamp_s: 0,
        max_timestamp_s: n as u32,
        record_count: n as u32,
        content_meta: Vec::new(),
    };
    let metadata = Metadata {
        histogram: Histogram {
            timestamps: vec![0],
            counts: vec![n as u32],
        },
        id_ranges: IdRanges {
            low_end: KvId(0),
            mid_end: KvId(0),
            high_end: KvId(0),
        },
        tree: Default::default(),
        columns: ColumnsTable(vec![
            ColumnEntry {
                name: TraceIds::NAME.into(),
                ty: TraceIds::COLUMN_TYPE,
            },
            ColumnEntry {
                name: SpanIds::NAME.into(),
                ty: SpanIds::COLUMN_TYPE,
            },
            ColumnEntry {
                name: Flags::NAME.into(),
                ty: Flags::COLUMN_TYPE,
            },
            ColumnEntry {
                name: DroppedAttributeCounts::NAME.into(),
                ty: DroppedAttributeCounts::COLUMN_TYPE,
            },
            ColumnEntry {
                name: ParentSpanIds::NAME.into(),
                ty: ParentSpanIds::COLUMN_TYPE,
            },
            ColumnEntry {
                name: Durations::NAME.into(),
                ty: Durations::COLUMN_TYPE,
            },
        ]),
    };
    // One ascending timestamp per row so start-sort order == row order.
    let timestamps: Vec<i64> = (0..n as i64).collect();

    let mut w = ChunkWriter::new(std::io::Cursor::new(Vec::new()), counts).unwrap();
    w.summary(&summary).unwrap();
    w.metadata(&metadata).unwrap();
    w.timestamps(&timestamps).unwrap();
    w.primary(std::iter::empty::<(&str, BitmapValue)>())
        .unwrap();
    w.trace_ids(&trace).unwrap();
    w.span_ids(&span).unwrap();
    w.flags(&flags).unwrap();
    w.dropped_attribute_counts(&drac).unwrap();
    w.parent_span_ids(&parent).unwrap();
    w.durations(&durations).unwrap();
    w.trace_id_index(&index).unwrap();
    if let Some(b) = bloom {
        w.trace_id_bloom(b).unwrap();
    }
    w.add_stream_batch(&StreamBatch::for_write(&vec![Vec::<KvId>::new(); n]))
        .unwrap();
    w.finish().unwrap().into_inner()
}

#[test]
fn corrupt_bloom_degrades_to_the_exact_lookup() {
    // The bloom is a skip hint: a TBLM that decodes but fails validation
    // (absurd hash count here) must NOT make a findable trace unfindable —
    // trace_by_id falls through to TIDX. The accessor itself still errors,
    // so cross-file callers can observe the corruption.
    let hostile = crate::TraceIdBloom::raw_for_tests(
        1,
        fastbloom::BloomFilter::from_vec(vec![0u64; 4])
            .seed(&1)
            .hashes(1_000),
    );
    let buf = trace_file_with_bloom(&[(sid(1), SpanId::from([0; 8]))], Some(&hostile));
    let reader = IndexReader::open(&buf).unwrap();

    assert!(reader.has_trace_id_bloom());
    assert!(
        reader.trace_id_bloom().is_err(),
        "accessor surfaces corruption"
    );
    let trace = reader.trace_by_id(TraceId::from(TRACE)).unwrap();
    assert_eq!(trace.spans.len(), 1, "lookup degraded to TIDX and resolved");
}

#[test]
fn absent_trace_yields_empty() {
    let buf = trace_file(&[(sid(1), SpanId::from([0; 8]))]);
    let reader = IndexReader::open(&buf).unwrap();
    let trace = reader.trace_by_id(TraceId::from([0xEE; 16])).unwrap();
    assert!(trace.spans.is_empty());
    assert!(trace.roots.is_empty());
}

#[test]
fn duplicate_span_id_is_collapsed_to_first() {
    let unset = SpanId::from([0; 8]);
    // span A sent twice (a resend), then span B — all roots (unset parents).
    let buf = trace_file(&[(sid(1), unset), (sid(1), unset), (sid(2), unset)]);
    let reader = IndexReader::open(&buf).unwrap();
    let trace = reader.trace_by_id(TraceId::from(TRACE)).unwrap();

    // The two A rows collapse to one span; B remains → 2 spans, both roots.
    assert_eq!(trace.spans.len(), 2);
    assert_eq!(trace.roots.len(), 2);
    // `children` is parallel to `spans` — no edges means all-empty lists.
    assert!(trace.children.iter().all(|kids| kids.is_empty()));
}

#[test]
fn unset_span_ids_are_not_collapsed() {
    let unset = SpanId::from([0; 8]);
    // Two rows both with an UNSET span_id are distinct spans, not one resend.
    let buf = trace_file(&[(unset, unset), (unset, unset)]);
    let reader = IndexReader::open(&buf).unwrap();
    let trace = reader.trace_by_id(TraceId::from(TRACE)).unwrap();
    assert_eq!(trace.spans.len(), 2);
    assert_eq!(trace.roots.len(), 2);
}

#[test]
fn parent_edges_and_missing_parent_root() {
    let unset = SpanId::from([0; 8]);
    // A(root), B(child of A), C(parent = X which is absent from this file → root).
    let buf = trace_file(&[(sid(1), unset), (sid(2), sid(1)), (sid(3), sid(9))]);
    let reader = IndexReader::open(&buf).unwrap();
    let trace = reader.trace_by_id(TraceId::from(TRACE)).unwrap();

    assert_eq!(trace.spans.len(), 3);
    // A and C are roots; B is A's child.
    assert_eq!(trace.roots.len(), 2);
    let a_idx = trace.spans.iter().position(|s| s.span_id == sid(1)).unwrap();
    let a_children = &trace.children[a_idx];
    assert_eq!(a_children.len(), 1);
    // The edge points at B.
    assert_eq!(trace.spans[a_children[0]].span_id, sid(2));
}

#[test]
fn parent_cycle_stays_a_forest_and_terminates() {
    // A's parent is B, B's parent is A — a 2-node cycle with no external entry.
    // The reachability guard must promote a root so every span is reachable,
    // and must not loop forever.
    let buf = trace_file(&[(sid(1), sid(2)), (sid(2), sid(1))]);
    let reader = IndexReader::open(&buf).unwrap();
    let trace = reader.trace_by_id(TraceId::from(TRACE)).unwrap();

    assert_eq!(trace.spans.len(), 2);
    // At least one span is promoted to a root so the forest is walkable.
    assert!(!trace.roots.is_empty());

    // Every span is reachable from some root via a revisit-guarded walk.
    let mut seen = vec![false; trace.spans.len()];
    let mut stack = trace.roots.clone();
    while let Some(i) = stack.pop() {
        if seen[i] {
            continue;
        }
        seen[i] = true;
        stack.extend(trace.children[i].iter().copied().filter(|&c| !seen[c]));
    }
    assert!(seen.iter().all(|&s| s), "every span reachable from a root");
}

/// A traces file with a low (`l`), a mid (`m`, 16 values) and a high (`h`,
/// 128 values) field over 256 rows (one stream batch), every span column,
/// the trace-id index and the bloom. Rows belong to traces `[1; 16]`..`[8; 16]`.
fn memo_file() -> Vec<u8> {
    ids_file(true)
}

/// [`memo_file`]'s rows, with or without the trace-id index, bloom and child
/// time (a logs file has none). Row `i` is span `i + 1` of trace
/// `[i % 8 + 1; 16]`.
fn ids_file(indexed: bool) -> Vec<u8> {
    let arena = bumpalo::Bump::new();
    let mut ri = crate::RowIndex::new(&arena, 10);
    let mut trace_ids = TraceIds::default();
    let mut span_ids = SpanIds::default();
    let mut parents = ParentSpanIds::default();
    let mut durations = Vec::new();
    let mut flags = Vec::new();
    let mut dropped = Vec::new();
    for i in 0..256u32 {
        let tokens = [
            format!("l={}", if i % 2 == 0 { "a" } else { "b" }),
            format!("m=v{:02}", i % 16),
            format!("h=w{:03}", i % 128),
        ];
        let slots: Vec<_> = tokens.iter().map(|kv| ri.intern(None, kv)).collect();
        ri.row(1_000 + i64::from(i), &slots);
        trace_ids.push(TraceId::from([(i % 8) as u8 + 1; 16]));
        span_ids.push(SpanId::from((u64::from(i) + 1).to_be_bytes()));
        parents.push(SpanId::UNSET);
        durations.push(i64::from(i) * 10);
        flags.push(i);
        dropped.push(0);
    }
    ri.trace_ids = Some(trace_ids);
    ri.span_ids = Some(span_ids);
    ri.parent_span_ids = Some(parents);
    ri.durations = Some(Durations(durations));
    ri.flags = Some(Flags(flags));
    ri.dropped_attribute_counts = Some(DroppedAttributeCounts(dropped));
    ri.build_trace_id_index = indexed;
    ri.build_trace_id_bloom = indexed;
    if indexed {
        ri.child_durations = Some(crate::ChildDurations((0..256).map(|i| i % 7).collect()));
    }
    let (buf, _summary, _meta) =
        crate::IndexWriter::write_into(&ri, std::io::Cursor::new(Vec::new()), Vec::new()).unwrap();
    buf.into_inner()
}

fn decoded_once(
    ids: &[chunk_file::ChunkId],
) -> std::collections::BTreeMap<chunk_file::ChunkId, u32> {
    ids.iter().map(|id| (*id, 1)).collect()
}

#[test]
fn memo_decodes_each_chunk_once() {
    let data = memo_file();
    let reader = IndexReader::open(&data).unwrap();
    assert_eq!(
        reader.decode_counts(),
        decoded_once(&[*b"SUMR", *b"META", *b"PRIM"])
    );

    let window = 0..i64::MAX;
    let grid = crate::Grid::new(1_000, 64, 4);
    let present = TraceId::from([1; 16]);
    let chips = crate::Filter::new().select("m", "v01").select("h", "w001");
    let filter = reader.compile_filter(&chips, None).unwrap();
    let text = reader
        .compile_text(&crate::text::LiteralText::new("w00"))
        .unwrap();
    let ids = reader
        .compile_trace_ids(&[present, TraceId::from([0xEE; 16])])
        .unwrap();
    for _ in 0..2 {
        for compiled in [&filter, &text, &ids] {
            reader.matched_count(compiled, window.clone()).unwrap();
        }
        reader.child_durations().unwrap();
        reader.row_values("m", 0..256).unwrap();
        reader.row_values("l", 10..20).unwrap();
        reader
            .compile_duration(crate::DurationRange {
                min_ns: Some(100),
                max_ns: None,
            })
            .unwrap();
        reader
            .compile_span_ids(&[SpanId::from(3u64.to_be_bytes())])
            .unwrap();
        reader.count_without(&filter, "m", window.clone()).unwrap();
        reader.count_absent(&filter, "h", window.clone()).unwrap();
    }
    reader.timeline("m", &filter, grid).unwrap();
    assert!(reader.timeline("h", &filter, grid).is_err());
    reader.timeline_totals(&filter, grid).unwrap();
    reader.facets(&["l", "m"], &filter, window.clone()).unwrap();
    let positions = reader.matched_positions(&text, window.clone()).unwrap();
    reader.durations().unwrap();
    reader.trace_ids().unwrap();
    reader.span_ids().unwrap();
    reader.materialize_fields(&["m", "h"], &positions).unwrap();
    reader.materialize_rows(&positions).unwrap();
    reader.trace_by_id(present).unwrap();
    reader.field_values("m").unwrap();
    reader.field_values_with_prefix("h", "w0", 5).unwrap();

    assert_eq!(
        reader.decode_counts(),
        decoded_once(&[
            *b"SUMR", *b"META", *b"PRIM", *b"TIMS", *b"TRCE", *b"SPAN", *b"PSPN", *b"DURN",
            *b"CHLD", *b"FLAG", *b"DRAC", *b"TIDX", *b"TBLM", *b"MF\0\0", *b"HF\0\0", *b"SB00",
        ])
    );
}

#[test]
fn compile_trace_ids_without_tidx_matches_scan() {
    let ids = [
        TraceId::from([3; 16]),
        TraceId::from([1; 16]),
        TraceId::from([0xEE; 16]),
    ];
    let positions = |indexed: bool| {
        let data = ids_file(indexed);
        let reader = IndexReader::open(&data).unwrap();
        assert_eq!(reader.has_trace_id_index(), indexed);
        let filter = reader.compile_trace_ids(&ids).unwrap();
        reader.matched_positions(&filter, 0..i64::MAX).unwrap()
    };
    let scanned = positions(false);
    let want: Vec<u32> = (0..256).filter(|i| i % 8 == 0 || i % 8 == 2).collect();
    assert_eq!(scanned, want);
    assert_eq!(scanned, positions(true), "the index agrees");
}

#[test]
fn compile_span_ids_selects_rows() {
    let data = ids_file(false);
    let reader = IndexReader::open(&data).unwrap();
    let span = |n: u64| SpanId::from(n.to_be_bytes());
    let filter = reader
        .compile_span_ids(&[span(200), span(5), span(5), span(999)])
        .unwrap();
    assert_eq!(
        reader.matched_positions(&filter, 0..i64::MAX).unwrap(),
        [4, 199]
    );
    let none = reader.compile_span_ids(&[]).unwrap();
    assert_eq!(reader.matched_count(&none, 0..i64::MAX).unwrap(), 0);
}

#[test]
fn ids_parse_from_their_hex_text() {
    let trace = "4BF92F3577B34DA6a3ce929d0e0e4736";
    assert_eq!(
        TraceId::from_hex(trace).map(|id| id.to_string()),
        Some(trace.to_lowercase())
    );
    assert_eq!(
        SpanId::from_hex("00f067aa0ba902b7"),
        Some(SpanId::from([
            0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7
        ]))
    );
    for bad in [
        "00f067aa0ba902b",
        "00f067aa0ba902b7a",
        "+0f067aa0ba902b7",
        "00f067aa0ba902bz",
        "",
    ] {
        assert_eq!(SpanId::from_hex(bad), None, "{bad:?}");
    }
    assert_eq!(TraceId::from_hex(&"0".repeat(32)), Some(TraceId::UNSET));
}

#[test]
fn trace_id_term_bloom_miss_decodes_nothing() {
    let data = memo_file();
    let reader = IndexReader::open(&data).unwrap();
    let bloom = reader.trace_id_bloom().unwrap();
    let miss = (0x10u8..=0xff)
        .map(|b| TraceId::from([b; 16]))
        .find(|id| !bloom.might_contain(*id))
        .expect("an id the bloom rejects");

    let filter = reader.compile_trace_ids(&[miss]).unwrap();
    assert_eq!(reader.matched_count(&filter, 0..i64::MAX).unwrap(), 0);
    let counts = reader.decode_counts();
    assert_eq!(counts.get(b"TBLM"), Some(&1));
    assert_eq!(counts.get(b"TIDX"), None);
    assert_eq!(counts.get(b"TRCE"), None);

    let filter = reader.compile_trace_ids(&[TraceId::from([1; 16])]).unwrap();
    assert_eq!(reader.matched_count(&filter, 0..i64::MAX).unwrap(), 32);
    let counts = reader.decode_counts();
    assert_eq!(
        (counts.get(b"TIDX"), counts.get(b"TRCE")),
        (Some(&1), Some(&1))
    );
}
