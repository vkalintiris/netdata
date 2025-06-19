use serde_json::{Map as JsonMap, Value as JsonValue};

use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, InstrumentationScope, KeyValue},
    logs::v1::LogRecord,
    resource::v1::Resource,
};

fn json_from_key_value_list(kvl: &Vec<KeyValue>) -> JsonMap<String, JsonValue> {
    let mut map = JsonMap::new();

    for kv in kvl {
        if let Some(any_value) = &kv.value {
            map.insert(kv.key.clone(), json_from_any_value(any_value));
        } else {
            map.insert(kv.key.clone(), JsonValue::Null);
        }
    }

    flatten_serde_json::flatten(&map)
}

fn json_from_any_value(any_value: &AnyValue) -> JsonValue {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;

    match &any_value.value {
        Some(Value::StringValue(s)) => JsonValue::String(s.clone()),
        Some(Value::BoolValue(b)) => JsonValue::Bool(*b),
        Some(Value::IntValue(i)) => JsonValue::Number(
            serde_json::Number::from_f64(*i as f64).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Some(Value::DoubleValue(d)) => JsonValue::Number(
            serde_json::Number::from_f64(*d).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Some(Value::ArrayValue(array)) => {
            let values: Vec<JsonValue> = array.values.iter().map(json_from_any_value).collect();
            JsonValue::Array(values)
        }
        Some(Value::KvlistValue(kvl)) => JsonValue::Object(json_from_key_value_list(&kvl.values)),
        Some(Value::BytesValue(_bytes)) => {
            todo!("Add support for byte values");
        }
        None => JsonValue::Null,
    }
}

fn json_from_resource(jm: &mut JsonMap<String, JsonValue>, resource: &Resource) {
    let resource_attrs = json_from_key_value_list(&resource.attributes);
    for (key, value) in resource_attrs {
        jm.insert(format!("resource.attributes.{}", key), value);
    }

    jm.insert(
        "resource.dropped_attributes_count".to_string(),
        JsonValue::Number(serde_json::Number::from(resource.dropped_attributes_count)),
    );
}

fn json_from_instrumentation_scope(
    jm: &mut JsonMap<String, JsonValue>,
    scope: &InstrumentationScope,
) {
    jm.insert(
        "scope.name".to_string(),
        JsonValue::String(scope.name.clone()),
    );
    jm.insert(
        "scope.version".to_string(),
        JsonValue::String(scope.version.clone()),
    );

    let scope_attrs = json_from_key_value_list(&scope.attributes);
    for (key, value) in scope_attrs {
        jm.insert(format!("scope.attributes.{}", key), value);
    }

    jm.insert(
        "scope.dropped_attributes_count".to_string(),
        JsonValue::Number(serde_json::Number::from(scope.dropped_attributes_count)),
    );
}

fn json_from_log_record(jm: &mut JsonMap<String, JsonValue>, log_record: &LogRecord) {
    // Add log record fields with "log." prefix
    jm.insert(
        "log.time_unix_nano".to_string(),
        JsonValue::Number(serde_json::Number::from(log_record.time_unix_nano)),
    );
    jm.insert(
        "log.observed_time_unix_nano".to_string(),
        JsonValue::Number(serde_json::Number::from(log_record.observed_time_unix_nano)),
    );
    jm.insert(
        "log.severity_number".to_string(),
        JsonValue::Number(serde_json::Number::from(log_record.severity_number)),
    );
    jm.insert(
        "log.severity_text".to_string(),
        JsonValue::String(log_record.severity_text.clone()),
    );

    // Add body if present
    if let Some(body) = &log_record.body {
        let mut temp_map = JsonMap::new();
        temp_map.insert("body".to_string(), json_from_any_value(body));

        let flattened_body = flatten_serde_json::flatten(&temp_map);
        for (key, value) in flattened_body {
            if key != "body" {
                jm.insert(format!("log.{}", key), value);
            }
        }
    }

    // Add event name
    jm.insert(
        "log.event_name".to_string(),
        JsonValue::String(log_record.event_name.clone()),
    );

    // Add log record attributes
    let log_attrs = json_from_key_value_list(&log_record.attributes);
    for (key, value) in log_attrs {
        jm.insert(format!("log.attributes.{}", key), value);
    }

    jm.insert(
        "log.dropped_attributes_count".to_string(),
        JsonValue::Number(serde_json::Number::from(
            log_record.dropped_attributes_count,
        )),
    );
    jm.insert(
        "log.flags".to_string(),
        JsonValue::Number(serde_json::Number::from(log_record.flags)),
    );

    // Add trace_id and span_id as hex strings
    // if !log_record.trace_id.is_empty() {
    //     jm.insert(
    //         "log.trace_id".to_string(),
    //         JsonValue::String(hex::encode(&log_record.trace_id)),
    //     );
    // }
    // if !log_record.span_id.is_empty() {
    //     jm.insert(
    //         "log.span_id".to_string(),
    //         JsonValue::String(hex::encode(&log_record.span_id)),
    //     );
    // }
}

pub fn json_from_export_logs_service_request(request: &ExportLogsServiceRequest) -> JsonValue {
    let mut items = Vec::new();

    for resource_logs in &request.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                let mut jm = JsonMap::new();

                // Add resource information
                if let Some(resource) = resource_logs.resource.as_ref() {
                    json_from_resource(&mut jm, resource);
                }

                // Add resource schema URL
                if !resource_logs.schema_url.is_empty() {
                    jm.insert(
                        "resource.schema_url".to_string(),
                        JsonValue::String(resource_logs.schema_url.clone()),
                    );
                }

                // Add scope information
                if let Some(scope) = scope_logs.scope.as_ref() {
                    json_from_instrumentation_scope(&mut jm, scope);
                }

                // Add scope schema URL
                if !scope_logs.schema_url.is_empty() {
                    jm.insert(
                        "scope.schema_url".to_string(),
                        JsonValue::String(scope_logs.schema_url.clone()),
                    );
                }

                // Add log record information
                json_from_log_record(&mut jm, log_record);

                items.push(JsonValue::Object(jm));
            }
        }
    }

    JsonValue::Array(items)
}
