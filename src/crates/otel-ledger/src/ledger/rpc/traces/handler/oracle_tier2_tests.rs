//! Tier 2 end to end, offline: a corpus stored the way the agent stores it (two
//! sealed files and a live WAL read as chunks and a tail), a capture of the
//! requests the agent was sent (with a resend and a span outside the ingestion
//! window), the reference calculator's membership and matching over the store's
//! files, and every tier-2 request (row pages walked by the agent's cursors
//! included) sent through the Function's byte path and judged against the
//! calculator.

use std::collections::BTreeMap;

use super::*;
use crate::ledger::rpc::traces::fixtures::{
    call_through_bridge, install_sealed, install_wal, make_registries_at,
};
use bridge::function::{HandlerAdapter, ProgressState};
use file_registry::TenantId;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::Span;
use otel_oracle::calc::Grid;
use otel_oracle::capture::{Record, Signal};
use otel_oracle::corpus::{self, MeshParams};
use otel_oracle::ingest::IngestWindow;
use otel_oracle::matching;
use otel_oracle::membership::{self, RowKey, UnitKind};
use otel_oracle::model;
use otel_oracle::tier2;
use prost::Message;
use tokio_util::sync::CancellationToken;

const T0_S: u64 = 1_700_000_000;
const CHUNK_ENTRIES: u32 = 100;
const MINUTE_NS: u64 = 60_000_000_000;
const HOUR_NS: u64 = 60 * MINUTE_NS;

struct Lab {
    _store: tempfile::TempDir,
    root: std::path::PathBuf,
    registries: Arc<RwLock<TenantRegistries>>,
    records: Vec<Record>,
    /// What each capture record left in the store, record by record.
    stored: Vec<ExportTraceServiceRequest>,
    after: u32,
    before: u32,
}

fn span_end(span: &Span) -> u64 {
    span.end_time_unix_nano.max(span.start_time_unix_nano)
}

/// The corpus as the agent received it and as it stored it, and the capture
/// the tee would have written in front of it.
async fn lab() -> Lab {
    let generated = corpus::generate(&MeshParams {
        traces: 300,
        start_ns: T0_S * 1_000_000_000,
        trace_spacing_ns: 900_000_000,
        seed: 61,
    });
    let mut requests = corpus::build_requests(&generated, 40);
    let third = requests.len() / 3;

    let mut last_ns = 0;
    for request in &requests {
        for rs in &request.resource_spans {
            for ss in &rs.scope_spans {
                for span in &ss.spans {
                    last_ns = last_ns.max(span_end(span));
                }
            }
        }
    }
    let received_ns = last_ns + MINUTE_NS;
    let too_old = received_ns - 25 * HOUR_NS;
    requests[2 * third + 1].resource_spans[0].scope_spans[0]
        .spans
        .push(Span {
            trace_id: vec![0xee; 16],
            span_id: vec![0xee; 8],
            name: "too old".to_string(),
            start_time_unix_nano: too_old,
            end_time_unix_nano: too_old + 1_000_000,
            ..Default::default()
        });

    let bounds = ng_flatten::TimeBounds {
        min_ns: received_ns - 24 * HOUR_NS,
        max_ns: received_ns + 10 * MINUTE_NS,
    };
    let mut records = Vec::new();
    let mut stored = Vec::new();
    let mut capture = |request: &ExportTraceServiceRequest, at_ns: u64| {
        let mut kept = request.clone();
        let normalized = ng_flatten::normalize_trace_request(&mut kept, at_ns, Some(bounds));
        records.push(Record {
            received_unix_ns: i64::try_from(at_ns).unwrap(),
            signal: Signal::Traces,
            grpc_code: 0,
            rejected: i64::try_from(normalized.rejected).unwrap(),
            request: request.encode_to_vec(),
        });
        stored.push(kept);
    };
    for (index, request) in requests.iter().enumerate() {
        capture(request, received_ns + index as u64 * 1_000_000);
    }
    let resent = 2 * third - 1;
    capture(&requests[resent], received_ns + 2_000_000_000);
    assert_eq!(records[2 * third + 1].rejected, 1);

    let store = tempfile::tempdir().unwrap();
    let root = store.path().to_path_buf();
    let registries = make_registries_at(&root);
    install_sealed(&registries, "default", 1, stored[..third].to_vec()).await;
    install_sealed(&registries, "default", 2, stored[third..2 * third].to_vec()).await;
    let mut live: Vec<ExportTraceServiceRequest> = stored[2 * third..requests.len()].to_vec();
    live.push(stored[requests.len()].clone());
    install_wal(&registries, "default", 3, live).await;

    let last_s = u32::try_from(last_ns / 1_000_000_000).unwrap();
    Lab {
        _store: store,
        root,
        registries,
        records,
        stored,
        after: T0_S as u32,
        before: last_s + 1,
    }
}

