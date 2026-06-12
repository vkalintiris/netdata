use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList, any_value},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    resource::v1::Resource,
};

pub fn str_val(s: &str) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::StringValue(s.to_string())),
    })
}

pub fn bool_val(b: bool) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::BoolValue(b)),
    })
}

pub fn kv(key: &str, value: Option<AnyValue>) -> KeyValue {
    KeyValue {
        key: key.to_string(),
        value,
    }
}

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

pub fn build_export_request(
    log_records: Vec<LogRecord>,
    service_name: &str,
    scope_name: &str,
    scope_version: &str,
) -> ExportLogsServiceRequest {
    ExportLogsServiceRequest {
        resource_logs: vec![ResourceLogs {
            resource: Some(Resource {
                attributes: vec![kv("service.name", str_val(service_name))],
                dropped_attributes_count: 0,
                entity_refs: vec![],
            }),
            scope_logs: vec![ScopeLogs {
                scope: Some(InstrumentationScope {
                    name: scope_name.to_string(),
                    version: scope_version.to_string(),
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

/// OTel severity number for INFO (9 = INFO per OpenTelemetry logs severity matrix).
pub const SEVERITY_INFO: i32 = 9;

pub fn now_unix_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
