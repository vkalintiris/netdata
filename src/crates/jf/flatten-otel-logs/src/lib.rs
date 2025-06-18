use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use serde_json::{Map, Value};

pub type FlattenedLog = Map<String, Value>;

/// Flatten an OTEL ExportLogsServiceRequest into a vector of flattened logs
pub fn flatten_export_logs_request(
    request: &ExportLogsServiceRequest,
) -> Result<Vec<FlattenedLog>, serde_json::Error> {
    let mut flattened_logs = Vec::new();

    for resource_logs in &request.resource_logs {
        for scope_logs in &resource_logs.scope_logs {
            for log_record in &scope_logs.log_records {
                // Create a combined log entry with resource, scope, and log data
                let combined_log =
                    create_combined_log_entry(resource_logs, scope_logs, log_record)?;

                // Flatten the combined JSON
                let flattened = flatten_serde_json::flatten(&combined_log);
                flattened_logs.push(flattened);
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
) -> Result<Map<String, Value>, serde_json::Error> {
    let mut combined = Map::new();

    // Add resource data under "resource" key
    if let Some(resource) = &resource_logs.resource {
        combined.insert("resource".to_string(), serde_json::to_value(resource)?);
    }

    // Add resource schema URL
    if !resource_logs.schema_url.is_empty() {
        combined.insert(
            "resource_schema_url".to_string(),
            Value::String(resource_logs.schema_url.clone()),
        );
    }

    // Add scope data under "scope" key
    if let Some(scope) = &scope_logs.scope {
        combined.insert("scope".to_string(), serde_json::to_value(scope)?);
    }

    // Add scope schema URL
    if !scope_logs.schema_url.is_empty() {
        combined.insert(
            "scope_schema_url".to_string(),
            Value::String(scope_logs.schema_url.clone()),
        );
    }

    // Add all log record fields at the top level
    let log_value = serde_json::to_value(log_record)?;
    if let Value::Object(log_obj) = log_value {
        for (key, value) in log_obj {
            combined.insert(key, value);
        }
    }

    Ok(combined)
}
