//! Tier 1 for logs of a trace: the logs engine against the reference
//! calculator's logs model (`otel_oracle::logs`). The records carry the ids of
//! a generated trace mesh, plus foreign traces, records of a trace without a
//! span and records without ids, over three streams; two thirds of each
//! stream's records are sealed, the rest sit in a live WAL (indexed chunks and
//! a row-scanned tail). Requests follow the drawer: a trace (or a span of it)
//! over the trace's time envelope, whole pages and short pages both ways.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use file_registry::TimestampNs;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;
use otel_oracle::corpus::{MeshParams, generate};
use otel_oracle::logs::{
    LogKey, LogsWanted, OracleLog, Stream, logs_of, logs_of_request, page_diff,
};
use sfsq::logs::{
    Direction, LogRow, LogSource, LogsData, LogsQueryBuilder, Part, SfstCandidate, Source, WalTail,
    run,
};

const STREAMS: [(&str, &str); 3] = [("shop", "front"), ("shop", "orders"), ("", "back")];
/// Records per export request, one WAL frame each.
const BATCH: usize = 8;
/// Records per indexed chunk of a live WAL.
const MIN_ENTRIES: u64 = 12;
const SLACK_NS: i64 = 1_000_000_000;

fn text(value: &str) -> AnyValue {
    AnyValue {
        value: Some(any_value::Value::StringValue(value.to_string())),
    }
}

fn record(ts: u64, trace_id: Vec<u8>, span_id: Vec<u8>) -> LogRecord {
    LogRecord {
        time_unix_nano: ts,
        trace_id,
        span_id,
        severity_text: "INFO".to_string(),
        ..LogRecord::default()
    }
}

fn request(stream: usize, records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    let (namespace, name) = STREAMS[stream];
    let mut attributes = vec![KeyValue {
        key: "service.name".to_string(),
        value: Some(text(name)),
    }];
    if !namespace.is_empty() {
        attributes.push(KeyValue {
            key: "service.namespace".to_string(),
            value: Some(text(namespace)),
        });
    }
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes,
                ..Resource::default()
            }),
            scope_logs: vec![ScopeLogs {
                log_records: records,
                ..ScopeLogs::default()
            }],
            ..ResourceLogs::default()
        }],
    }
}

/// Per stream, its export requests in arrival order; and the trace ids in
/// first-seen order with each trace's span time envelope.
struct Corpus {
    streams: Vec<Vec<ExportLogsServiceRequest>>,
    traces: Vec<([u8; 16], i64, i64)>,
    spans: Vec<([u8; 16], [u8; 8])>,
}

fn corpus() -> Corpus {
    let spans = generate(&MeshParams {
        traces: 45,
        start_ns: 1_700_000_000_000_000_000,
        trace_spacing_ns: 400_000_000,
        seed: 11,
    });
    let mut per_stream: Vec<Vec<LogRecord>> = vec![Vec::new(); STREAMS.len()];
    let mut traces: Vec<([u8; 16], i64, i64)> = Vec::new();
    let mut span_ids = Vec::new();
    for (i, mesh) in spans.iter().enumerate() {
        let span = &mesh.span;
        let stream = mesh.service as usize % STREAMS.len();
        let ts = span.start_time_unix_nano + (i as u64 % 3) * 1_000_000;
        per_stream[stream].push(record(ts, span.trace_id.clone(), span.span_id.clone()));
        match i % 5 {
            // A second record at the same time: ties the store breaks its own way.
            0 => per_stream[stream].push(record(ts, span.trace_id.clone(), span.span_id.clone())),
            // A record of the trace without a span.
            1 => per_stream[stream].push(record(ts + 1, span.trace_id.clone(), Vec::new())),
            // Another trace's record at the same time, in another stream.
            2 => per_stream[(stream + 1) % STREAMS.len()].push(record(
                ts,
                vec![0xf0 ^ (i as u8); 16],
                vec![7; 8],
            )),
            3 => per_stream[stream].push(record(ts, Vec::new(), Vec::new())),
            _ => {}
        }
        let (Ok(trace), Ok(span_id)) = (
            <[u8; 16]>::try_from(span.trace_id.as_slice()),
            <[u8; 8]>::try_from(span.span_id.as_slice()),
        ) else {
            continue;
        };
        span_ids.push((trace, span_id));
        let start = span.start_time_unix_nano as i64;
        let end = (span.end_time_unix_nano as i64).max(start);
        match traces.iter_mut().find(|(id, _, _)| *id == trace) {
            Some((_, lo, hi)) => {
                *lo = (*lo).min(start);
                *hi = (*hi).max(end);
            }
            None => traces.push((trace, start, end)),
        }
    }
    let mut streams = Vec::new();
    for (stream, mut records) in per_stream.into_iter().enumerate() {
        // The last record alone in the last frame: fewer entries than a chunk
        // needs, so every live WAL keeps a tail.
        let last = records.pop().expect("every stream has records");
        let mut requests = Vec::new();
        for chunk in records.chunks(BATCH) {
            requests.push(request(stream, chunk.to_vec()));
        }
        requests.push(request(stream, vec![last]));
        streams.push(requests);
    }
    Corpus {
        streams,
        traces,
        spans: span_ids,
    }
}

