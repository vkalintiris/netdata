use std::collections::HashMap;

use bridge::config::LogsConfig;
use bridge::{LedgerRequest, LedgerResponse};
use ferryboat::Connection;
use file_registry::{ByteSize, FileId};
use tokio_util::sync::CancellationToken;

use crate::cleaner::Cleaner;
use crate::component::ComponentHandle;
use crate::event::LedgerEvent;
use crate::indexer::Indexer;
use crate::ipc::{
    CleanerRequest, CleanerResponse, IndexerRequest, IndexerResponse, UploaderRequest,
    UploaderResponse,
};
use crate::recovery::{
    now_ns, recover_orphaned_wals, recover_retention, recover_unindexed, recover_unuploaded,
};
use crate::registry::TenantRegistries;
use crate::uploader::Uploader;

pub struct Ledger {
    supervisor: Connection<LedgerResponse, LedgerRequest>,
    ingestor: Connection<(), wal::format::WalMessage>,
    indexer: ComponentHandle<IndexerRequest, IndexerResponse>,
    cleaner: ComponentHandle<CleanerRequest, CleanerResponse>,
    uploader: ComponentHandle<UploaderRequest, UploaderResponse>,
    registries: TenantRegistries,
    logs_config: LogsConfig,
    /// Maps file sequence number → tenant ID for routing component responses.
    seq_to_tenant: HashMap<u64, String>,
    expected_seq: u64,
    pub(crate) cancel: CancellationToken,
}

impl Ledger {
    pub async fn new(
        supervisor: Connection<LedgerResponse, LedgerRequest>,
        writer_socket_path: &str,
        logs_config: &LogsConfig,
    ) -> anyhow::Result<Self> {
        let wal_base_dir = logs_config.wal.dir.clone();
        let index_base_dir = logs_config.index.dir.clone();

        std::fs::create_dir_all(&wal_base_dir)?;
        std::fs::create_dir_all(&index_base_dir)?;

        let mut registries = TenantRegistries::new(wal_base_dir, index_base_dir);
        registries.discover_tenants();

        let cancel = CancellationToken::new();

        let mut indexer = ComponentHandle::spawn::<Indexer>((), cancel.child_token());
        tracing::info!("indexer spawned");
        let mut cleaner = ComponentHandle::spawn::<Cleaner>((), cancel.child_token());
        tracing::info!("cleaner spawned");

        // Populate seq_to_tenant from recovered registries and run recovery per tenant.
        let mut seq_to_tenant = HashMap::new();
        for (tenant_id, registry) in registries.iter_mut() {
            // Record all known sequences for this tenant.
            for file in registry.wal.archived_files() {
                seq_to_tenant.insert(file.id.seq, tenant_id.clone());
            }
            for file in registry.sfst.values() {
                seq_to_tenant.insert(file.id.seq, tenant_id.clone());
            }

            let retention =
                bridge::config::RetentionConfig::resolve(&logs_config.retention, tenant_id);
            recover_orphaned_wals(registry, &mut cleaner).await?;
            recover_unindexed(registry, &mut indexer, &mut cleaner).await?;
            recover_retention(registry, &mut cleaner, &retention).await?;
        }

        let operator = opendal::Operator::from_uri(logs_config.storage.uri.as_str())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;

        let mut uploader =
            ComponentHandle::spawn::<Uploader>(operator.clone(), cancel.child_token());
        tracing::info!("uploader spawned");

        // Recover remote state and unuploaded files per tenant.
        for (tenant_id, registry) in registries.iter_mut() {
            let remote_available =
                match crate::registry::RemoteRegistry::recover(&operator, tenant_id).await {
                    Ok(remote) => {
                        registry.remote = remote;
                        true
                    }
                    Err(e) => {
                        tracing::warn!(
                            tenant = tenant_id.as_str(),
                            "remote storage unreachable, skipping upload recovery: {e}"
                        );
                        false
                    }
                };

            if logs_config.storage.enabled && remote_available {
                recover_unuploaded(registry, &mut uploader, tenant_id).await?;
            }
        }

        tracing::info!("recovery complete");

        let ingestor = crate::ipc::accept_writer(writer_socket_path).await?;
        tracing::info!("ingestor connected");

        Ok(Self {
            supervisor,
            ingestor,
            indexer,
            cleaner,
            uploader,
            registries,
            logs_config: logs_config.clone(),
            seq_to_tenant,
            expected_seq: 1,
            cancel,
        })
    }

