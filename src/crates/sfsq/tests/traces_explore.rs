//! The explorer engine (`sfsq::traces::explore`): the histogram stacked by a
//! field, window totals, and how every unreadable source is counted.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use sfsq::Source;
use sfsq::traces::explore::{
    ExploreData, ExploreOptions, ExploreQuery, ExploreScope, ExploreSelection, FacetSpec,
    GroupsDelta, HistogramData, HistogramSpec, RowDirection, RowOrder, RowsSpec, Sections,
    StackBucket, Totals, ValuesData, ValuesQuery, explore, field_values,
};
use sfsq::traces::{
    PartialReason, QueryStatus, ReasonCount, SourceId, TraceFailed, TraceSfstCandidate,
    TraceSource, WalCoverage,
};
use tokio_util::sync::CancellationToken;

const S: u64 = 1_000_000_000;

/// Ten one-second buckets from 0.
fn grid() -> sfst::Grid {
    sfst::Grid::new(0, S as i64, 10)
}

fn query(stack: &str, chips: &[(&str, &str)]) -> ExploreQuery {
    let mut filter = sfst::Filter::new();
    for (field, value) in chips {
        filter = filter.select(*field, *value);
    }
    ExploreQuery {
        grid: grid(),
        scope: ExploreScope {
            filter,
            text: None,
            trace_ids: Vec::new(),
        },
        selection: None,
        sections: Sections {
            histogram: Some(HistogramSpec {
                stack: stack.to_string(),
                percentiles: false,
            }),
            facets: None,
            groups: false,
            rows: None,
            fields: false,
        },
    }
}

fn entry_spans(stack: &str) -> ExploreQuery {
    query(stack, &[("_role", "root"), ("_role", "inbound")])
}

fn run(sources: Vec<TraceSource>, query: ExploreQuery) -> ExploreData {
    explore(
        sources,
        query,
        ExploreOptions::default(),
        CancellationToken::new(),
        Arc::new(AtomicUsize::new(0)),
    )
    .unwrap()
}

/// A request: a failing root, a served inbound span, an outbound call and an
/// internal step, at seconds 1, 2, 2 and 3.
fn request(trace: u8, id_base: u8) -> Vec<SpanSpec> {
    let span = |id: u8, parent: u8, second: u64, kind, status| SpanSpec {
        trace: [trace; 16],
        kind,
        status,
        ..sp(
            id_base + id,
            if parent == 0 { 0 } else { id_base + parent },
            second * S,
            "op",
        )
    };
    vec![
        span(1, 0, 1, 2, Some((2, "boom"))),
        span(2, 1, 2, 2, Some((1, ""))),
        span(3, 2, 2, 3, None),
        span(4, 2, 3, 1, None),
    ]
}

fn bucket(counts: &[u64], unset: u64) -> StackBucket {
    StackBucket {
        counts: counts.to_vec(),
        unset,
        other: 0,
        percentiles: None,
    }
}

/// The histogram of `request(..)` once, entry spans stacked by status.
fn one_request_histogram(times: u64) -> HistogramData {
    let mut buckets = vec![bucket(&[0, 0], 0); 10];
    buckets[1] = bucket(&[times, 0], 0);
    buckets[2] = bucket(&[0, times], 0);
    HistogramData {
        status: QueryStatus::Complete,
        stack: "status_code".to_string(),
        dimensions: vec!["ERROR".to_string(), "OK".to_string()],
        buckets,
        totals: Totals {
            count: 2 * times,
            errors: times,
            percentiles: None,
        },
        percentiles: false,
    }
}

fn partial(reasons: &[(PartialReason, u64, u64)]) -> QueryStatus {
    let mut map = std::collections::BTreeMap::new();
    for (reason, count, of) in reasons {
        map.insert(
            *reason,
            ReasonCount {
                count: *count,
                of: Some(*of),
                detail: Default::default(),
            },
        );
    }
    QueryStatus::Partial(map)
}

#[test]
fn entry_spans_are_counted_by_status() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let data = run(
        vec![sealed_source(dir.path(), &wal, "a")],
        entry_spans("status_code"),
    );
    assert_eq!(
        data,
        ExploreData {
            status: QueryStatus::Complete,
            sources: 1,
            histogram: Some(one_request_histogram(1)),
            facets: None,
            groups: None,
            rows: None,
            fields: None,
        }
    );

    // Every span, stacked by role: nothing lacks a role.
    let all = run(
        vec![sealed_source(dir.path(), &wal, "a")],
        query("_role", &[]),
    );
    let histogram = all.histogram.unwrap();
    assert_eq!(
        histogram.dimensions,
        ["inbound", "internal", "outbound", "root"]
    );
    assert_eq!(
        histogram.totals,
        Totals {
            count: 4,
            errors: 1,
            percentiles: None
        }
    );
    assert!(histogram.buckets.iter().all(|b| b.unset == 0));
}

/// Four bytes posing as an in-memory chunk over the first seconds.
fn garbage_source() -> TraceSource {
    TraceSource::Sfst(TraceSfstCandidate {
        source_id: SourceId::new("garbage".to_string()),
        summary: sfst::Summary {
            min_timestamp_s: 0,
            max_timestamp_s: 5,
            record_count: 3,
            content_meta: Vec::new(),
        },
        source: Source::Memory(Arc::new(vec![0; 4])),
        coverage: Some(WalCoverage {
            wal_id: "garbage.wal".into(),
            range: wal::FrameRange::new(wal::HEADER_SIZE as u64, wal::HEADER_SIZE as u64 + 4),
        }),
    })
}

#[test]
fn a_garbage_source_is_a_counted_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let data = run(
        vec![sealed_source(dir.path(), &wal, "a"), garbage_source()],
        entry_spans("status_code"),
    );
    let status = partial(&[(PartialReason::SourceFailure, 1, 2)]);
    assert_eq!(data.status, status);
    assert_eq!(
        data.histogram,
        Some(HistogramData {
            status,
            ..one_request_histogram(1)
        })
    );
}

#[test]
fn legacy_unavailable_and_failed_sources_are_named_with_counts() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let data = run(
        vec![
            sealed_source(dir.path(), &wal, "a"),
            legacy_sfst_source(dir.path(), "legacy"),
            unavailable_source("remote", 0, 5),
            unavailable_source("remote-old", 100, 200),
            TraceSource::Failed(TraceFailed {
                source_id: SourceId::new("refused.wal".to_string()),
                error: "chunk 0 build failed".to_string(),
            }),
        ],
        entry_spans("status_code"),
    );
    let status = partial(&[
        (PartialReason::SourceFailure, 1, 4),
        (PartialReason::RemoteUnavailable, 1, 4),
        (PartialReason::LegacyFile, 1, 4),
    ]);
    assert_eq!(data.status, status);
    assert_eq!(
        data.sources, 4,
        "the remote file outside the window is not a candidate"
    );
    assert_eq!(
        data.histogram,
        Some(HistogramData {
            status,
            ..one_request_histogram(1)
        })
    );
}

