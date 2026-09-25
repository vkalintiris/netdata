//! Membership against a small store written the way the agent writes one: WAL
//! frames from the agent's own frame preparation, and a sealed file from the
//! store format's writer. The rows it reads must be the rows the calculator
//! rebuilds from the same requests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use otel_oracle::membership::{self, Membership, MembershipError, RowKey, UnitKind};
use otel_oracle::model::spans_of_request;

const BASE_NS: u64 = 1_790_000_000_000_000_000;

fn request(trace: u8, first_span: u8, spans: u8) -> ExportTraceServiceRequest {
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

fn keys(requests: &[&ExportTraceServiceRequest]) -> Vec<RowKey> {
    requests
        .iter()
        .flat_map(|request| spans_of_request(request, 0))
        .map(|span| RowKey::of(&span))
        .collect()
}

/// Writes one frame per request (with `extra` added to each frame's entry
/// count) into `<store>/wal/default`, and returns the WAL's path.
fn write_wal(store: &Path, requests: &[&ExportTraceServiceRequest], extra: usize) -> PathBuf {
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
fn write_sealed(store: &Path, wal: &Path, rows: &[RowKey]) -> PathBuf {
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

fn kinds_and_rows(membership: &Membership) -> Vec<(UnitKind, Vec<RowKey>)> {
    membership
        .units
        .iter()
        .map(|unit| (unit.kind.clone(), unit.rows.clone()))
        .collect()
}

#[test]
fn a_wal_splits_into_chunks_and_a_tail_holding_the_rebuilt_rows() {
    let store = tempfile::tempdir().unwrap();
    let (a, b, c) = (request(1, 1, 4), request(2, 11, 4), request(3, 21, 3));
    write_wal(store.path(), &[&a, &b, &c], 0);

    let read = membership::read_store(store.path(), 5).unwrap();

    assert_eq!(
        kinds_and_rows(&read),
        vec![
            (UnitKind::Chunk(0), keys(&[&a, &b])),
            (UnitKind::Tail(2), keys(&[&c])),
        ]
    );
    assert!(read.stale_wals.is_empty());
    let first = BASE_NS / 1_000_000_000;
    assert_eq!(
        read.units[0].seconds,
        Some((
            u32::try_from(first + 1).unwrap(),
            u32::try_from(first + 14).unwrap()
        ))
    );
}

#[test]
fn a_sealed_file_hides_the_wal_it_was_sealed_from() {
    let store = tempfile::tempdir().unwrap();
    let (a, b) = (request(1, 1, 3), request(2, 11, 3));
    let wal = write_wal(store.path(), &[&a, &b], 0);
    let rows = keys(&[&a, &b]);
    write_sealed(store.path(), &wal, &rows);

    let read = membership::read_store(store.path(), 2).unwrap();

    assert_eq!(kinds_and_rows(&read), vec![(UnitKind::Sealed, rows)]);
}

#[test]
fn a_wal_left_by_an_older_agent_instance_is_set_aside() {
    let store = tempfile::tempdir().unwrap();
    let a = request(1, 1, 2);
    let newest = write_wal(store.path(), &[&a], 0);

    let name = newest.file_name().unwrap().to_str().unwrap().to_string();
    let parts: Vec<&str> = name.splitn(3, '-').collect();
    let older = newest.with_file_name(format!(
        "{}-{}-{}",
        parts[0],
        "0".repeat(31) + "1",
        parts[2]
    ));
    fs::copy(&newest, &older).unwrap();
    let an_hour_ago = SystemTime::now() - Duration::from_secs(3_600);
    fs::File::options()
        .write(true)
        .open(&older)
        .unwrap()
        .set_modified(an_hour_ago)
        .unwrap();

    let read = membership::read_store(store.path(), 100).unwrap();

    let second = u32::try_from(BASE_NS / 1_000_000_000).unwrap();
    assert_eq!(
        read.stale_wals,
        vec![membership::StaleWal {
            path: older,
            seconds: Some((second + 1, second + 2)),
            readable: true,
        }]
    );
    assert!(read.stale_wals[0].overlaps(second + 2, second + 3));
    assert!(!read.stale_wals[0].overlaps(second + 3, second + 4));
    assert_eq!(
        kinds_and_rows(&read),
        vec![(UnitKind::Tail(0), keys(&[&a]))]
    );
}

#[test]
fn a_wal_cut_mid_frame_is_not_settled() {
    let store = tempfile::tempdir().unwrap();
    let wal = write_wal(store.path(), &[&request(1, 1, 3)], 0);
    let length = fs::metadata(&wal).unwrap().len();
    fs::File::options()
        .write(true)
        .open(&wal)
        .unwrap()
        .set_len(length - 3)
        .unwrap();

    let error = membership::read_store(store.path(), 100).unwrap_err();

    assert!(matches!(error, MembershipError::NotSettled(path, _) if path == wal));
}

#[test]
fn a_frame_whose_count_disagrees_with_its_spans_is_reported() {
    let store = tempfile::tempdir().unwrap();
    let wal = write_wal(store.path(), &[&request(1, 1, 3)], 1);

    let error = membership::read_store(store.path(), 100).unwrap_err();

    assert_eq!(
        error,
        MembershipError::FrameCount {
            path: wal,
            frame: 0,
            entries: 4,
            spans: 3
        }
    );
}
