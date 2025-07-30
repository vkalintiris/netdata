use flatten_otel::json_from_export_logs_service_request;
use journal_file::file::load_machine_id;
use journal_file::{load_boot_id, JournalFile, JournalFileOptions, JournalWriter};
use journal_log::{JournalDirectory, JournalDirectoryConfig, RetentionPolicy, SealingPolicy};
use memmap2::MmapMut;
use opentelemetry_proto::tonic::collector::logs::v1::{
    logs_service_server::LogsService, ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tonic::{Request, Response, Status};

use crate::plugin_config::LogsConfig;

fn generate_uuid() -> [u8; 16] {
    uuid::Uuid::new_v4().into_bytes()
}

pub struct JournalManager {
    directory: Arc<Mutex<JournalDirectory>>,
    current_file: Arc<Mutex<Option<JournalFile<MmapMut>>>>,
    current_writer: Arc<Mutex<Option<JournalWriter>>>,
    machine_id: [u8; 16],
    boot_id: [u8; 16],
    seqnum_id: [u8; 16],
}

impl JournalManager {
    pub fn new(config: &LogsConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Create journal directory configuration
        let sealing_policy = SealingPolicy::new()
            .with_max_file_size(config.max_file_size_mb * 1024 * 1024) // Convert MB to bytes
            .with_max_entry_span(Duration::from_secs(3600)); // 1 hour max span

        let retention_policy = RetentionPolicy::new()
            .with_max_files(config.max_files)
            .with_max_total_size(config.max_total_size_mb * 1024 * 1024) // Convert MB to bytes
            .with_max_entry_age(Duration::from_secs(config.max_entry_age_days * 24 * 3600));

        let journal_config = JournalDirectoryConfig::new(&config.journal_dir)
            .with_sealing_policy(sealing_policy)
            .with_retention_policy(retention_policy);

        let directory = JournalDirectory::with_config(journal_config)?;

        let machine_id = load_machine_id()?;
        let boot_id = load_boot_id().unwrap_or_else(|_| generate_uuid());
        let seqnum_id = generate_uuid();

        Ok(JournalManager {
            directory: Arc::new(Mutex::new(directory)),
            current_file: Arc::new(Mutex::new(None)),
            current_writer: Arc::new(Mutex::new(None)),
            machine_id,
            boot_id,
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
                self.seqnum_id,
                generate_uuid(),
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

    pub fn write_log_entry(&self, json_value: &Value) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_active_journal()?;

        let mut current_file = self.current_file.lock().unwrap();
        let mut current_writer = self.current_writer.lock().unwrap();

        let Some(journal_file) = current_file.as_mut() else {
            return Ok(());
        };
        let Some(writer) = current_writer.as_mut() else {
            return Ok(());
        };

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

        Ok(())
    }
}

pub struct NetdataLogsService {
    journal_manager: Arc<JournalManager>,
}

impl NetdataLogsService {
    pub fn new(config: &LogsConfig) -> Result<Self, Box<dyn std::error::Error>> {
        // Ensure journal directory exists
        std::fs::create_dir_all(&config.journal_dir)?;

        let journal_manager = Arc::new(JournalManager::new(config)?);
        Ok(NetdataLogsService { journal_manager })
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
                if let Err(e) = self.journal_manager.write_log_entry(&entry) {
                    eprintln!("Failed to write log entry: {}", e);
                    return Err(Status::internal(format!(
                        "Failed to write log entry: {}",
                        e
                    )));
                }
            }
        }

        let reply = ExportLogsServiceResponse {
            partial_success: None,
        };

        Ok(Response::new(reply))
    }
}