/// A file sealed after the explorer's entries but before the seal derived
/// values (`_role`, no `child_duration`) is named `legacy_file`: its error
/// origins and self time were never written. The same bytes as a chunk image
/// of a live WAL are read, and so is a sealed file whose rows have no error
/// (no `_err_origin` field, but the column).
#[test]
fn a_sealed_file_without_derived_values_is_legacy_and_a_chunk_image_is_not() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let (_, image) = ng_index::build_sfst_traces_range(&wal, whole_range(&wal)).unwrap();
    let intermediate = dir.path().join("intermediate.sfst");
    std::fs::write(&intermediate, image).unwrap();
    let mut no_errors = request(0x22, 0x20);
    for span in &mut no_errors {
        span.status = None;
    }
    let calm = write_wal(dir.path(), vec![req(&no_errors)], "calm");

    let data = run(
        vec![
            sealed_source_at(&intermediate, "intermediate"),
            memory_source(&wal, "chunk"),
            sealed_source(dir.path(), &calm, "calm"),
        ],
        entry_spans("status_code"),
    );

    let status = partial(&[(PartialReason::LegacyFile, 1, 3)]);
    assert_eq!(data.status, status);
    let histogram = data.histogram.unwrap();
    let counted: u64 = histogram
        .buckets
        .iter()
        .map(|bucket| bucket.counts.iter().sum::<u64>() + bucket.unset)
        .sum();
    assert_eq!(
        counted, 4,
        "two entry spans from the chunk, two from the calm file"
    );
}

#[test]
fn a_chip_on_a_field_absent_from_a_file_matches_nothing_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut tagged = request(0x11, 0x10);
    for span in &mut tagged {
        span.attrs.push(kv_str("tenant", "acme"));
    }
    let a = write_wal(dir.path(), vec![req(&tagged)], "a");
    let b = write_wal(dir.path(), vec![req(&request(0x22, 0x20))], "b");
    let data = run(
        vec![
            sealed_source(dir.path(), &a, "a"),
            sealed_source(dir.path(), &b, "b"),
        ],
        query(
            "status_code",
            &[
                ("_role", "root"),
                ("_role", "inbound"),
                ("attributes.tenant", "acme"),
            ],
        ),
    );
    assert_eq!(data.status, QueryStatus::Complete);
    assert_eq!(data.histogram, Some(one_request_histogram(1)));
}

#[test]
fn sources_combine_to_the_same_answer_in_any_order() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let b = write_wal(dir.path(), vec![req(&request(0x22, 0x20))], "b");
    let c = write_wal(dir.path(), vec![req(&request(0x33, 0x30))], "c");
    let sources = vec![
        sealed_source(dir.path(), &a, "a"),
        memory_source(&b, "b"),
        tail_source(&c, "c"),
    ];
    let forward = run(sources.clone(), entry_spans("status_code"));
    let reversed = run(
        sources.into_iter().rev().collect(),
        entry_spans("status_code"),
    );
    assert_eq!(forward, reversed);
    assert_eq!(forward.histogram, Some(one_request_histogram(3)));
    assert_eq!(forward.sources, 3);
}

/// The live tail is evaluated through an image built with the seal's own
/// builder, so it answers exactly like a chunk image and a sealed file —
/// including the event tokens a row scan of the tail never carried.
#[test]
fn tail_chunk_and_sealed_file_agree() {
    let dir = tempfile::tempdir().unwrap();
    let mut spans = request(0x11, 0x10);
    spans[0]
        .events
        .push(("exception", vec![kv_str("exception.type", "Boom")]));
    spans[2].events.push(("retry", Vec::new()));
    let wal = write_wal(dir.path(), vec![req(&spans)], "a");
    for stack in ["status_code", "events.name", "_duration_band"] {
        let sealed = run(
            vec![sealed_source(dir.path(), &wal, "s")],
            query(stack, &[]),
        );
        let chunk = run(vec![memory_source(&wal, "m")], query(stack, &[]));
        let tail = run(vec![tail_source(&wal, "t")], query(stack, &[]));
        assert_eq!(chunk, sealed, "{stack}");
        assert_eq!(tail, sealed, "{stack}");
    }
    let events = run(vec![tail_source(&wal, "t")], query("events.name", &[]));
    assert_eq!(events.histogram.unwrap().dimensions, ["exception", "retry"]);
}

/// Rows, the field list and value suggestions count the same unreadable
/// sources as the histogram, and answer from the readable one.
#[test]
fn rows_fields_and_values_count_unreadable_sources_too() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let sources = || {
        vec![
            sealed_source(dir.path(), &wal, "a"),
            legacy_sfst_source(dir.path(), "legacy"),
            unavailable_source("remote", 0, 5),
            TraceSource::Failed(TraceFailed {
                source_id: SourceId::new("refused.wal".to_string()),
                error: "chunk 0 build failed".to_string(),
            }),
        ]
    };
    let status = partial(&[
        (PartialReason::SourceFailure, 1, 4),
        (PartialReason::RemoteUnavailable, 1, 4),
        (PartialReason::LegacyFile, 1, 4),
    ]);
    let mut query = entry_spans("status_code");
    query.sections.histogram = None;
    query.sections.fields = true;
    query.sections.rows = Some(RowsSpec {
        order: RowOrder::Newest {
            anchor: None,
            direction: RowDirection::Older,
        },
        limit: 10,
        columns: Vec::new(),
    });
    let data = run(sources(), query);
    let rows = data.rows.expect("rows");
    assert_eq!(rows.status, status);
    assert_eq!(rows.items.len() as u64, rows.matched);
    assert!(rows.matched > 0);
    let fields = data.fields.expect("fields");
    assert_eq!(fields.status, status);
    assert!(fields.items.iter().any(|f| f.name == "name"));

    let values = field_values(
        sources(),
        ValuesQuery {
            window: 0..10 * S as i64,
            field: "name".to_string(),
            prefix: String::new(),
            limit: 100,
        },
        ExploreOptions::default(),
        CancellationToken::new(),
        Arc::new(AtomicUsize::new(0)),
    )
    .unwrap();
    assert_eq!(values.status, status);
    assert!(!values.values.is_empty() && !values.truncated);
}

/// Value suggestions are all or nothing: a cancelled call answers no values.
#[test]
fn a_cancelled_values_request_answers_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let progress = Arc::new(AtomicUsize::new(0));
    let values = field_values(
        vec![sealed_source(dir.path(), &wal, "a")],
        ValuesQuery {
            window: 0..10 * S as i64,
            field: "name".to_string(),
            prefix: String::new(),
            limit: 100,
        },
        ExploreOptions::default(),
        cancel,
        Arc::clone(&progress),
    )
    .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), 0);
    assert!(values.values.is_empty() && !values.truncated);
    assert!(values.status.has(PartialReason::Cancelled));
}

fn origin_values(sources: Vec<TraceSource>) -> ValuesData {
    field_values(
        sources,
        ValuesQuery {
            window: 0..10 * S as i64,
            field: sfst::ERR_ORIGIN_FIELD.to_string(),
            prefix: String::new(),
            limit: 10,
        },
        ExploreOptions::default(),
        CancellationToken::new(),
        Arc::new(AtomicUsize::new(0)),
    )
    .unwrap()
}

