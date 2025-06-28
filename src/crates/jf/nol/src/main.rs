use flatten_otel_logs::json_from_export_logs_service_request;
use journal_file::{load_boot_id, JournalFile, JournalFileOptions, JournalWriter};
use journal_log::{JournalDirectory, JournalDirectoryConfig, RetentionPolicy, SealingPolicy};
use memmap2::MmapMut;
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::{LogsService, LogsServiceServer},
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tonic::{codec::CompressionEncoding, transport::Server, Request, Response, Status};

fn generate_uuid() -> [u8; 16] {
    uuid::Uuid::new_v4().into_bytes()
}

struct JournalManager {
    directory: Arc<Mutex<JournalDirectory>>,
    current_file: Arc<Mutex<Option<JournalFile<MmapMut>>>>,
    current_writer: Arc<Mutex<Option<JournalWriter>>>,
    boot_id: [u8; 16],
    machine_id: [u8; 16],
    seqnum_id: [u8; 16],
}

impl JournalManager {
    fn new(journal_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        // Create journal directory configuration
        let sealing_policy = SealingPolicy::new()
            .with_max_file_size(1 * 1024 * 1024) // 100MB max file size
            .with_max_entry_span(Duration::from_hours(1)); // 1 hour max span

        let retention_policy = RetentionPolicy::new()
            .with_max_files(10) // Keep max 10 files
            .with_max_total_size(50 * 1024 * 1024) // 1GB total
            .with_max_entry_age(Duration::from_days(7)); // Keep entries for 7 days

        let config = JournalDirectoryConfig::new(journal_dir)
            .with_sealing_policy(sealing_policy)
            .with_retention_policy(retention_policy);

        let directory = JournalDirectory::with_config(config)?;

        let boot_id = load_boot_id().unwrap_or_else(|_| generate_uuid());
        let machine_id = generate_uuid();
        let seqnum_id = generate_uuid();

        Ok(JournalManager {
            directory: Arc::new(Mutex::new(directory)),
            current_file: Arc::new(Mutex::new(None)),
            current_writer: Arc::new(Mutex::new(None)),
            boot_id,
            machine_id,
            seqnum_id,
        })
    }

    fn ensure_active_journal(&self) -> Result<(), Box<dyn std::error::Error>> {
        let mut current_file = self.current_file.lock().unwrap();
        let mut current_writer = self.current_writer.lock().unwrap();

        if current_file.is_none() {
            // Create a new journal file
            let mut directory = self.directory.lock().unwrap();
            let file_info = directory.new_file(None)?;

            // Get the full path for the journal file
            let file_path = directory.get_full_path(&file_info);

            let options = JournalFileOptions::new(
                self.machine_id,
                self.boot_id,
                generate_uuid(),
                self.seqnum_id,
            )
            .with_window_size(64 * 1024)
            .with_data_hash_table_buckets(4096)
            .with_field_hash_table_buckets(512)
            .with_keyed_hash(true);

            let mut journal_file = JournalFile::create(&file_path, options)?;
            let writer = JournalWriter::new(&mut journal_file)?;

            *current_file = Some(journal_file);
            *current_writer = Some(writer);
        }

        Ok(())
    }

    fn write_log_entry(&self, json_value: &Value) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_active_journal()?;

        let mut current_file = self.current_file.lock().unwrap();
        let mut current_writer = self.current_writer.lock().unwrap();

        if let (Some(ref mut journal_file), Some(ref mut writer)) =
            (current_file.as_mut(), current_writer.as_mut())
        {
            // Convert JSON log entry to key-value pairs
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

            if !entry_data.is_empty() {
                // Convert to slice references for the writer
                let entry_refs: Vec<&[u8]> = entry_data.iter().map(|v| v.as_slice()).collect();

                // Get current timestamps
                let now = SystemTime::now();
                let realtime = now
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_micros() as u64;

                // For monotonic time, we'll use the same as realtime for simplicity
                // In a real implementation, you'd want to use a proper monotonic clock
                let monotonic = realtime;

                writer.add_entry(journal_file, &entry_refs, realtime, monotonic, self.boot_id)?;
            }
        }

        Ok(())
    }
}

pub struct MyLogsService {
    journal_manager: Arc<JournalManager>,
}

impl MyLogsService {
    fn new(journal_dir: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let journal_manager = Arc::new(JournalManager::new(journal_dir)?);
        Ok(MyLogsService { journal_manager })
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
            println!("FFFFFFFFFFFFFFFFFF");
            for entry in entries {
                println!("AAAAAAAAAAAAAAAAAAAAAAAAAA");
                if let Err(e) = self.journal_manager.write_log_entry(&entry) {
                    eprintln!("Failed to write log entry: {}", e);
                    return Err(Status::internal(format!(
                        "Failed to write log entry: {}",
                        e
                    )));
                }
            }
        }

        println!("WWWWWWWWWWWWWWWWWWWWWWWWWW");

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

// Helper trait for duration constants
trait DurationExt {
    fn from_hours(hours: u64) -> Duration;
    fn from_days(days: u64) -> Duration;
}

impl DurationExt for Duration {
    fn from_hours(hours: u64) -> Duration {
        Duration::from_secs(hours * 3600)
    }

    fn from_days(days: u64) -> Duration {
        Duration::from_secs(days * 24 * 3600)
    }
}
