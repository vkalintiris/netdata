use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{any_value, AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    resource::v1::Resource,
};

use crate::jetstream::Event;

const SERVICE_NAME: &str = "bluesky-jetstream";
const SCOPE_NAME: &str = "jetstream-otel-bridge";
const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
const SEVERITY_INFO: i32 = 9;

fn str_val(s: &str) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::StringValue(s.to_string())),
    })
}

fn bool_val(b: bool) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::BoolValue(b)),
    })
}

fn kv(key: &str, value: Option<AnyValue>) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value,
    }
}

/// Convert a serde_json::Value into an OTel AnyValue.
pub fn json_to_any_value(value: &serde_json::Value) -> AnyValue {
    match value {
        serde_json::Value::Null => AnyValue { value: None },
        serde_json::Value::Bool(b) => AnyValue {
            value: Some(any_value::Value::BoolValue(*b)),
        },
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                AnyValue {
                    value: Some(any_value::Value::IntValue(i)),
                }
            } else if let Some(f) = n.as_f64() {
                AnyValue {
                    value: Some(any_value::Value::DoubleValue(f)),
                }
            } else {
                AnyValue {
                    value: Some(any_value::Value::StringValue(n.to_string())),
                }
            }
        }
        serde_json::Value::String(s) => AnyValue {
            value: Some(any_value::Value::StringValue(s.clone())),
        },
        serde_json::Value::Array(arr) => AnyValue {
            value: Some(any_value::Value::ArrayValue(ArrayValue {
                values: arr.iter().map(json_to_any_value).collect(),
            })),
        },
        serde_json::Value::Object(obj) => AnyValue {
            value: Some(any_value::Value::KvlistValue(KeyValueList {
                values: obj
                    .iter()
                    .map(|(k, v)| KeyValue {
                        key: k.clone(),
                        value: Some(json_to_any_value(v)),
                    })
                    .collect(),
            })),
        },
    }
}

/// Convert a Jetstream Event into an OTel LogRecord.
pub fn event_to_log_record(event: &Event, raw_json: &serde_json::Value) -> LogRecord {
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let mut attributes = vec![
        kv("bluesky.did", str_val(&event.did)),
        kv("bluesky.event.kind", str_val(&event.kind.to_string())),
    ];

    if let Some(commit) = &event.commit {
        attributes.push(kv(
            "bluesky.commit.operation",
            str_val(&commit.operation.to_string()),
        ));
        attributes.push(kv("bluesky.commit.collection", str_val(&commit.collection)));
        attributes.push(kv("bluesky.commit.rkey", str_val(&commit.rkey)));
        attributes.push(kv("bluesky.commit.rev", str_val(&commit.rev)));
        if let Some(cid) = &commit.cid {
            attributes.push(kv("bluesky.commit.cid", str_val(cid)));
        }
    }

    if let Some(identity) = &event.identity {
        if let Some(handle) = &identity.handle {
            attributes.push(kv("bluesky.identity.handle", str_val(handle)));
        }
    }

    if let Some(account) = &event.account {
        attributes.push(kv("bluesky.account.active", bool_val(account.active)));
        if let Some(status) = &account.status {
            attributes.push(kv("bluesky.account.status", str_val(&status.to_string())));
        }
    }

    LogRecord {
        time_unix_nano: event.time_us * 1000,
        observed_time_unix_nano: now_ns,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(json_to_any_value(raw_json)),
        attributes,
        event_name: event.kind.to_string(),
        ..Default::default()
    }
}

/// Build an ExportLogsServiceRequest from a batch of LogRecords.
pub fn build_export_request(log_records: Vec<LogRecord>) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("service.name", str_val(SERVICE_NAME))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: SCOPE_NAME.to_string(),
                    version: CRATE_VERSION.to_string(),
                    attributes: vec![],
                    dropped_attributes_count: 0,
                }),
                log_records,
                schema_url: String::new(),
            }],
            schema_url: String::new(),
        }],
    }
}