/// A live WAL's error origins exist only through the live pass: value
/// suggestions list them as the sealed file will, and say when the pass
/// could not run.
#[test]
fn error_origin_values_of_a_live_wal() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(
        dir.path(),
        vec![req(&request(0x11, 0x10)), req(&request(0x12, 0x20))],
        "live",
    );
    let sealed = origin_values(vec![sealed_source(dir.path(), &wal, "sealed")]);
    assert!(sealed.status.is_complete());
    assert!(!sealed.values.is_empty());
    assert_eq!(
        origin_values(vec![tail_source(&wal, "live#tail")]),
        sealed,
        "the live WAL answers as its sealed file"
    );

    let whole = whole_range(&wal);
    let frames = wal::scan_frame_boundaries(&wal, whole).unwrap();
    let broken = TraceSource::Tail(sfsq::traces::TraceWalTail {
        source_id: SourceId::new("live#tail".to_string()),
        path: wal.clone(),
        coverage: WalCoverage {
            wal_id: wal.display().to_string().into(),
            range: wal::FrameRange::new(frames[0].end_offset, whole.end()),
        },
    });
    let values = origin_values(vec![broken]);
    assert_eq!(
        values.status,
        partial(&[(PartialReason::LivePassFailed, 1, 1)])
    );
    assert!(values.values.is_empty());
}

#[test]
fn a_cancelled_request_evaluates_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let cancel = CancellationToken::new();
    cancel.cancel();
    let progress = Arc::new(AtomicUsize::new(0));
    let data = explore(
        vec![sealed_source(dir.path(), &wal, "a")],
        entry_spans("status_code"),
        ExploreOptions::default(),
        cancel,
        Arc::clone(&progress),
    )
    .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), 0);
    assert_eq!(data.histogram, None);
    assert!(data.status.has(PartialReason::Cancelled));
}

#[test]
fn progress_ticks_once_per_source() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let progress = Arc::new(AtomicUsize::new(0));
    explore(
        vec![
            sealed_source(dir.path(), &wal, "a"),
            unavailable_source("remote-old", 100, 200),
        ],
        entry_spans("status_code"),
        ExploreOptions::default(),
        CancellationToken::new(),
        Arc::clone(&progress),
    )
    .unwrap();
    assert_eq!(progress.load(Ordering::Relaxed), 2);
}

fn facets_query(chips: &[(&str, &str)], fields: Option<&[&str]>) -> ExploreQuery {
    let mut q = query("status_code", chips);
    q.sections.histogram = None;
    q.sections.facets = Some(FacetSpec {
        fields: fields.map(|f| f.iter().map(|s| s.to_string()).collect()),
    });
    q
}

fn facet_values(data: &ExploreData, field: &str) -> Vec<(Option<String>, u64)> {
    let facets = data.facets.as_ref().expect("facets section");
    let facet = facets
        .fields
        .iter()
        .find(|f| f.field == field)
        .unwrap_or_else(|| panic!("no facet {field}"));
    facet
        .values
        .iter()
        .map(|v| (v.value.clone(), v.count))
        .collect()
}

/// Named facet values with their counts.
fn pairs(items: &[(&str, u64)]) -> Vec<(Option<String>, u64)> {
    items
        .iter()
        .map(|(v, c)| (Some(v.to_string()), *c))
        .collect()
}

#[test]
fn facets_count_scope_rows_and_ignore_the_fields_own_chips() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let b = write_wal(dir.path(), vec![req(&request(0x22, 0x20))], "b");
    let data = run(
        vec![
            sealed_source(dir.path(), &a, "a"),
            sealed_source(dir.path(), &b, "b"),
        ],
        facets_query(
            &[("_role", "root"), ("_role", "inbound")],
            Some(&["_role", "status_code", "kind", "absent.field"]),
        ),
    );
    assert_eq!(data.status, QueryStatus::Complete);
    assert_eq!(
        facet_values(&data, "_role"),
        pairs(&[
            ("inbound", 2),
            ("internal", 2),
            ("outbound", 2),
            ("root", 2)
        ]),
        "the role facet is not narrowed by the role chip"
    );
    assert_eq!(
        facet_values(&data, "status_code"),
        pairs(&[("ERROR", 2), ("OK", 2)])
    );
    assert_eq!(facet_values(&data, "kind"), pairs(&[("SERVER", 4)]));
    assert_eq!(facet_values(&data, "absent.field"), pairs(&[]));
    let facets = data.facets.unwrap();
    assert_eq!(
        facets
            .fields
            .iter()
            .map(|f| f.field.as_str())
            .collect::<Vec<_>>(),
        ["_role", "status_code", "kind", "absent.field"],
        "requested fields keep the request order"
    );
}

/// The status facet lists the rows without a status as the unset value,
/// last: a file without the field counts every row; the values and the
/// unset value add up to the scope; other facets list no unset value.
#[test]
fn the_status_facet_counts_unset_rows() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let b = write_wal(dir.path(), vec![req(&request(0x22, 0x20))], "b");
    let c = write_wal(
        dir.path(),
        vec![req(&[sp(1, 0, S, "x"), sp(2, 1, S, "y")])],
        "c",
    );
    let mut q = facets_query(&[], Some(&["status_code", "kind"]));
    q.sections.histogram = Some(HistogramSpec {
        stack: "status_code".to_string(),
        percentiles: false,
    });
    let data = run(
        vec![
            sealed_source(dir.path(), &a, "a"),
            sealed_source(dir.path(), &b, "b"),
            memory_source(&c, "c"),
        ],
        q,
    );
    assert!(data.status.is_complete(), "{:?}", data.status);
    let mut statuses = pairs(&[("ERROR", 2), ("OK", 2)]);
    statuses.push((None, 6));
    assert_eq!(facet_values(&data, "status_code"), statuses);
    let listed: u64 = statuses.iter().map(|(_, count)| count).sum();
    assert_eq!(listed, data.histogram.as_ref().unwrap().totals.count);
    assert!(
        facet_values(&data, "kind")
            .iter()
            .all(|(value, _)| value.is_some()),
        "only the status facet lists an unset value"
    );

    let errors = run(
        vec![sealed_source(dir.path(), &a, "a")],
        facets_query(&[("status_code", "ERROR")], Some(&["status_code"])),
    );
    let mut own_chip_dropped = pairs(&[("ERROR", 1), ("OK", 1)]);
    own_chip_dropped.push((None, 2));
    assert_eq!(
        facet_values(&errors, "status_code"),
        own_chip_dropped,
        "the status chip does not narrow its own facet, unset included"
    );
}

#[test]
fn default_facets_are_every_visible_field() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let data = run(
        vec![sealed_source(dir.path(), &a, "a")],
        facets_query(&[], None),
    );
    let fields: Vec<String> = data
        .facets
        .unwrap()
        .fields
        .into_iter()
        .map(|f| f.field)
        .collect();
    for hidden in ["_kind", "_status_code"] {
        assert!(!fields.iter().any(|f| f == hidden), "{hidden} is hidden");
    }
    for visible in [
        "_duration_band",
        "_role",
        "kind",
        "name",
        "resource.attributes.service.name",
        "status_code",
    ] {
        assert!(
            fields.iter().any(|f| f == visible),
            "{visible} missing from {fields:?}"
        );
    }
}