fn write_wal(dir: &Path, requests: &[ExportLogsServiceRequest]) -> PathBuf {
    let seq = Arc::new(wal::SeqAllocator::ephemeral(0));
    let mut writer = wal::Writer::new(
        dir,
        wal::Config::default(),
        seq,
        wal::FileStamp {
            pipeline_id: 0,
            payload_format: ng_flatten::LOG_FRAME_PAYLOAD_FORMAT,
        },
        wal::test_identity(),
    )
    .expect("writer");
    for (i, request) in requests.iter().enumerate() {
        let count = request.resource_logs[0].scope_logs[0].log_records.len();
        let (flattened, _) = ng_flatten::flatten_log_request(request.clone());
        let bytes = ng_flatten::encode_log_frame(&flattened).expect("encode frame");
        writer
            .write_frame(
                0,
                &[],
                &bytes,
                wal::FrameMeta {
                    entry_count: count,
                    ingestion_ns: TimestampNs(1_700_000_100_000_000_000 + i as u64),
                    log_ts_range: None,
                },
            )
            .expect("write frame");
    }
    writer.shutdown_all().expect("shutdown");
    let mut wals = Vec::new();
    for entry in std::fs::read_dir(dir).expect("read dir") {
        let path = entry.expect("entry").path();
        if path.extension().is_some_and(|x| x == "wal") {
            wals.push(path);
        }
    }
    assert_eq!(wals.len(), 1, "one WAL per directory");
    wals.pop().unwrap()
}

/// Every stream's sources: its sealed file, then its live WAL's chunks and tail.
fn sources(corpus: &Corpus, dir: &Path) -> Vec<LogSource> {
    let header = wal::HEADER_SIZE as u64;
    let mut out = Vec::new();
    for (s, requests) in corpus.streams.iter().enumerate() {
        let cut = requests.len() * 2 / 3;
        let sealed_dir = dir.join(format!("sealed-{s}"));
        std::fs::create_dir_all(&sealed_dir).unwrap();
        write_wal(&sealed_dir, &requests[..cut]);
        let sealed = dir.join(format!("stream-{s}.sfst"));
        ng_index::build_sfst(&sealed_dir, &sealed, &ng_index::Metrics::new()).expect("seal");
        let bytes = std::fs::read(&sealed).unwrap();
        out.push(LogSource::Sfst(SfstCandidate {
            summary: sfst::read_summary(&bytes).expect("summary"),
            file_seq: 100 * s as u64 + 1,
            part: Part::Indexed(0),
            source: Source::File(sealed),
        }));

        let live_dir = dir.join(format!("live-{s}"));
        std::fs::create_dir_all(&live_dir).unwrap();
        let live = write_wal(&live_dir, &requests[cut..]);
        let end = std::fs::metadata(&live).unwrap().len();
        let frames = wal::scan_frame_boundaries(&live, wal::FrameRange::new(header, end)).unwrap();
        let chunks = wal::prefix::chunk_boundaries(&frames, header, MIN_ENTRIES);
        assert!(
            !chunks.is_empty(),
            "stream {s}: the live WAL has an indexed chunk ({} requests, {} frames, cut {cut})",
            requests.len(),
            frames.len()
        );
        let file_seq = 100 * s as u64 + 2;
        for (i, chunk) in chunks.iter().enumerate() {
            let (summary, bytes) = ng_index::build_sfst_range(&live, chunk.range).unwrap();
            out.push(LogSource::Sfst(SfstCandidate {
                summary,
                file_seq,
                part: Part::Indexed(i as u32),
                source: Source::Memory(Arc::new(bytes)),
            }));
        }
        let tail = wal::prefix::tail_start(&chunks, header);
        assert!(tail < end, "stream {s}: the live WAL has a tail");
        out.push(LogSource::Tail(WalTail {
            file_seq,
            path: live,
            range: wal::FrameRange::new(tail, end),
        }));
    }
    out
}

fn clone_source(source: &LogSource) -> LogSource {
    match source {
        LogSource::Sfst(c) => LogSource::Sfst(SfstCandidate {
            summary: c.summary.clone(),
            file_seq: c.file_seq,
            part: c.part,
            source: c.source.clone(),
        }),
        LogSource::Tail(t) => LogSource::Tail(WalTail {
            file_seq: t.file_seq,
            path: t.path.clone(),
            range: t.range,
        }),
    }
}

fn field<'a>(row: &'a LogRow, name: &str) -> &'a str {
    for (field, value) in &row.row.fields {
        if field == name {
            return value;
        }
    }
    ""
}

