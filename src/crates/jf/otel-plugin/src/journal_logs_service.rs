use flatten_otel::json_from_export_logs_service_request;
use journal_log::{JournalLog, JournalLogConfig};
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::LogsService, ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use serde_json::Value;
use std::sync::Arc;
use tonic::{Request, Response, Status};

use crate::plugin_config::LogsConfig;

pub struct NetdataLogsService {
    journal_log: Arc<JournalLog>,
}

impl NetdataLogsService {
    pub fn new(config: &LogsConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Ensure journal directory exists
        std::fs::create_dir_all(&config.journal_dir)?;

        let journal_config = JournalLogConfig::new(&config.journal_dir)
            .with_max_file_size(config.max_file_size_mb * 1024 * 1024)
            .with_max_files(config.max_files)
            .with_max_total_size(config.max_total_size_mb * 1024 * 1024)
            .with_max_entry_age_secs(config.max_entry_age_days * 24 * 3600);

        let journal_log = Arc::new(JournalLog::new(journal_config)?);
        Ok(NetdataLogsService { journal_log })
    }

    fn json_to_entry_data(&self, json_value: &Value) -> Vec<Vec<u8>> {
        let mut entry_data = Vec::new();

        if let Value::Object(obj) = json_value {
            for (key, value) in obj {
                let value_str = match value {
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    _ => serde_json::to_string(value).unwrap_or_default(),
                };

                let kv_pair = format!("{}={}", key, value_str);
                entry_data.push(kv_pair.into_bytes());
            }
        }

        entry_data
    }
}

#[tonic::async_trait]
impl LogsService for NetdataLogsService {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let req = request.into_inner();

        let json_array = json_from_export_logs_service_request(&req);

        if let Value::Array(entries) = json_array {
            for entry in entries {
                let entry_data = self.json_to_entry_data(&entry);
                if !entry_data.is_empty() {
                    let entry_refs: Vec<&[u8]> = entry_data.iter().map(|v| v.as_slice()).collect();
                    if let Err(e) = self.journal_log.write_entry(&entry_refs) {
                        eprintln!("Failed to write log entry: {}", e);
                        return Err(Status::internal(format!(
                            "Failed to write log entry: {}",
                            e
                        )));
                    }
                }
            }
        }

        let reply = ExportLogsServiceResponse {
            partial_success: None,
        };

        Ok(Response::new(reply))
    }
}