/// `n` spans of one trace, each with a distinct `attributes.id`.
fn distinct_ids(trace: u8, n: u32, prefix: &str) -> Vec<SpanSpec> {
    let mut spans = Vec::new();
    for i in 0..n {
        spans.push(SpanSpec {
            trace: [trace; 16],
            id: [
                (i % 250) as u8 + 1,
                (i / 250) as u8 + 1,
                0,
                0,
                0,
                0,
                0,
                trace,
            ],
            attrs: vec![kv_str("id", &format!("{prefix}{i:05}"))],
            ..sp(1, 0, S + u64::from(i), "op")
        });
    }
    spans
}

/// An absent chip keeps the rows without the field: none of a file where
/// every row has it (high-cardinality there), all of a file without it, and
/// ORs with the field's values.
#[test]
fn an_absent_chip_keeps_rows_without_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let wide = write_wal(
        dir.path(),
        vec![req(&distinct_ids(0x60, 1_100, "a"))],
        "wide",
    );
    let plain = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "plain");
    let sources = || {
        vec![
            sealed_source(dir.path(), &wide, "wide"),
            sealed_source(dir.path(), &plain, "plain"),
        ]
    };
    let count = |filter: sfst::Filter| {
        let mut q = query("status_code", &[]);
        q.scope.filter = filter;
        let data = run(sources(), q);
        assert!(data.status.is_complete(), "{:?}", data.status);
        data.histogram.unwrap().totals.count
    };
    assert_eq!(count(sfst::Filter::new().select_absent("attributes.id")), 4);
    assert_eq!(
        count(
            sfst::Filter::new()
                .select("attributes.id", "a00007")
                .select_absent("attributes.id")
        ),
        5
    );
    assert_eq!(
        count(sfst::Filter::new().select("attributes.id", "a00007")),
        1
    );
}

#[test]
fn a_high_cardinality_facet_is_named_and_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let wide = write_wal(
        dir.path(),
        vec![req(&distinct_ids(0x31, 1_100, "w"))],
        "wide",
    );
    let sources = || vec![sealed_source(dir.path(), &wide, "wide")];

    let asked = run(
        sources(),
        facets_query(&[], Some(&["attributes.id", "name"])),
    );
    let facets = asked.facets.as_ref().unwrap();
    assert_eq!(
        facets.unavailable,
        [("attributes.id".to_string(), PartialReason::FacetHighCard)]
    );
    assert_eq!(
        facets
            .fields
            .iter()
            .map(|f| f.field.as_str())
            .collect::<Vec<_>>(),
        ["name"]
    );
    let reason = asked.status.count(PartialReason::FacetHighCard).unwrap();
    assert_eq!(reason.count, 1);
    assert!(reason.detail.contains("attributes.id"));

    let default = run(sources(), facets_query(&[], None));
    assert!(
        default.status.is_complete(),
        "a default facet set leaves high fields out by definition"
    );
    assert!(
        !default
            .facets
            .unwrap()
            .fields
            .iter()
            .any(|f| f.field == "attributes.id")
    );
}

#[test]
fn a_facet_over_the_value_cap_says_what_it_left_out() {
    let dir = tempfile::tempdir().unwrap();
    let a = write_wal(dir.path(), vec![req(&distinct_ids(0x41, 600, "a"))], "a");
    let b = write_wal(dir.path(), vec![req(&distinct_ids(0x42, 600, "b"))], "b");
    let data = run(
        vec![
            sealed_source(dir.path(), &a, "a"),
            sealed_source(dir.path(), &b, "b"),
        ],
        facets_query(&[], Some(&["attributes.id"])),
    );
    let facets = data.facets.as_ref().unwrap();
    let facet = &facets.fields[0];
    assert_eq!(facet.values.len(), 1_000);
    assert_eq!((facet.omitted_values, facet.omitted_rows), (200, 200));
    let reason = data.status.count(PartialReason::FacetValueCap).unwrap();
    assert_eq!(reason.count, 1);
    assert!(reason.detail.contains("attributes.id"));
    assert_eq!(facets.status, data.status);
}

/// About twenty sources of every kind: sealed files (the first two hold rows
/// with the same keys but different operation names, so ties across sources
/// are visible), chunk images, tails, a file with a high-cardinality field, a
/// file whose high-cardinality chunk is corrupt, and garbage, legacy,
/// unavailable, missing and failed sources.
fn mixed_sources(dir: &std::path::Path) -> Vec<TraceSource> {
    let mut sources = Vec::new();
    for i in 0..8u8 {
        let mut spans = request(if i < 2 { 0x11 } else { 0x20 + i }, 0x10);
        if i == 1 {
            for span in &mut spans {
                span.name = "collide";
            }
        }
        let name = format!("sealed{i}");
        let wal = write_wal(dir, vec![req(&spans)], &name);
        sources.push(sealed_source(dir, &wal, &name));
    }
    for i in 0..5u8 {
        let name = format!("chunk{i}");
        let wal = write_wal(dir, vec![req(&request(0x40 + i, 0x10))], &name);
        sources.push(memory_source(&wal, &name));
    }
    for i in 0..3u8 {
        let name = format!("tail{i}");
        let wal = write_wal(dir, vec![req(&request(0x50 + i, 0x10))], &name);
        sources.push(tail_source(&wal, &name));
    }
    let wide = write_wal(dir, vec![req(&distinct_ids(0x60, 1_100, "a"))], "wide");
    sources.push(sealed_source(dir, &wide, "wide"));
    let broken_wal = write_wal(dir, vec![req(&distinct_ids(0x61, 1_100, "b"))], "broken");
    let broken = dir.join("broken.sfst");
    ng_index::build_sfst_traces_file(&broken_wal, &broken, &ng_index::Metrics::new()).unwrap();
    corrupt_chunk(&broken, *b"HF\0\0");
    sources.push(sealed_source_at(&broken, "broken"));
    sources.push(garbage_source());
    sources.push(legacy_sfst_source(dir, "legacy"));
    sources.push(unavailable_source("remote", 0, 5));
    sources.push(missing_source(dir, "gone", 0, 5));
    sources.push(TraceSource::Failed(TraceFailed {
        source_id: SourceId::new("failed".to_string()),
        error: "boom".to_string(),
    }));
    sources
}

fn newest(anchor: Option<sfsq::traces::explore::RowKey>, direction: RowDirection) -> RowOrder {
    RowOrder::Newest { anchor, direction }
}

/// Every section, with percentiles and an `attributes.id` column on the rows.
fn every_section(stack: &str, order: RowOrder, limit: usize) -> ExploreQuery {
    let mut query = entry_spans(stack);
    query.sections.histogram = Some(HistogramSpec {
        stack: stack.to_string(),
        percentiles: true,
    });
    query.sections.facets = Some(FacetSpec { fields: None });
    query.sections.groups = true;
    query.sections.fields = true;
    query.sections.rows = Some(RowsSpec {
        order,
        limit,
        columns: vec!["attributes.id".to_string()],
    });
    query
}