fn key(row: &LogRow) -> LogKey {
    LogKey {
        ts_ns: row.cursor.timestamp_ns,
        trace_id: row.trace_id.map(|id| *id.as_bytes()),
        span_id: row.span_id.map(|id| *id.as_bytes()),
        stream: Stream {
            namespace: field(row, "resource.attributes.service.namespace").to_string(),
            name: field(row, "resource.attributes.service.name").to_string(),
        },
    }
}

/// One request: the engine's answer and the calculator's records for it.
struct Case {
    label: String,
    wanted: LogsWanted,
}

impl Case {
    fn query(&self, direction: Direction, limit: usize) -> sfsq::logs::LogsQuery {
        let window = &self.wanted.window;
        let mut traces = Vec::new();
        for id in &self.wanted.trace_ids {
            traces.push(sfst::TraceId::from(*id));
        }
        let mut spans = Vec::new();
        for id in &self.wanted.span_ids {
            spans.push(sfst::SpanId::from(*id));
        }
        LogsQueryBuilder::new(sfst::Grid::new(window.start, window.end - window.start, 1))
            .trace_ids(traces)
            .span_ids(spans)
            .direction(direction)
            .limit(limit)
            .build()
    }
}

fn cases(corpus: &Corpus) -> Vec<Case> {
    let mut out = Vec::new();
    let picks = [0, 7, 15, corpus.traces.len() - 1];
    for &at in &picks {
        let (trace, lo, hi) = corpus.traces[at];
        let envelope = lo - SLACK_NS..hi + SLACK_NS;
        let mut wanted = LogsWanted {
            window: envelope.clone(),
            trace_ids: [trace].into_iter().collect(),
            span_ids: Default::default(),
            streams: None,
        };
        out.push(Case {
            label: format!("trace {at}"),
            wanted: wanted.clone(),
        });
        if let Some((_, span)) = corpus.spans.iter().find(|(t, _)| *t == trace) {
            wanted.span_ids = [*span].into_iter().collect();
            out.push(Case {
                label: format!("trace {at} span"),
                wanted: wanted.clone(),
            });
            wanted.span_ids.clear();
        }
        wanted.window = envelope.start..(lo + hi) / 2;
        out.push(Case {
            label: format!("trace {at} first half"),
            wanted: wanted.clone(),
        });
        wanted.window = envelope;
        wanted.trace_ids.insert([0xf0 ^ 2; 16]);
        wanted
            .trace_ids
            .insert(corpus.traces[(at + 1) % corpus.traces.len()].0);
        out.push(Case {
            label: format!("trace {at} with two more"),
            wanted,
        });
    }
    out
}

#[test]
fn the_logs_of_a_trace_match_the_calculator() {
    let corpus = corpus();
    let dir = tempfile::tempdir().expect("tempdir");
    let sources = sources(&corpus, dir.path());
    let mut logs: Vec<OracleLog> = Vec::new();
    for requests in &corpus.streams {
        for request in requests {
            logs.extend(logs_of_request(request));
        }
    }
    let answer = |query| -> LogsData {
        let mut set = Vec::new();
        for source in &sources {
            set.push(clone_source(source));
        }
        run(
            set,
            query,
            tokio_util::sync::CancellationToken::new(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        )
    };

    let mut judged = 0;
    let mut in_tail = false;
    for case in cases(&corpus) {
        let expected = logs_of(&logs, &case.wanted);
        let label = &case.label;

        let whole = answer(case.query(Direction::Backward, 10_000));
        assert_eq!(
            (whole.sources, whole.failed_sources),
            (sources.len() as u64, 0),
            "{label}"
        );
        assert_eq!(whole.matched, expected.len() as u64, "{label}");
        let page: Vec<LogKey> = whole.rows.iter().map(key).collect();
        assert!(
            page.windows(2).all(|pair| pair[0].ts_ns >= pair[1].ts_ns),
            "{label}: rows newest first"
        );
        assert_eq!(page_diff(&expected, &page, expected.len()), None, "{label}");
        for row in &whole.rows {
            if row.cursor.part == Part::Tail {
                in_tail = true;
            }
        }

        let newest_first: Vec<LogKey> = expected.iter().rev().cloned().collect();
        for (direction, closest) in [
            (Direction::Forward, &expected),
            (Direction::Backward, &newest_first),
        ] {
            let short = answer(case.query(direction, 3));
            let page: Vec<LogKey> = short.rows.iter().map(key).collect();
            assert!(
                page.windows(2).all(|pair| pair[0].ts_ns >= pair[1].ts_ns),
                "{label} {direction:?}: rows newest first"
            );
            assert_eq!(
                page_diff(closest, &page, 3),
                None,
                "{label} {direction:?} page of 3"
            );
        }
        if !expected.is_empty() {
            judged += 1;
        }
    }
    assert!(judged >= 12, "most cases select records: {judged}");
    assert!(in_tail, "some selected record sits in a live tail");
}