fn handler(lab: &Lab) -> OtelTracesHandler {
    OtelTracesHandler::new(
        lab.registries.clone(),
        Arc::new(ChunkCache::new(64 * 1024 * 1024)),
        u64::from(CHUNK_ENTRIES),
        None,
    )
}

#[tokio::test]
async fn tier2_through_the_handler_finds_nothing() {
    let lab = lab().await;
    let store = membership::read_store(&lab.root, CHUNK_ENTRIES).unwrap();
    let expected = matching::expected_rows(&lab.records, &IngestWindow::LAB);
    assert!(expected.notes.is_empty(), "{:?}", expected.notes);
    assert!(expected.doubtful.is_empty() && expected.synthesized == 0);

    let matched = matching::match_rows(&expected, &store.units);
    assert!(matched.lost.is_empty(), "{:?}", matched.lost.len());
    assert!(
        matched.unmatched.is_empty(),
        "{:?}",
        matched.unmatched.len()
    );
    assert!(matched.collisions.is_empty() && matched.despite_error.is_empty());
    assert!(matched.known.iter().all(|known| *known));
    assert!(store.stale_wals.is_empty());
    let kinds = |pick: fn(&UnitKind) -> bool| store.units.iter().filter(|u| pick(&u.kind)).count();
    assert_eq!(kinds(|k| matches!(k, UnitKind::Sealed)), 2);
    assert!(kinds(|k| matches!(k, UnitKind::Chunk(_))) >= 1);
    assert_eq!(kinds(|k| matches!(k, UnitKind::Tail(_))), 1);

    let grid = Grid::for_window(lab.after, lab.before);
    let check = matching::check_window(
        &store.units,
        &matched,
        &store.stale_wals,
        grid.after_s,
        grid.before_s,
    );
    assert!(check.judged(), "{check:?}");
    let spans = matching::window_spans(&matched, &check);
    let mut plan = tier2::plan(lab.after, lab.before, &spans, check.units.len() as u64);
    assert_eq!(plan.scenarios.len(), 5);

    let adapter = HandlerAdapter::new(handler(&lab));
    let mut answers = BTreeMap::new();
    for _ in 0..3 {
        for request in &plan.requests {
            if answers.contains_key(&request.id) {
                continue;
            }
            let body = serde_json::to_vec(&request.body).unwrap();
            let (status, payload) = call_through_bridge(&adapter, Some(&body)).await;
            assert_eq!(
                status,
                200,
                "{}: {}",
                request.id,
                String::from_utf8_lossy(&payload)
            );
            let answer: serde_json::Value = serde_json::from_slice(&payload).unwrap();
            answers.insert(request.id.clone(), answer);
        }
        tier2::add_pages(&mut plan, &answers);
    }
    let pages = |suffix: &str| {
        plan.requests
            .iter()
            .filter(|r| r.id.ends_with(suffix))
            .count()
    };
    let older_with_rows = plan
        .requests
        .iter()
        .filter(|r| r.id.ends_with(" older"))
        .filter(|r| {
            answers[&r.id]["data"]["rows"]["items"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
        })
        .count();
    assert_eq!(pages(" older"), 5);
    assert!(older_with_rows > 0);
    assert_eq!(pages(" older newer"), older_with_rows);
    assert_eq!(answers.len(), plan.requests.len());

    let (findings, checks) = tier2::judge(&plan, &spans, &answers);
    assert!(
        findings.is_empty(),
        "{:#?}",
        &findings[..findings.len().min(5)]
    );
    for check in [
        "ORC-ANSWER",
        "ORC-WINDOW",
        "ORC-HIST",
        "ORC-PCT",
        "ORC-TOTALS",
        "ORC-FACET",
        "ORC-ROWS",
        "ORC-TOPK",
        "ORC-FIELDS",
        "ORC-VALUES",
        "ORC-STATUS",
    ] {
        let count = checks.get(check).copied().unwrap_or_default();
        assert!(
            count.compared > 0 && count.differing == 0,
            "{check}: {count:?}"
        );
    }

    let id = plan.requests[0].id.clone();
    let buckets = answers.get_mut(&id).unwrap()["data"]["histogram"]["buckets"]
        .as_array_mut()
        .unwrap();
    let bucket = buckets
        .iter_mut()
        .find(|b| {
            b["counts"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c.as_u64() > Some(0))
        })
        .unwrap();
    let counts = bucket["counts"].as_array_mut().unwrap();
    let slot = counts.iter().position(|c| c.as_u64() > Some(0)).unwrap();
    counts[slot] = serde_json::json!(counts[slot].as_u64().unwrap() + 1);
    let (findings, _) = tier2::judge(&plan, &spans, &answers);
    assert_eq!(findings.len(), 1, "{findings:#?}");
    assert_eq!(findings[0].check, "ORC-HIST");
}

#[tokio::test]
async fn a_dropped_capture_record_leaves_exactly_its_spans_unmatched() {
    let lab = lab().await;
    let store = membership::read_store(&lab.root, CHUNK_ENTRIES).unwrap();
    let expected = matching::expected_rows(&lab.records[1..], &IngestWindow::LAB);

    let matched = matching::match_rows(&expected, &store.units);

    let mut unmatched: Vec<RowKey> = matched.unmatched.iter().map(|(_, key)| *key).collect();
    unmatched.sort();
    let mut dropped: Vec<RowKey> = model::spans_of_request(&lab.stored[0], 0)
        .iter()
        .map(RowKey::of)
        .collect();
    dropped.sort();
    assert_eq!(unmatched, dropped);
    assert!(matched.lost.is_empty());

    let grid = Grid::for_window(lab.after, lab.before);
    let check = matching::check_window(
        &store.units,
        &matched,
        &store.stale_wals,
        grid.after_s,
        grid.before_s,
    );
    assert_eq!(check.unmatched, dropped.len());
    assert!(!check.ingest_ok() && !check.judged());
}

/// The calculator's units are the engine's sources: sealed files and chunks by
/// name and record count, and one tail.
#[tokio::test]
async fn membership_units_are_the_handlers_sources() {
    let lab = lab().await;
    let store = membership::read_store(&lab.root, CHUNK_ENTRIES).unwrap();
    let grid = Grid::for_window(lab.after, lab.before);
    let capture = handler(&lab)
        .supplier
        .capture(
            &TenantId::from("default"),
            grid.after_s..grid.before_s,
            1,
            &CancellationToken::new(),
            &ProgressState::new(),
        )
        .await
        .unwrap();

    let mut sources: Vec<(String, Option<u32>)> = capture.sets[0]
        .iter()
        .map(|source| match source {
            sfsq::traces::TraceSource::Sfst(candidate) => (
                candidate.source_id.as_str().to_string(),
                Some(candidate.summary.record_count),
            ),
            sfsq::traces::TraceSource::Tail(_) => ("tail".to_string(), None),
            other => panic!("an unexpected source {:?}", other.source_id()),
        })
        .collect();
    sources.sort();
    let mut units: Vec<(String, Option<u32>)> = store
        .units
        .iter()
        .map(|unit| match unit.kind {
            UnitKind::Tail(_) => ("tail".to_string(), None),
            _ => (unit.name(), Some(unit.rows.len() as u32)),
        })
        .collect();
    units.sort();
    assert_eq!(units, sources);
}
