//! The explorer engine (`sfsq::traces::explore`): the histogram stacked by a
//! field, window totals, and how every unreadable source is counted.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use sfsq::Source;
use sfsq::traces::explore::{
    ExploreData, ExploreQuery, ExploreScope, HistogramData, HistogramSpec, Sections, StackBucket,
    Totals, explore,
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
        scope: ExploreScope { filter },
        sections: Sections {
            histogram: Some(HistogramSpec {
                stack: stack.to_string(),
            }),
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
    }
}

/// The histogram of `request(..)` once, entry spans stacked by status.
fn one_request_histogram(times: u64) -> HistogramData {
    let mut buckets = vec![bucket(&[0, 0], 0); 10];
    buckets[1] = bucket(&[times, 0], 0);
    buckets[2] = bucket(&[0, times], 0);
    HistogramData {
        stack: "status_code".to_string(),
        dimensions: vec!["ERROR".to_string(), "OK".to_string()],
        buckets,
        totals: Totals {
            count: 2 * times,
            errors: times,
        },
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
            errors: 1
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
    assert_eq!(
        data.status,
        partial(&[(PartialReason::SourceFailure, 1, 2)])
    );
    assert_eq!(data.histogram, Some(one_request_histogram(1)));
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
    assert_eq!(
        data.status,
        partial(&[
            (PartialReason::SourceFailure, 1, 4),
            (PartialReason::RemoteUnavailable, 1, 4),
            (PartialReason::LegacyFile, 1, 4),
        ])
    );
    assert_eq!(
        data.sources, 4,
        "the remote file outside the window is not a candidate"
    );
    assert_eq!(data.histogram, Some(one_request_histogram(1)));
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
