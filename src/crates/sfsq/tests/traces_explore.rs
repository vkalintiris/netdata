//! The explorer engine (`sfsq::traces::explore`): the histogram stacked by a
//! field, window totals, and how every unreadable source is counted.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use sfsq::Source;
use sfsq::traces::explore::{
    ExploreData, ExploreQuery, ExploreScope, FacetSpec, HistogramData, HistogramSpec, Sections,
    StackBucket, Totals, explore,
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
        sections: Sections {
            histogram: Some(HistogramSpec {
                stack: stack.to_string(),
                percentiles: false,
            }),
            facets: None,
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

#[test]
fn a_garbage_source_is_a_counted_failure() {
    let dir = tempfile::tempdir().unwrap();
    let wal = write_wal(dir.path(), vec![req(&request(0x11, 0x10))], "a");
    let garbage = TraceSource::Sfst(TraceSfstCandidate {
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
    });
    let data = run(
        vec![sealed_source(dir.path(), &wal, "a"), garbage],
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

fn facet_values(data: &ExploreData, field: &str) -> Vec<(String, u64)> {
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

fn pairs(items: &[(&str, u64)]) -> Vec<(String, u64)> {
    items.iter().map(|(v, c)| (v.to_string(), *c)).collect()
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