/// QRY-37: however the sources are ordered and split among workers, every
/// section of the answer is the same: 24 seeded shuffles of the mixed sources
/// (sealed files, chunk images, tails, a high-cardinality and a broken file),
/// each on its own worker count, against one sequential answer. The two files
/// holding the same span keys keep their relative order: rows with equal keys
/// are ordered by where they are listed, and the capture lists files in
/// sequence order.
#[test]
fn explore_shard_merge_split_many_ways() {
    let dir = tempfile::tempdir().unwrap();
    let sources = mixed_sources(dir.path());
    let answer = |sources: Vec<TraceSource>, query: ExploreQuery, workers: usize| {
        explore(
            sources,
            query,
            ExploreOptions { workers },
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        )
        .unwrap()
    };
    let queries = || {
        [
            every_section("status_code", newest(None, RowDirection::Older), 4),
            every_section("_duration_band", RowOrder::Slowest, 4),
        ]
    };
    let baseline: Vec<_> = queries()
        .into_iter()
        .map(|query| answer(sources.clone(), query, 1))
        .collect();

    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = |bound: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % bound as u64) as usize
    };
    for round in 0..24 {
        let mut shuffled = sources.clone();
        for at in (1..shuffled.len()).rev() {
            shuffled.swap(at, next(at + 1));
        }
        let position = |name: &str| {
            shuffled
                .iter()
                .position(|source| source.source_id().as_str() == name)
                .unwrap()
        };
        let (first, second) = (position("sealed0"), position("sealed1"));
        if first > second {
            shuffled.swap(first, second);
        }
        let workers = [1, 2, 3, 5, 8, 16][round % 6];
        for (query, want) in queries().into_iter().zip(&baseline) {
            let got = answer(shuffled.clone(), query, workers);
            assert_eq!(&got, want, "round {round}, {workers} workers");
        }
    }
}

#[test]
fn explore_parallel_equals_sequential() {
    let dir = tempfile::tempdir().unwrap();
    let sources = mixed_sources(dir.path());
    let answer = |query: ExploreQuery, workers: usize| {
        let progress = Arc::new(AtomicUsize::new(0));
        let data = explore(
            sources.clone(),
            query,
            ExploreOptions { workers },
            CancellationToken::new(),
            Arc::clone(&progress),
        )
        .unwrap();
        let passes = if data.groups.is_some() { 2 } else { 1 };
        assert_eq!(progress.load(Ordering::Relaxed), passes * sources.len());
        data
    };

    let first = answer(
        every_section("status_code", newest(None, RowDirection::Older), 3),
        1,
    );
    assert!(!first.status.is_complete());
    let page = &first.rows.as_ref().expect("rows").items;
    let (head, last) = (page[0].key, page[page.len() - 1].key);

    type Case<'a> = (&'a str, Box<dyn Fn() -> ExploreQuery>);
    let cases: Vec<Case> = vec![
        (
            "newest",
            Box::new(|| every_section("status_code", newest(None, RowDirection::Older), 3)),
        ),
        (
            "older page",
            Box::new(move || {
                every_section("status_code", newest(Some(last), RowDirection::Older), 3)
            }),
        ),
        (
            "newer page",
            Box::new(move || {
                every_section("status_code", newest(Some(head), RowDirection::Newer), 3)
            }),
        ),
        (
            "slowest",
            Box::new(|| every_section("status_code", RowOrder::Slowest, 5)),
        ),
        (
            "high-cardinality stack",
            Box::new(|| every_section("attributes.id", newest(None, RowDirection::Older), 3)),
        ),
        (
            "text",
            Box::new(|| {
                let mut query = every_section("status_code", RowOrder::Slowest, 5);
                query.scope.text = Some(sfst::text::LiteralText::new("a00"));
                query
            }),
        ),
        (
            "selection",
            Box::new(|| {
                let mut query = every_section("status_code", newest(None, RowDirection::Older), 3);
                query.selection = Some(ExploreSelection {
                    filter: sfst::Filter::new(),
                    duration: Some(sfst::DurationRange {
                        min_ns: Some(0),
                        max_ns: None,
                    }),
                    time_ns: None,
                });
                query
            }),
        ),
        (
            "selection across files",
            Box::new(|| {
                let mut query = every_section("status_code", newest(None, RowDirection::Older), 3);
                query.selection = selection(sfst::Filter::new().select("name", "collide"));
                query
            }),
        ),
        (
            "trace ids",
            Box::new(|| {
                let mut query = every_section("status_code", newest(None, RowDirection::Older), 3);
                query.scope.trace_ids = vec![
                    sfst::TraceId::from([0x11; 16]),
                    sfst::TraceId::from([0x60; 16]),
                ];
                query
            }),
        ),
    ];
    for (name, query) in &cases {
        let sequential = answer(query(), 1);
        for workers in [2, 4, 7] {
            assert_eq!(
                answer(query(), workers),
                sequential,
                "{name}, {workers} workers"
            );
        }
        for _ in 0..10 {
            assert_eq!(answer(query(), 4), sequential, "{name}, 4 workers again");
        }
    }

    let values = |field: &str, prefix: &str, workers: usize| {
        field_values(
            sources.clone(),
            ValuesQuery {
                window: 0..10 * S as i64,
                field: field.to_string(),
                prefix: prefix.to_string(),
                limit: 3,
            },
            ExploreOptions { workers },
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        )
        .unwrap()
    };
    for (field, prefix) in [
        ("attributes.id", ""),
        ("attributes.id", "b0"),
        ("name", "o"),
    ] {
        let sequential = values(field, prefix, 1);
        for workers in [2, 4, 7] {
            assert_eq!(
                values(field, prefix, workers),
                sequential,
                "{field} {prefix}"
            );
        }
    }
}

#[test]
fn a_scope_names_at_most_the_trace_id_limit() {
    let mut query = entry_spans("status_code");
    query.scope.trace_ids = (0..=sfsq::traces::explore::TRACE_IDS_MAX)
        .map(|i| sfst::TraceId::from([(i % 250) as u8 + 1; 16]))
        .collect();
    let refused = explore(
        Vec::new(),
        query,
        ExploreOptions::default(),
        CancellationToken::new(),
        Arc::new(AtomicUsize::new(0)),
    );
    assert!(matches!(
        refused,
        Err(sfsq::traces::explore::ExploreRequestError::Invalid(_))
    ));
}
/// A span of `trace` named `name`, of `kind`, starting at `second`, ERROR
/// when `error`.
fn group_span(
    trace: u8,
    id: u8,
    parent: u8,
    second: u64,
    name: &'static str,
    kind: i32,
    error: bool,
) -> SpanSpec {
    SpanSpec {
        trace: [trace; 16],
        kind,
        status: error.then_some((2, "boom")),
        ..sp(id, parent, second * S, name)
    }
}

