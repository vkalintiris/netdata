use opentelemetry_proto::tonic::{
    collector::logs::v1::ExportLogsServiceRequest,
    common::v1::{AnyValue, KeyValue},
};
use serde_json::{Map as JsonMap, Value as JsonValue};

pub type FlattenedLog = JsonMap<String, JsonValue>;

pub fn key_value_to_json(kv: &KeyValue) -> JsonValue {
    let mut map = JsonMap::new();

    if let Some(any_value) = &kv.value {
        map.insert(kv.key.clone(), any_value_to_json(any_value));
    } else {
        map.insert(kv.key.clone(), JsonValue::Null);
    }

    JsonValue::Object(map)
}

pub fn key_value_list_to_json(kvl: &Vec<KeyValue>) -> JsonValue {
    let mut map = JsonMap::new();

    for kv in kvl {
        if let Some(any_value) = &kv.value {
            map.insert(kv.key.clone(), any_value_to_json(any_value));
        } else {
            map.insert(kv.key.clone(), JsonValue::Null);
        }
    }

    JsonValue::Object(map)
}

fn any_value_to_json(any_value: &AnyValue) -> JsonValue {
    use opentelemetry_proto::tonic::common::v1::any_value::Value;

    match &any_value.value {
        Some(Value::StringValue(s)) => JsonValue::String(s.clone()),
        Some(Value::BoolValue(b)) => JsonValue::Bool(*b),
        Some(Value::IntValue(i)) => JsonValue::String(i.to_string()),
        Some(Value::DoubleValue(d)) => JsonValue::Number(
            serde_json::Number::from_f64(*d).unwrap_or_else(|| serde_json::Number::from(0)),
        ),
        Some(Value::ArrayValue(array)) => {
            let values: Vec<JsonValue> = array.values.iter().map(any_value_to_json).collect();
            JsonValue::Array(values)
        }
        Some(Value::KvlistValue(kvl)) => key_value_list_to_json(&kvl.values),
        Some(Value::BytesValue(_bytes)) => {
            todo!("At some point");
            // inner_map.insert(
            //     "bytesValue".to_string(),
            //     JsonValue::String(base64::encode(bytes)),
            // );
            // JsonValue::Object(inner_map)
        }
        None => JsonValue::Null,
    }
}

/// Flatten an OTEL ExportLogsServiceRequest into a vector of flattened logs
pub fn flatten_export_logs_request(
    request: &ExportLogsServiceRequest,
) -> Result<Vec<FlattenedLog>, serde_json::Error> {
    let mut flattened_logs = Vec::new();

    for resource_logs in &request.resource_logs {
        let Some(resource) = resource_logs.resource.as_ref() else {
            continue;
        };

        let attrs = key_value_list_to_json(&resource.attributes);
        // let attrs = JsonValue::Array(resource.attributes.iter().map(key_value_to_json).collect());
        // let json = std::mem::take(attrs.as_object_mut().unwrap());
        println!("{}", serde_json::to_string_pretty(&attrs).unwrap());

        // let mut base = serde_json::to_value(&resource_logs.resource)?;

        // let attrs = base.get_mut("attributes").unwrap();
        // flatten_key_value_array_deep(attrs).unwrap();

        // println!("Foo: {:?}", base.get("attributes").unwrap());

        // let mut json = std::mem::take(base.as_object_mut().unwrap());

        // // json.remove("attributes");

        // println!(">>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>>");
        // println!("Before");
        // println!("{}", serde_json::to_string_pretty(&json).unwrap());
        // println!("-----------------------------------------------------------");
        // println!("After");
        // let mut flattened = flatten_serde_json::flatten(&json);
        // flattened.remove("attributes");
        // println!("{}", serde_json::to_string_pretty(&flattened).unwrap());
        // println!("<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<<");

        // flattened_logs.push(flattened);

        for scope_logs in &resource_logs.scope_logs {
            for _log_record in &scope_logs.log_records {
                // // Create a combined log entry with resource, scope, and log data
                // let combined_log =
                //     create_combined_log_entry(resource_logs, scope_logs, log_record)?;

                // // Flatten the combined JSON
                // let flattened = flatten_serde_json::flatten(&combined_log);
                // flattened_logs.push(flattened);
                continue;
            }
        }
    }

    Ok(flattened_logs)
}

/// Create a combined JSON object with resource, scope, and log record data
fn create_combined_log_entry(
    resource_logs: &opentelemetry_proto::tonic::logs::v1::ResourceLogs,
    scope_logs: &opentelemetry_proto::tonic::logs::v1::ScopeLogs,
    log_record: &opentelemetry_proto::tonic::logs::v1::LogRecord,
) -> Result<JsonMap<String, JsonValue>, serde_json::Error> {
    let mut combined = JsonMap::new();

    // Add resource data under "resource" key
    if let Some(resource) = &resource_logs.resource {
        combined.insert("resource".to_string(), serde_json::to_value(resource)?);

        let v = serde_json::to_value(resource)?;
        println!("Inserted:\n{:#?}", v);
    }

    // // Add resource schema URL
    // if !resource_logs.schema_url.is_empty() {
    //     combined.insert(
    //         "resource_schema_url".to_string(),
    //         Value::String(resource_logs.schema_url.clone()),
    //     );
    // }

    // Add scope data under "scope" key
    // if let Some(scope) = &scope_logs.scope {
    //     combined.insert("scope".to_string(), serde_json::to_value(scope)?);
    // }

    // // Add scope schema URL
    // if !scope_logs.schema_url.is_empty() {
    //     combined.insert(
    //         "scope_schema_url".to_string(),
    //         Value::String(scope_logs.schema_url.clone()),
    //     );
    // }

    // Add all log record fields at the top level
    // let log_value = serde_json::to_value(log_record)?;
    // combined.insert("log".to_string(), serde_json::to_value(log_value)?);

    // if let Value::Object(log_obj) = log_value {
    //     for (key, value) in log_obj {
    //         combined.insert(key, value);
    //     }
    // }

    Ok(combined)
}
