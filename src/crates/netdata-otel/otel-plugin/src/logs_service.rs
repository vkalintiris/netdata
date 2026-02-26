use anyhow::{Context, Result};
use flatten_otel::logs_direct;
use journal_common::load_machine_id;
use journal_log_writer::{Config, Log, RetentionPolicy, RotationPolicy};
use journal_registry::Origin;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse, logs_service_server::LogsService,
};
use std::sync::{Arc, Mutex};
use tonic::{Request, Response, Status};

use crate::plugin_config::PluginConfig;

pub struct NetdataLogsService {
    log: Arc<Mutex<Log>>,
    store_otlp_json: bool,
}

impl NetdataLogsService {
    pub fn new(plugin_config: PluginConfig) -> Result<Self> {
        let logs_config = plugin_config.logs;

        let rotation_policy = RotationPolicy::default()
            .with_size_of_journal_file(logs_config.size_of_journal_file.as_u64())
            .with_duration_of_journal_file(logs_config.duration_of_journal_file)
            .with_number_of_entries(logs_config.entries_of_journal_file);

        let retention_policy = RetentionPolicy::default()
            .with_number_of_journal_files(logs_config.number_of_journal_files)
            .with_size_of_journal_files(logs_config.size_of_journal_files.as_u64())
            .with_duration_of_journal_files(logs_config.duration_of_journal_files);

        let machine_id = load_machine_id()?;
        let origin = Origin {
            machine_id: Some(machine_id),
            namespace: None,
            source: journal_registry::Source::System,
        };

        let path = std::path::Path::new(&logs_config.journal_dir);
        let journal_config = Config::new(origin, rotation_policy, retention_policy);

        let journal_log = Arc::new(Mutex::new(Log::new(path, journal_config).with_context(
            || {
                format!(
                    "Failed to create journal log for directory: {}",
                    logs_config.journal_dir
                )
            },
        )?));
        Ok(NetdataLogsService {
            log: journal_log,
            store_otlp_json: logs_config.store_otlp_json,
        })
    }
}

#[tonic::async_trait]
impl LogsService for NetdataLogsService {
    #[tracing::instrument(skip_all, fields(received_logs))]
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let req = request.into_inner();

        let bump = bumpalo::Bump::new();
        let mut entries = logs_direct::prepare_log_entries(&bump, &req, self.store_otlp_json);

        tracing::Span::current().record("received_logs", entries.len());

        // Sort entries by their creation timestamp before writing to journal
        entries.sort_by_key(|e| e.sort_key);

        let mut log = self.log.lock().unwrap();
        for entry in entries.iter() {
            if entry.items.is_empty() {
                continue;
            }
            if let Err(e) = log.write_entry(entry.items, entry.source_timestamp_usec) {
                eprintln!("Failed to write log entry: {}", e);
                return Err(Status::internal(format!(
                    "Failed to write log entry: {}",
                    e
                )));
            }
        }

        if let Err(e) = log.sync() {
            eprintln!("Failed to sync journal file: {}", e);
            return Err(Status::internal(format!(
                "Failed to sync journal file: {}",
                e
            )));
        }

        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}