/// `(service, operation, spans, errors, errors originated, self ns)` per
/// group, in the section's order.
fn group_numbers(data: &ExploreData) -> Vec<GroupLine> {
    let groups = data.groups.as_ref().expect("groups section");
    let mut out = Vec::new();
    for row in &groups.rows {
        assert!(row.numbers.p95_ns.is_some());
        out.push((
            row.key.service.clone(),
            row.key.operation.clone(),
            row.numbers.spans,
            row.numbers.errors,
            row.numbers.errors_originated,
            row.numbers.self_ns,
        ));
    }
    out
}

fn groups_query(chips: &[(&str, &str)]) -> ExploreQuery {
    let mut q = query("status_code", chips);
    q.sections.histogram = None;
    q.sections.groups = true;
    q
}

type GroupLine = (Option<String>, Option<String>, u64, u64, u64, u128);

/// A group of service `svc`: its operation, spans, errors, errors
/// originated and self time.
fn svc(operation: &str, spans: u64, errors: u64, origins: u64, self_ns: u128) -> GroupLine {
    (
        Some("svc".to_string()),
        Some(operation.to_string()),
        spans,
        errors,
        origins,
        self_ns,
    )
}

/// Trace 1 over two sealed files, a chunk image and a tail: its `checkout`
/// root (an error with no erroring child) in `a`, three `op` spans in the
/// others. Trace 2: two `op` spans, in `b` and in the tail. Every span lasts
/// 50 ns and no child overlaps its parent, so each has 50 ns of self time.
fn split_traces(dir: &std::path::Path) -> Vec<TraceSource> {
    let a = write_wal(
        dir,
        vec![req(&[group_span(1, 1, 0, 1, "checkout", 2, true)])],
        "a",
    );
    let b = write_wal(
        dir,
        vec![req(&[
            group_span(1, 2, 1, 2, "op", 2, false),
            group_span(2, 9, 0, 2, "op", 2, false),
        ])],
        "b",
    );
    let c = write_wal(
        dir,
        vec![req(&[group_span(1, 3, 2, 2, "op", 3, false)])],
        "c",
    );
    let d = write_wal(
        dir,
        vec![req(&[
            group_span(1, 4, 2, 3, "op", 1, false),
            group_span(2, 8, 9, 3, "op", 1, false),
        ])],
        "d",
    );
    vec![
        sealed_source(dir, &a, "a"),
        sealed_source(dir, &b, "b"),
        memory_source(&c, "c"),
        tail_source(&d, "d"),
    ]
}

/// QRY-46: one trace whose spans sit in two sealed files, a chunk image and
/// a tail is counted once, every span of it, whatever its role; a trace with
/// no span in scope is not.
#[test]
fn trace_join_across_files() {
    let dir = tempfile::tempdir().unwrap();
    let sources = split_traces(dir.path());
    let progress = Arc::new(AtomicUsize::new(0));
    let data = explore(
        sources,
        groups_query(&[("name", "checkout")]),
        ExploreOptions::default(),
        CancellationToken::new(),
        Arc::clone(&progress),
    )
    .unwrap();

    assert!(data.status.is_complete(), "{:?}", data.status);
    let groups = data.groups.as_ref().unwrap();
    assert!(groups.status.is_complete(), "{:?}", groups.status);
    assert_eq!(groups.window_s, 10);
    assert_eq!(
        group_numbers(&data),
        vec![svc("op", 3, 0, 0, 150), svc("checkout", 1, 1, 1, 50)]
    );
    assert_eq!(groups.self_ns_total, 200);
    assert!(groups.other.is_none());
    assert!(groups.delta.is_none() && groups.rows.iter().all(|row| row.delta.is_none()));
    assert_eq!(
        progress.load(Ordering::Relaxed),
        8,
        "two passes over four sources"
    );
}

/// `(service, operation, selection side, baseline side)` per group, each
/// side as `(spans, errors originated, self ns)`.
fn group_sides(data: &ExploreData) -> Vec<(Option<String>, Option<String>, SideLine, SideLine)> {
    let groups = data.groups.as_ref().expect("groups section");
    let side = |side: &sfsq::traces::explore::SideNumbers| {
        (side.spans, side.errors_originated, side.self_ns)
    };
    let mut out = Vec::new();
    for row in &groups.rows {
        let delta = row.delta.expect("sides under a selection");
        assert_eq!(
            delta.selection.spans + delta.baseline.spans,
            row.numbers.spans
        );
        out.push((
            row.key.service.clone(),
            row.key.operation.clone(),
            side(&delta.selection),
            side(&delta.baseline),
        ));
    }
    out
}

type SideLine = (u64, u64, u128);

/// QRY-20: a trace is on the selection side when any of its scope rows is a
/// selection row, and all its window rows follow it into every group, in
/// whichever file they sit; the other scope traces make the baseline.
#[test]
fn delta_sides_follow_the_trace_across_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut q = groups_query(&[]);
    q.selection = selection(sfst::Filter::new().select("name", "checkout"));
    let data = run(split_traces(dir.path()), q);

    let groups = data.groups.as_ref().unwrap();
    assert!(groups.status.is_complete(), "{:?}", groups.status);
    let svc = |name: &str| (Some("svc".to_string()), Some(name.to_string()));
    let line = |(service, operation): (Option<String>, Option<String>), selection, baseline| {
        (service, operation, selection, baseline)
    };
    assert_eq!(
        group_sides(&data),
        vec![
            line(svc("op"), (3, 0, 150), (2, 0, 100)),
            line(svc("checkout"), (1, 1, 50), (0, 0, 0)),
        ]
    );
    assert_eq!(
        groups.delta,
        Some(GroupsDelta {
            selection_traces: 1,
            baseline_traces: 1,
            selection_self_ns_total: 200,
            baseline_self_ns_total: 100,
        })
    );

    let mut nothing = groups_query(&[]);
    nothing.selection = selection(sfst::Filter::new().select("name", "absent"));
    let data = run(split_traces(dir.path()), nothing);
    assert_eq!(
        data.groups.unwrap().delta,
        Some(GroupsDelta {
            selection_traces: 0,
            baseline_traces: 2,
            selection_self_ns_total: 0,
            baseline_self_ns_total: 300,
        }),
        "a selection without rows leaves every trace on the baseline"
    );
}

/// D41: a window row of a scope trace counts whatever its role; a row past
/// the window does not; a scope row without a trace id counts as itself and
/// one outside the scope does not. A file the first pass set aside (legacy)
/// is not read again.
#[test]
fn groups_count_every_span_of_the_scope_traces() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(
        dir.path(),
        vec![req(&[
            group_span(1, 1, 0, 1, "checkout", 2, false),
            group_span(1, 2, 1, 2, "op", 3, true),
            group_span(1, 3, 1, 12, "late", 1, false),
            group_span(0, 4, 0, 3, "checkout", 2, false),
            group_span(0, 5, 0, 3, "op", 2, false),
            group_span(2, 6, 0, 4, "op", 2, false),
        ])],
        "a",
    );
    let data = run(
        vec![
            sealed_source(dir.path(), &wal, "a"),
            legacy_sfst_source(dir.path(), "old"),
        ],
        groups_query(&[("name", "checkout")]),
    );
    let groups = data.groups.as_ref().unwrap();
    assert_eq!(groups.status, partial(&[(PartialReason::LegacyFile, 1, 2)]));
    assert_eq!(
        group_numbers(&data),
        vec![svc("checkout", 2, 0, 0, 100), svc("op", 1, 1, 1, 50)]
    );
}

