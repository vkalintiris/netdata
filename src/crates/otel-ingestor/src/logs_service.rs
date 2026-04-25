use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};

use bridge::config::AuthConfig;
use file_registry::TenantId;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse, logs_service_server::LogsService,
};
use opentelemetry_proto::tonic::common::v1::any_value::Value;
use opentelemetry_proto::tonic::logs::v1::ResourceLogs;
use tonic::{Request, Response, Status};

use crate::arrow_bridge;
use crate::ledger_sender::LedgerSender;

/// Extract `service.namespace` and `service.name` from a `ResourceLogs`
/// entry's resource attributes and compute the namespace hash.
fn ns_hash_from_resource(rl: &ResourceLogs) -> u64 {
    let attrs = match rl.resource.as_ref() {
        Some(r) => &r.attributes,
        None => return 0,
    };

    let mut namespace = None;
    let mut name = None;

    for kv in attrs {
        match kv.key.as_str() {
            "service.namespace" => {
                if let Some(Value::StringValue(s)) =
                    kv.value.as_ref().and_then(|v| v.value.as_ref())
                {
                    namespace = Some(s.as_str());
                }
            }
            "service.name" => {
                if let Some(Value::StringValue(s)) =
                    kv.value.as_ref().and_then(|v| v.value.as_ref())
                {
                    name = Some(s.as_str());
                }
            }
            _ => {}
        }
    }

    file_registry::compute_ns_hash(namespace, name)
}

fn validate_tenant_id(id: &str) -> Result<(), Status> {
    if id.is_empty() || id.len() > 255 {
        return Err(Status::invalid_argument("tenant ID must be 1-255 bytes"));
    }
    if id == "." || id == ".." || id == "default" {
        return Err(Status::invalid_argument(
            "tenant ID must not be '.', '..', or 'default'",
        ));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(Status::invalid_argument(
            "tenant ID must contain only [a-zA-Z0-9._-]",
        ));
    }
    Ok(())
}

fn extract_tenant_id(
    metadata: &tonic::metadata::MetadataMap,
    auth: &AuthConfig,
) -> Result<TenantId, Status> {
    if !auth.enabled {
        return Ok(TenantId::from("default"));
    }
    let value = metadata
        .get(AuthConfig::TENANT_HEADER)
        .ok_or_else(|| Status::unauthenticated("missing tenant header"))?;
    let tenant = value
        .to_str()
        .map_err(|_| Status::invalid_argument("tenant header must be valid UTF-8"))?;
    validate_tenant_id(tenant)?;
    Ok(TenantId::from(tenant))
}

pub struct NetdataLogsService {
    writers: Mutex<HashMap<TenantId, wal::Writer>>,
    sender: LedgerSender,
    wal_base_dir: PathBuf,
    wal_config: bridge::config::WalConfig,
    seq: Arc<AtomicU64>,
    auth: AuthConfig,
}

impl NetdataLogsService {
    pub fn new(
        sender: LedgerSender,
        wal_base_dir: PathBuf,
        wal_config: bridge::config::WalConfig,
        seq: Arc<AtomicU64>,
        auth: AuthConfig,
    ) -> Self {
        Self {
            writers: Mutex::new(HashMap::new()),
            sender,
            wal_base_dir,
            wal_config,
            seq,
            auth,
        }
    }

    fn resolve_wal_config(&self, tenant_id: &str) -> wal::Config {
        let rotation =
            bridge::config::RotationConfig::resolve(&self.wal_config.rotation, tenant_id);
        wal::Config {
            rotation: wal::RotationConfig {
                max_log_entries: rotation.max_log_entries,
                max_file_size: file_registry::ByteSize(rotation.max_file_size.as_u64()),
                max_duration: Some(rotation.max_file_duration),
            },
            crc_enabled: self.wal_config.crc_enabled,
            compression_enabled: self.wal_config.compression_enabled,
        }
    }
}

#[tonic::async_trait]
impl LogsService for NetdataLogsService {
    #[tracing::instrument(skip_all, fields(received_logs))]
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        let tenant_id = extract_tenant_id(request.metadata(), &self.auth)?;
        let req = request.into_inner();

        // Group ResourceLogs by ns_hash.
        let mut groups: HashMap<u64, Vec<ResourceLogs>> = HashMap::new();
        for rl in req.resource_logs {
            let ns_hash = ns_hash_from_resource(&rl);
            groups.entry(ns_hash).or_default().push(rl);
        }

        let mut writers = self.writers.lock().unwrap();
        let writer = if let Some(w) = writers.get_mut(&tenant_id) {
            w
        } else {
            let path = self.wal_base_dir.join(tenant_id.as_str());
            let wal_config = self.resolve_wal_config(tenant_id.as_str());
            let w = wal::Writer::new(&path, wal_config, Arc::clone(&self.seq)).map_err(|e| {
                tracing::error!(%e, tenant = %tenant_id, "failed to create WAL writer");
                Status::internal("WAL writer creation failed")
            })?;
            writers.entry(tenant_id.clone()).or_insert(w)
        };

        for (ns_hash, resource_logs) in groups {
            let (data, count) = arrow_bridge::encode(resource_logs).map_err(|e| {
                tracing::error!(%e, "failed to encode Arrow");
                Status::internal("Arrow encode error")
            })?;

            writer.write_frame(ns_hash, &data, count).map_err(|e| {
                tracing::error!(%e, "failed to write WAL entry");
                Status::internal("WAL write error")
            })?;
        }

        writer.sync_all().map_err(|e| {
            tracing::error!(%e, "failed to sync WAL");
            Status::internal("WAL sync error")
        })?;

        let events = writer.take_all_events();
        self.sender.send_events(tenant_id, events);

        Ok(Response::new(ExportLogsServiceResponse {
            partial_success: None,
        }))
    }
}
