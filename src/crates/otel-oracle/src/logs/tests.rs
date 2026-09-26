use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue, any_value};
use opentelemetry_proto::tonic::logs::v1::{LogRecord, ResourceLogs, ScopeLogs};
use opentelemetry_proto::tonic::resource::v1::Resource;

use super::*;

fn kv(key: &str, value: any_value::Value) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value: Some(AnyValue { value: Some(value) }),
    }
}

fn text(value: &str) -> any_value::Value {
    any_value::Value::StringValue(value.to_string())
}

fn stream(namespace: &str, name: &str) -> Stream {
    Stream {
        namespace: namespace.to_string(),
        name: name.to_string(),
    }
}

fn record(time: u64, observed: u64, trace: Vec<u8>, span: Vec<u8>) -> LogRecord {
    LogRecord {
        time_unix_nano: time,
        observed_time_unix_nano: observed,
        trace_id: trace,
        span_id: span,
        ..LogRecord::default()
    }
}

fn request(resource: Vec<KeyValue>, records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: resource,
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

fn key(ts_ns: i64, trace: u8, span: u8, name: &str) -> LogKey {
    LogKey {
        ts_ns,
        trace_id: Some([trace; 16]),
        span_id: Some([span; 8]),
        stream: stream("", name),
    }
}

#[test]
fn records_keep_time_ids_and_stream_by_the_rules() {
    let logs = logs_of_request(&request(
        vec![
            kv("service.namespace", text("shop")),
            kv("service.name", any_value::Value::IntValue(7)),
            kv("service.name", text("cart")),
        ],
        vec![
            record(10, 20, vec![1; 16], vec![2; 8]),
            record(0, 20, vec![1; 8], vec![0; 8]),
            record(0, 0, Vec::new(), vec![3; 8]),
        ],
    ));
    let cart = stream("shop", "cart");
    assert_eq!(
        logs,
        vec![
            OracleLog {
                ts_ns: Some(10),
                trace_id: Some([1; 16]),
                span_id: Some([2; 8]),
                stream: cart.clone(),
            },
            OracleLog {
                ts_ns: Some(20),
                trace_id: None,
                span_id: None,
                stream: cart.clone(),
            },
            OracleLog {
                ts_ns: None,
                trace_id: None,
                span_id: Some([3; 8]),
                stream: cart,
            },
        ]
    );
    let bare = logs_of_request(&ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            scope_logs: vec![ScopeLogs {
                log_records: vec![record(5, 0, Vec::new(), Vec::new())],
                ..ScopeLogs::default()
            }],
            ..ResourceLogs::default()
        }],
    });
    assert_eq!(bare[0].stream, Stream::default());
}

#[test]
fn logs_of_selects_by_window_ids_and_streams_oldest_first() {
    let log = |ts: Option<i64>, trace: Option<u8>, span: u8, name: &str| OracleLog {
        ts_ns: ts,
        trace_id: trace.map(|b| [b; 16]),
        span_id: Some([span; 8]),
        stream: stream("", name),
    };
    let logs = vec![
        log(Some(30), Some(1), 1, "a"),
        log(Some(10), Some(1), 2, "b"),
        log(Some(20), Some(2), 1, "a"),
        log(Some(40), Some(1), 1, "a"),
        log(Some(15), None, 1, "a"),
        log(None, Some(1), 1, "a"),
    ];
    let wanted = |trace: &[u8], span: &[u8], streams: Option<&[&str]>| LogsWanted {
        window: 10..40,
        trace_ids: trace.iter().map(|b| [*b; 16]).collect(),
        span_ids: span.iter().map(|b| [*b; 8]).collect(),
        streams: streams.map(|names| names.iter().map(|n| stream("", n)).collect()),
    };

    assert_eq!(
        logs_of(&logs, &wanted(&[1], &[], None)),
        vec![key(10, 1, 2, "b"), key(30, 1, 1, "a")]
    );
    assert_eq!(
        logs_of(&logs, &wanted(&[1], &[1], None)),
        vec![key(30, 1, 1, "a")]
    );
    assert_eq!(
        logs_of(&logs, &wanted(&[1, 2], &[], Some(&["a"]))),
        vec![key(20, 2, 1, "a"), key(30, 1, 1, "a")]
    );
    assert_eq!(logs_of(&logs, &wanted(&[], &[1], None)).len(), 3);
    assert_eq!(
        logs_of(&logs, &wanted(&[3], &[], None)),
        Vec::<LogKey>::new()
    );
}

#[test]
fn a_page_may_hold_any_records_at_its_farthest_time() {
    let expected = vec![
        key(1, 1, 1, "a"),
        key(2, 1, 2, "a"),
        key(2, 1, 3, "a"),
        key(3, 1, 4, "a"),
    ];
    assert_eq!(
        page_diff(&expected, &[key(2, 1, 3, "a"), key(1, 1, 1, "a")], 2),
        None
    );
    assert_eq!(
        page_diff(&expected, &[key(1, 1, 1, "a"), key(2, 1, 2, "a")], 2),
        None
    );
    assert_eq!(page_diff(&expected, &expected, 10), None);
    assert_eq!(page_diff(&[], &[], 5), None);

    for (page, last) in [
        (vec![key(1, 1, 1, "a"), key(3, 1, 4, "a")], 2),
        (vec![key(2, 1, 2, "a"), key(2, 1, 3, "a")], 2),
        (vec![key(1, 1, 1, "a"), key(2, 1, 9, "a")], 2),
        (vec![key(1, 1, 1, "a")], 2),
        (
            vec![key(1, 1, 1, "a"), key(2, 1, 2, "a"), key(2, 1, 2, "a")],
            3,
        ),
    ] {
        assert!(page_diff(&expected, &page, last).is_some(), "{page:?}");
    }

    let newest_first: Vec<LogKey> = expected.iter().rev().cloned().collect();
    assert_eq!(
        page_diff(&newest_first, &[key(3, 1, 4, "a"), key(2, 1, 2, "a")], 2),
        None
    );
}
