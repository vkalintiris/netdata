use flatten_otel::json_from_export_logs_service_request;
use journal_log::{JournalLog, JournalLogConfig};
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::{LogsService, LogsServiceServer},
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use serde_json::Value;
use std::sync::Arc;
use tonic::{codec::CompressionEncoding, transport::Server, Request, Response, Status};

pub struct MyLogsService {
    journal_log: Arc<JournalLog>,
}

impl MyLogsService {
    fn new(journal_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let journal_config = JournalLogConfig::new(journal_dir)
            .with_max_file_size(1 * 1024 * 1024) // 1MB max file size
            .with_max_files(10) // Keep max 10 files
            .with_max_total_size(50 * 1024 * 1024) // 50MB total
            .with_max_entry_age_secs(7 * 24 * 3600); // Keep entries for 7 days

        let journal_log = Arc::new(JournalLog::new(journal_config)?);
        Ok(MyLogsService { journal_log })
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
impl LogsService for MyLogsService {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let req = request.into_inner();

        // Convert OTLP logs to flattened JSON format
        let json_array = json_from_export_logs_service_request(&req);

        println!(
            "Received {} log entries",
            json_array.as_array().map(|a| a.len()).unwrap_or(0)
        );

        // Write each log entry to the journal
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let journal_dir =
        std::env::var("JOURNAL_DIR").unwrap_or_else(|_| "/tmp/nol-journals".to_string());

    let addr = std::env::var("LISTEN_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:20000".to_string())
        .parse()?;

    // Create journal directory if it doesn't exist
    std::fs::create_dir_all(&journal_dir)?;

    let logs_service = MyLogsService::new(&journal_dir)?;

    println!("Starting OTEL logs receiver on {}", addr);
    println!("Journal directory: {}", journal_dir);

    Server::builder()
        .add_service(
            LogsServiceServer::new(logs_service).accept_compressed(CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await?;

    Ok(())
}