/// QRY-20 with D41: a scope row without a trace id is its own trace, on the
/// selection side when it is a selection row.
#[test]
fn delta_counts_an_unset_trace_row_as_its_own_trace() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(
        dir.path(),
        vec![req(&[
            group_span(1, 1, 0, 1, "checkout", 2, false),
            group_span(1, 2, 1, 2, "op", 3, true),
            group_span(0, 4, 0, 3, "checkout", 2, false),
            group_span(0, 5, 0, 3, "op", 2, false),
            group_span(2, 6, 0, 4, "op", 2, false),
        ])],
        "a",
    );
    let mut q = groups_query(&[]);
    q.selection = selection(sfst::Filter::new().select("name", "checkout"));
    let data = run(vec![sealed_source(dir.path(), &wal, "a")], q);

    let svc = |name: &str| (Some("svc".to_string()), Some(name.to_string()));
    let line = |(service, operation): (Option<String>, Option<String>), selection, baseline| {
        (service, operation, selection, baseline)
    };
    assert_eq!(
        group_sides(&data),
        vec![
            line(svc("op"), (1, 1, 50), (2, 0, 100)),
            line(svc("checkout"), (2, 0, 100), (0, 0, 0)),
        ]
    );
    assert_eq!(
        data.groups.unwrap().delta,
        Some(GroupsDelta {
            selection_traces: 2,
            baseline_traces: 2,
            selection_self_ns_total: 150,
            baseline_self_ns_total: 100,
        })
    );
}

/// A source whose second pass fails is named on the Groups section only:
/// the sections of the first pass still count it.
#[test]
fn groups_name_a_source_that_fails_the_second_pass() {
    let dir = tempfile::tempdir().unwrap();
    let good = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "good");
    let bad = write_wal(dir.path(), vec![req(&request(0x12, 0x10))], "bad");
    let bad_path = dir.path().join("bad.sfst");
    ng_index::build_sfst_traces_file(&bad, &bad_path, &ng_index::Metrics::new()).unwrap();
    corrupt_chunk(&bad_path, *b"CHLD");
    let mut q = entry_spans("status_code");
    q.sections.groups = true;
    let data = run(
        vec![
            sealed_source(dir.path(), &good, "good"),
            sealed_source_at(&bad_path, "bad"),
        ],
        q,
    );
    let histogram = data.histogram.as_ref().unwrap();
    assert!(histogram.status.is_complete(), "{:?}", histogram.status);
    assert_eq!(histogram.totals.count, 4, "both files in the first pass");
    let groups = data.groups.as_ref().unwrap();
    assert_eq!(
        groups.status,
        partial(&[(PartialReason::SourceFailure, 1, 2)])
    );
    assert_eq!(data.status, groups.status);
    let spans: u64 = groups.rows.iter().map(|row| row.numbers.spans).sum();
    assert_eq!(spans, 4, "the good file's trace, every span");
}

/// A source that fails both the Groups pass and the Rows page is one failed
/// source in the request's status, whichever sections name it.
#[test]
fn a_source_failing_groups_and_rows_counts_once() {
    let dir = tempfile::tempdir().unwrap();
    let good = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "good");
    let bad = write_wal(dir.path(), vec![req(&request(0x12, 0x10))], "bad");
    let bad_path = dir.path().join("bad.sfst");
    ng_index::build_sfst_traces_file(&bad, &bad_path, &ng_index::Metrics::new()).unwrap();
    corrupt_chunk(&bad_path, *b"CHLD");
    let mut q = entry_spans("status_code");
    q.sections.groups = true;
    q.sections.rows = Some(RowsSpec {
        order: newest(None, RowDirection::Older),
        limit: 10,
        columns: Vec::new(),
    });
    let data = run(
        vec![
            sealed_source(dir.path(), &good, "good"),
            sealed_source_at(&bad_path, "bad"),
        ],
        q,
    );
    let failed = partial(&[(PartialReason::SourceFailure, 1, 2)]);
    assert_eq!(data.groups.as_ref().unwrap().status, failed);
    assert_eq!(data.rows.as_ref().unwrap().status, failed);
    assert_eq!(data.status, failed);
}

/// A live WAL whose live pass fails leaves its rows without origins and
/// self time: the Groups section says so, and the request once.
#[test]
fn groups_are_partial_when_a_live_pass_fails() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(
        dir.path(),
        vec![req(&request(0x11, 0x10)), req(&request(0x12, 0x20))],
        "live",
    );
    let whole = whole_range(&wal);
    let frames = wal::scan_frame_boundaries(&wal, whole).unwrap();
    assert_eq!(frames.len(), 2);
    let second = frames[0].end_offset;
    let tail = TraceSource::Tail(sfsq::traces::TraceWalTail {
        source_id: SourceId::new("live#tail".to_string()),
        path: wal.clone(),
        coverage: WalCoverage {
            wal_id: wal.display().to_string().into(),
            range: wal::FrameRange::new(second, whole.end()),
        },
    });
    let mut q = entry_spans("status_code");
    q.sections.groups = true;
    let data = run(vec![tail], q);
    let groups = data.groups.as_ref().unwrap();
    assert_eq!(
        groups.status,
        partial(&[(PartialReason::LivePassFailed, 1, 1)])
    );
    assert_eq!(data.status, groups.status);
    assert_eq!(groups.self_ns_total, 0);
    let spans: u64 = groups.rows.iter().map(|row| row.numbers.spans).sum();
    assert_eq!(spans, 4, "the second request's trace, every span");
}

fn selection(filter: sfst::Filter) -> Option<ExploreSelection> {
    Some(ExploreSelection {
        filter,
        duration: None,
        time_ns: None,
    })
}

fn every_span_with_rows() -> ExploreQuery {
    let mut q = query("status_code", &[]);
    q.sections.facets = Some(FacetSpec { fields: None });
    q.sections.rows = Some(RowsSpec {
        order: newest(None, RowDirection::Older),
        limit: 10,
        columns: Vec::new(),
    });
    q
}

