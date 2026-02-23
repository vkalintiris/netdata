use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{any_value, AnyValue, ArrayValue, InstrumentationScope, KeyValue, KeyValueList},
    logs::v1::{LogRecord, ResourceLogs, ScopeLogs},
    resource::v1::Resource,
};

use crate::certstream::CertData;

const SERVICE_NAME: &str = "certstream";
const SCOPE_NAME: &str = "certstream-otel-bridge";
const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");
const SEVERITY_INFO: i32 = 9;

fn str_val(s: &str) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::StringValue(s.to_string())),
    })
}

fn float_val(f: f64) -> Option<AnyValue> {
    Some(AnyValue {
        value: Some(any_value::Value::DoubleValue(f)),
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

/// Convert CertStream CertData into an OTel LogRecord.
pub fn event_to_log_record(data: &CertData, raw_json: &serde_json::Value) -> LogRecord {
    let now_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;

    let time_unix_nano = (data.seen * 1e9) as u64;

    let domains_array = AnyValue {
        value: Some(any_value::Value::ArrayValue(ArrayValue {
            values: data
                .leaf_cert
                .all_domains
                .iter()
                .map(|d| AnyValue {
                    value: Some(any_value::Value::StringValue(d.clone())),
                })
                .collect(),
        })),
    };

    let mut attributes = vec![
        kv("cert.update_type", str_val(&data.update_type)),
        kv("cert.fingerprint", str_val(&data.leaf_cert.fingerprint)),
        kv(
            "cert.serial_number",
            str_val(&data.leaf_cert.serial_number),
        ),
        kv("cert.source.name", str_val(&data.source.name)),
        kv("cert.source.url", str_val(&data.source.url)),
        kv(
            "cert.domains",
            Some(domains_array),
        ),
        kv("cert.not_before", float_val(data.leaf_cert.not_before)),
        kv("cert.not_after", float_val(data.leaf_cert.not_after)),
    ];

    if let Some(cn) = &data.leaf_cert.subject.cn {
        attributes.push(kv("cert.subject.cn", str_val(cn)));
    }
    if let Some(aggregated) = &data.leaf_cert.subject.aggregated {
        attributes.push(kv("cert.subject.aggregated", str_val(aggregated)));
    }
    if let Some(cn) = &data.leaf_cert.issuer.cn {
        attributes.push(kv("cert.issuer.cn", str_val(cn)));
    }
    if let Some(o) = &data.leaf_cert.issuer.o {
        attributes.push(kv("cert.issuer.o", str_val(o)));
    }

    LogRecord {
        time_unix_nano,
        observed_time_unix_nano: now_ns,
        severity_number: SEVERITY_INFO,
        severity_text: "INFO".to_string(),
        body: Some(json_to_any_value(raw_json)),
        attributes,
        event_name: "certificate_update".to_string(),
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