    pub async fn run(&mut self) -> Result<(), ferryboat::Error> {
        loop {
            let event = tokio::select! {
                msg = self.ingestor.recv() => LedgerEvent::WalMsg(msg?),
                resp = self.indexer.recv() => match resp {
                    Some(r) => LedgerEvent::IndexerResp(r),
                    None => break Ok(()),
                },
                resp = self.cleaner.recv() => match resp {
                    Some(r) => LedgerEvent::CleanerResp(r),
                    None => break Ok(()),
                },
                resp = self.uploader.recv() => match resp {
                    Some(r) => LedgerEvent::UploaderResp(r),
                    None => break Ok(()),
                },
                req = self.supervisor.recv() => LedgerEvent::SupervisorReq(req?),
            };

            match event {
                LedgerEvent::WalMsg(msg) => self.handle_ingestor_msg(msg).await,
                LedgerEvent::IndexerResp(resp) => self.handle_indexer_resp(resp).await,
                LedgerEvent::CleanerResp(resp) => self.handle_cleaner_resp(resp),
                LedgerEvent::UploaderResp(resp) => self.handle_uploader_resp(resp),
                LedgerEvent::SupervisorReq(req) => {
                    if self.handle_supervisor_req(req).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    /// Handle a supervisor request. Returns `true` if the loop should exit.
    async fn handle_supervisor_req(
        &mut self,
        req: LedgerRequest,
    ) -> Result<bool, ferryboat::Error> {
        match req {
            LedgerRequest::Call {
                transaction,
                name,
                args,
                ..
            } => {
                tracing::info!("function call: name={name} args={args:?}");
                let result = self.handle_function_call(&name, &args);
                let resp = LedgerResponse::Result(netdata_plugin_types::FunctionResult {
                    transaction,
                    ..result
                });
                self.supervisor.send(resp).await?;
                Ok(false)
            }
            LedgerRequest::Cancel { .. } => Ok(false),
            LedgerRequest::Shutdown => {
                tracing::info!("received Shutdown from supervisor");
                Ok(true)
            }
            LedgerRequest::Configure(_) => {
                tracing::warn!("unexpected late Configure message");
                Ok(false)
            }
        }
    }

    fn handle_function_call(
        &self,
        name: &str,
        args: &[String],
    ) -> netdata_plugin_types::FunctionResult {
        match name {
            "otel-logs" => {
                let mut total_wal = 0;
                let mut total_index = 0;
                for (_tenant_id, registry) in self.registries.tenants.iter() {
                    total_wal += registry.wal.len();
                    total_index += registry.sfst.len();
                }
                let payload = format!(
                    "otel-logs called with args: {args:?}\ntenants={} wal_files={total_wal} index_files={total_index}",
                    self.registries.tenants.len(),
                );
                netdata_plugin_types::FunctionResult {
                    transaction: String::new(),
                    status: 200,
                    format: "text/plain".to_string(),
                    expires: 0,
                    payload: payload.into_bytes(),
                }
            }
            _ => netdata_plugin_types::FunctionResult {
                transaction: String::new(),
                status: 404,
                format: "text/plain".to_string(),
                expires: 0,
                payload: format!("unknown function: {name}").into_bytes(),
            },
        }
    }

    async fn handle_ingestor_msg(&mut self, msg: wal::format::WalMessage) {
        let seq = msg.seq;
        let tenant_id = msg.tenant_id;

        if seq != self.expected_seq {
            tracing::warn!(
                "sequence gap: expected={} got={seq} missed={}",
                self.expected_seq,
                seq - self.expected_seq,
            );
        }
        self.expected_seq = seq + 1;

        // Log before applying — extract fields for logging.
        match &msg.event {
            wal::format::WalEvent::FileCreated { file_id, .. } => {
                tracing::info!(tenant = %tenant_id, "FileCreated seq={seq} id={file_id}");
                self.seq_to_tenant.insert(file_id.seq, tenant_id.clone());
            }
            wal::format::WalEvent::FileSynced {
                file_id,
                frame_count,
                entry_count,
                ..
            } => {
                tracing::info!(
                    tenant = %tenant_id,
                    "DataSynced seq={seq} id={file_id} frames={frame_count} entries={entry_count}",
                );
            }
            wal::format::WalEvent::FileCompleted {
                file_id,
                frame_count,
                size,
                ..
            } => {
                tracing::info!(
                    tenant = %tenant_id,
                    "FileCompleted seq={seq} id={file_id} frames={frame_count} size={size}",
                );
                self.seq_to_tenant.insert(file_id.seq, tenant_id.clone());
            }
        }

        let registry = self.registries.get_or_create(&tenant_id);

        // Apply the event to the registry.
        if let Err(e) = registry.wal.apply_event(&msg.event) {
            tracing::error!("failed to apply WAL event: {e}");
            return;
        }

        // Trigger indexing on file completion.
        if let wal::format::WalEvent::FileCompleted { file_id, .. } = msg.event {
            let req = IndexerRequest::FinalizeIndex {
                wal_path: registry.wal.file_path(file_id),
                sfst_path: registry.sfst.file_path(file_id),
            };

            if let Err(e) = self.indexer.send(req) {
                tracing::error!("failed to send to indexer: {e}");
            }
        }
    }

    async fn handle_indexer_resp(&mut self, resp: IndexerResponse) {
        match resp {
            IndexerResponse::IndexFinalized { seq, min_date, .. } => {
                tracing::info!("index finalized seq={seq}");

                let tenant_id = match self.seq_to_tenant.get(&seq) {
                    Some(t) => t.clone(),
                    None => {
                        tracing::warn!("index finalized for unknown seq={seq}, no tenant mapping");
                        return;
                    }
                };

                // Extract what we need from the registry, then drop the borrow
                // so we can call methods on `self`.
                let (wal_file_id, wal_path) = {
                    let registry = self.registries.get_or_create(&tenant_id);
                    match registry.wal.get(seq) {
                        Some(wf) => {
                            let id = wf.id;
                            let wal_path = registry.wal.file_path(id);
                            let index_file_path = registry.sfst.file_path(id);
                            let index_size = ByteSize(
                                std::fs::metadata(&index_file_path)
                                    .map(|m| m.len())
                                    .unwrap_or(0),
                            );
                            registry.sfst.track(id, wf.created_at_ns, index_size);
                            (Some(id), Some(wal_path))
                        }
                        None => {
                            tracing::warn!("index finalized for unknown WAL seq={seq}");
                            (None, None)
                        }
                    }
                };

                if let (Some(id), Some(wal_path)) = (wal_file_id, wal_path) {
                    self.request_wal_delete(id.seq, wal_path);
                    self.request_upload(id, &tenant_id, min_date.as_deref());
                }

                self.evaluate_retention(&tenant_id);
            }
            IndexerResponse::IndexFailed { path, error } => {
                tracing::error!("indexing failed path={} error={error}", path.display());
            }
        }
    }

    fn handle_uploader_resp(&mut self, resp: UploaderResponse) {
        match resp {
            UploaderResponse::Uploaded { seq, remote_key } => {
                tracing::info!("upload complete seq={seq} remote_key={remote_key}");
                let tenant_id = match self.seq_to_tenant.get(&seq) {
                    Some(t) => t.clone(),
                    None => return,
                };
                if let Some(registry) = self.registries.get_mut(&tenant_id) {
                    if let Some(entry) = registry.sfst.get(seq) {
                        registry.remote.track(entry.id, remote_key);
                    }
                }
            }
            UploaderResponse::UploadFailed { seq, error } => {
                tracing::error!("upload failed seq={seq}: {error}");
            }
        }
    }

    fn handle_cleaner_resp(&mut self, resp: CleanerResponse) {
        match resp {
            CleanerResponse::WalFileDeleted { sequence } => {
                if let Some(tenant_id) = self.seq_to_tenant.get(&sequence) {
                    if let Some(registry) = self.registries.get_mut(tenant_id) {
                        registry.wal.remove_by_seq(sequence);
                    }
                }
                tracing::info!("WAL file deleted seq={sequence}");
            }
            CleanerResponse::IndexFileDeleted { sequence } => {
                let tenant_id = self.seq_to_tenant.remove(&sequence);
                if let Some(tid) = &tenant_id {
                    if let Some(registry) = self.registries.get_mut(tid) {
                        registry.sfst.remove(sequence);
                    }
                }
                tracing::info!("index file evicted seq={sequence}");
            }
            CleanerResponse::WalFileFailed { sequence, error } => {
                tracing::error!("WAL file deletion failed seq={sequence} error={error}");
            }
            CleanerResponse::IndexFileFailed { sequence, error } => {
                tracing::error!("index file deletion failed seq={sequence} error={error}");
                if let Some(tenant_id) = self.seq_to_tenant.get(&sequence) {
                    if let Some(registry) = self.registries.get_mut(tenant_id) {
                        registry.sfst.clear_pending_deletion(sequence);
                    }
                }
            }
        }
    }

    fn request_wal_delete(&mut self, sequence: u64, path: std::path::PathBuf) {
        let req = CleanerRequest::DeleteWalFile { sequence, path };
        if let Err(e) = self.cleaner.send(req) {
            tracing::error!("failed to send WAL delete request seq={sequence}: {e}");
        }
    }

    fn request_upload(&mut self, id: FileId, tenant_id: &str, min_date: Option<&str>) {
        if !self.logs_config.storage.enabled {
            return;
        }
        let registry = match self.registries.get(tenant_id) {
            Some(r) => r,
            None => return,
        };
        let local_path = registry.sfst.file_path(id);
        let date = min_date
            .map(|d| d.to_string())
            .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
        let remote_key = format!("{}/{}/{}", tenant_id, date, id.to_filename("sfst"));
        let req = UploaderRequest::Upload {
            seq: id.seq,
            local_path,
            remote_key,
        };
        if let Err(e) = self.uploader.send(req) {
            tracing::error!("failed to send upload request seq={}: {e}", id.seq);
        }
    }

    fn evaluate_retention(&mut self, tenant_id: &str) {
        let registry = match self.registries.get_mut(tenant_id) {
            Some(r) => r,
            None => return,
        };

        let retention =
            bridge::config::RetentionConfig::resolve(&self.logs_config.retention, tenant_id);
        let to_evict = registry.sfst.evaluate_retention(&retention, now_ns());

        for seq in to_evict {
            registry.sfst.mark_pending_deletion(seq);
            if let Some(entry) = registry.sfst.get(seq) {
                let path = registry.sfst.file_path(entry.id);
                tracing::info!("retention: evicting seq={seq} path={}", path.display());
                let req = CleanerRequest::DeleteIndexFile {
                    sequence: seq,
                    path,
                };
                if let Err(e) = self.cleaner.send(req) {
                    tracing::error!("failed to send index eviction seq={seq}: {e}");
                    registry.sfst.clear_pending_deletion(seq);
                }
            }
        }
    }
}