/// QRY-07: a selection leaves the histogram the scope's, lists only its own
/// rows, and gives every facet its comparison out of the scope and
/// selection rows — except Status, which the selection is made of: it keeps
/// its plain facet, unranked, after the ranked fields (D43).
#[test]
fn a_selection_leaves_the_histogram_and_narrows_the_rows() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let source = || vec![sealed_source(dir.path(), &wal, "a")];

    let plain = run(source(), every_span_with_rows());
    let mut q = every_span_with_rows();
    q.selection = selection(sfst::Filter::new().select("status_code", "ERROR"));
    let data = run(source(), q);

    assert!(data.status.is_complete(), "{:?}", data.status);
    assert_eq!(data.histogram, plain.histogram);
    let rows = data.rows.unwrap();
    assert_eq!(rows.matched, 1);
    assert_eq!(rows.items.len(), 1);
    assert_eq!(rows.items[0].status.as_deref(), Some("ERROR"));
    let facets = data.facets.unwrap();
    assert_eq!(
        facets.comparison,
        Some(sfsq::traces::explore::ComparisonTotals {
            scope: 4,
            selection: 1
        })
    );
    let plain_facets = plain.facets.unwrap();
    assert!(plain_facets.comparison.is_none());
    for facet in &facets.fields {
        if facet.field == "status_code" {
            assert!(facet.in_selection && facet.comparison.is_none());
            let unselected = plain_facets
                .fields
                .iter()
                .find(|f| f.field == "status_code")
                .unwrap();
            assert_eq!(facet.values, unselected.values);
        } else {
            assert!(
                !facet.in_selection && facet.comparison.is_some(),
                "{}",
                facet.field
            );
        }
    }
    let ranked = facets
        .fields
        .iter()
        .filter(|f| f.comparison.as_ref().is_some_and(|c| c.rank.is_some()))
        .count();
    let status = facets
        .fields
        .iter()
        .position(|f| f.field == "status_code")
        .unwrap();
    assert_eq!(status, ranked, "right after the ranked fields");
}

/// A selection whose time range lies outside the window selects nothing: no
/// rank, no rows, and nothing partial.
#[test]
fn a_selection_time_range_outside_the_window_selects_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let mut q = every_span_with_rows();
    q.selection = Some(ExploreSelection {
        filter: sfst::Filter::new(),
        duration: None,
        time_ns: Some(20 * S as i64..30 * S as i64),
    });
    let data = run(vec![sealed_source(dir.path(), &wal, "a")], q);

    assert!(data.status.is_complete(), "{:?}", data.status);
    let facets = data.facets.unwrap();
    assert_eq!(facets.comparison.map(|c| c.selection), Some(0));
    for facet in &facets.fields {
        let comparison = facet.comparison.as_ref().unwrap();
        assert!(comparison.rank.is_none() && comparison.best.is_none());
    }
    let rows = data.rows.unwrap();
    assert_eq!((rows.matched, rows.items.len()), (0, 0));
}

/// A page selected again without a source whose row fields fail keeps the
/// selection: the broken file's rows fall in the selected second, and the
/// page chosen again lists only the other file's rows of that second.
#[test]
fn a_reselected_page_keeps_the_selection() {
    let dir = tempfile::tempdir().unwrap();
    let mut spans = Vec::new();
    for i in 0..8u8 {
        spans.push(SpanSpec {
            trace: [0x30 + i; 16],
            ..sp(i + 1, 0, u64::from(i + 1) * S / 2, "op")
        });
    }
    let good = write_wal(dir.path(), vec![req(&spans)], "good");
    let broken_wal = write_wal(
        dir.path(),
        vec![req(&distinct_ids(0x61, 1_100, "b"))],
        "broken",
    );
    let broken = dir.path().join("broken.sfst");
    ng_index::build_sfst_traces_file(&broken_wal, &broken, &ng_index::Metrics::new()).unwrap();
    corrupt_chunk(&broken, *b"HF\0\0");

    let second = S as i64..2 * S as i64;
    let mut q = every_span_with_rows();
    q.sections.facets = None;
    q.sections.histogram = None;
    q.sections.rows.as_mut().unwrap().columns = vec!["attributes.id".to_string()];
    q.selection = Some(ExploreSelection {
        filter: sfst::Filter::new(),
        duration: None,
        time_ns: Some(second.clone()),
    });
    let data = run(
        vec![
            sealed_source(dir.path(), &good, "good"),
            sealed_source_at(&broken, "broken"),
        ],
        q,
    );
    let rows = data.rows.unwrap();
    assert!(!rows.status.is_complete(), "the broken file is named");
    let starts: Vec<i64> = rows.items.iter().map(|row| row.key.start_ns).collect();
    assert_eq!(starts, [3 * S as i64 / 2, S as i64]);
}

/// Chips on `_err_origin` in the selection make the facets name a live
/// WAL whose live pass failed.
#[test]
fn a_selection_on_err_origin_names_a_failed_live_pass() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(
        dir.path(),
        vec![req(&request(0x11, 0x10)), req(&request(0x12, 0x20))],
        "live",
    );
    let whole = whole_range(&wal);
    let frames = wal::scan_frame_boundaries(&wal, whole).unwrap();
    let tail = TraceSource::Tail(sfsq::traces::TraceWalTail {
        source_id: SourceId::new("live#tail".to_string()),
        path: wal.clone(),
        coverage: WalCoverage {
            wal_id: wal.display().to_string().into(),
            range: wal::FrameRange::new(frames[0].end_offset, whole.end()),
        },
    });
    let mut q = query("status_code", &[]);
    q.sections.histogram = None;
    q.sections.facets = Some(FacetSpec {
        fields: Some(vec!["name".to_string()]),
    });
    q.selection = selection(sfst::Filter::new().select(sfst::ERR_ORIGIN_FIELD, "true"));
    let data = run(vec![tail], q);
    assert_eq!(
        data.facets.unwrap().status,
        partial(&[(PartialReason::LivePassFailed, 1, 1)])
    );
}

/// A selection without a term, or with inverted or negative bounds, is a
/// request error.
#[test]
fn a_malformed_selection_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let bad = [
        ExploreSelection {
            filter: sfst::Filter::new(),
            duration: None,
            time_ns: None,
        },
        ExploreSelection {
            filter: sfst::Filter::new(),
            duration: Some(sfst::DurationRange {
                min_ns: Some(5),
                max_ns: Some(4),
            }),
            time_ns: None,
        },
        ExploreSelection {
            filter: sfst::Filter::new(),
            duration: Some(sfst::DurationRange {
                min_ns: Some(-1),
                max_ns: None,
            }),
            time_ns: None,
        },
        ExploreSelection {
            filter: sfst::Filter::new(),
            duration: None,
            time_ns: Some(5..5),
        },
    ];
    for selection in bad {
        let mut q = every_span_with_rows();
        q.selection = Some(selection);
        let answer = explore(
            vec![sealed_source(dir.path(), &wal, "a")],
            q,
            ExploreOptions::default(),
            CancellationToken::new(),
            Arc::new(AtomicUsize::new(0)),
        );
        assert!(answer.is_err());
    }
}

/// A file sealed while the seal still wrote the retired per-trace rollup
/// chunk (`TRSU`) is an ordinary current file to the explorer: every span
/// counted, nothing partial.
#[test]
fn a_file_carrying_the_retired_rollup_chunk_is_explored_like_any_other() {
    let path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/trsu_seal.sfst");
    let mut query = query("status_code", &[]);
    query.grid = sfst::Grid::new(1_700_000_000 * S as i64, S as i64, 10);
    let data = run(vec![sealed_source_at(&path, "trsu_seal")], query);
    let histogram = data.histogram.unwrap();
    assert!(histogram.status.is_complete(), "{:?}", histogram.status);
    assert_eq!(histogram.totals.count, 3);
}
