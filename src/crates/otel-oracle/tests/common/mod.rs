//! Stores and requests for the integration tests, written the way the agent
//! writes them: WAL frames from the agent's own frame preparation, and sealed
//! files from the store format's writer.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use otel_oracle::membership::RowKey;

pub const BASE_NS: u64 = 1_790_000_000_000_000_000;

pub fn request(trace: u8, first_span: u8, spans: u8) -> ExportTraceServiceRequest {
    let spans = (0..spans)
        .map(|i| {
            let n = u64::from(first_span + i);
            Span {
                trace_id: vec![trace; 16],
                span_id: vec![first_span + i; 8],
                start_time_unix_nano: BASE_NS + n * 1_000_000_000,
                end_time_unix_nano: BASE_NS + n * 1_000_000_000 + n * 1_000,
                name: format!("op-{n}"),
                ..Default::default()
            }
        })
        .collect();
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            scope_spans: vec![ScopeSpans {
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

/// Writes one frame per request (with `extra` added to each frame's entry
/// count) into `<store>/wal/default`, and returns the WAL's path.
pub fn write_wal(store: &Path, requests: &[&ExportTraceServiceRequest], extra: usize) -> PathBuf {
    let dir = store.join("wal/default");
    let mut writer = wal::Writer::new(
        &dir,
        wal::Config::default(),
        Arc::new(wal::SeqAllocator::ephemeral(0)),
        wal::FileStamp {
            pipeline_id: 1,
            payload_format: ng_flatten::TRACE_FRAME_PAYLOAD_FORMAT,
        },
        wal::test_identity(),
    )
    .unwrap();
    for (i, request) in requests.iter().enumerate() {
        let frame = ng_flatten::prepare_trace_frame((*request).clone(), 1, None).unwrap();
        writer
            .write_frame(
                7,
                &[],
                &frame.data,
                wal::FrameMeta {
                    entry_count: frame.records + extra,
                    ingestion_ns: file_registry::TimestampNs(i as u64 + 1),
                    log_ts_range: None,
                },
            )
            .unwrap();
        writer.sync_all().unwrap();
    }
    drop(writer);
    let mut wals: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(wals.len(), 1);
    wals.remove(0)
}

/// Writes a sealed file holding `rows` under the stem of `wal`.
pub fn write_sealed(store: &Path, wal: &Path, rows: &[RowKey]) -> PathBuf {
    let dir = store.join("index/default");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(wal.with_extension("sfst").file_name().unwrap());

    let arena = bumpalo::Bump::new();
    let mut index = sfst::RowIndex::new(&arena, 100);
    let mut trace_ids = sfst::TraceIds::with_capacity(rows.len());
    let mut span_ids = sfst::SpanIds::with_capacity(rows.len());
    let mut durations = Vec::new();
    for row in rows {
        let token = index.intern(None, "name=x");
        index.row(row.start_ns, &[token]);
        trace_ids.push(sfst::TraceId::from_bytes(&row.trace_id).unwrap());
        span_ids.push(sfst::SpanId::from_bytes(&row.span_id).unwrap());
        durations.push(row.duration_ns);
    }
    index.trace_ids = Some(trace_ids);
    index.span_ids = Some(span_ids);
    index.durations = Some(sfst::Durations(durations));
    sfst::IndexWriter::write_file(&index, &path, Vec::new()).unwrap();
    path
}
