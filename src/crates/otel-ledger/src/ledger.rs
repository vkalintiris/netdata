use std::collections::HashMap;

use bridge::config::LogsConfig;
use bridge::{LedgerRequest, LedgerResponse};
use ferryboat::Connection;
use file_registry::{ByteSize, FileId, TimestampNs};
use tokio_util::sync::CancellationToken;

use crate::catalog_builder::{CatalogBuilder, CatalogBuilderArgs};
use crate::cleaner::Cleaner;
use crate::component::ComponentHandle;
use crate::event::LedgerEvent;
use crate::indexer::Indexer;
use crate::ipc::{
    CatalogBuilderRequest, CatalogBuilderResponse, CleanerRequest, CleanerResponse, IndexerRequest,
    IndexerResponse, UploaderRequest, UploaderResponse,
};
use crate::recovery::{
    drain_wal_deletes, now_ns, recover_orphaned_wals, recover_retention, recover_unindexed,
    recover_unuploaded,
};
use crate::registry::TenantRegistries;
use crate::uploader::Uploader;

pub struct Ledger {
    supervisor: Connection<LedgerResponse, LedgerRequest>,
    ingestor: Connection<(), wal::Message>,
    indexer: ComponentHandle<IndexerRequest, IndexerResponse>,
    cleaner: ComponentHandle<CleanerRequest, CleanerResponse>,
    uploader: ComponentHandle<UploaderRequest, UploaderResponse>,
    catalog_builder: ComponentHandle<CatalogBuilderRequest, CatalogBuilderResponse>,
    registries: TenantRegistries,
    logs_config: LogsConfig,
    /// IndexMetadata produced by the indexer, keyed by sequence number.
    /// Populated on `IndexFinalized`. Drained on `Uploaded` when storage
    /// is enabled (the normal path). When storage is disabled, no
    /// `Uploaded` will ever fire, so entries are cleaned up on
    /// `IndexFileDeleted` when retention evicts the local SFST instead.
    pending_metadata: HashMap<u64, log_index::IndexMetadata>,
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
        let catalog_base_dir = logs_config.catalog.dir.clone();

        std::fs::create_dir_all(&wal_base_dir)?;
        std::fs::create_dir_all(&index_base_dir)?;
        std::fs::create_dir_all(&catalog_base_dir)?;

        let mut registries =
            TenantRegistries::new(wal_base_dir, index_base_dir, catalog_base_dir.clone());
        registries.discover_tenants();

        let cancel = CancellationToken::new();

        let mut indexer = ComponentHandle::spawn::<Indexer>((), cancel.child_token());
        tracing::info!("indexer spawned");
        let mut cleaner = ComponentHandle::spawn::<Cleaner>((), cancel.child_token());
        tracing::info!("cleaner spawned");

        let retry_layer = opendal::layers::RetryLayer::new()
            .with_min_delay(std::time::Duration::from_secs(1))
            .with_max_delay(std::time::Duration::from_secs(30))
            .with_max_times(10)
            .with_factor(2.0)
            .with_jitter()
            .with_notify(|err: &opendal::Error, dur: std::time::Duration| {
                tracing::warn!(
                    "remote storage operation failed, retrying in {:.1}s: {err}",
                    dur.as_secs_f64(),
                );
            });
        let operator = opendal::Operator::from_uri(logs_config.storage.uri.as_str())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?
            .layer(retry_layer);

        let mut uploader =
            ComponentHandle::spawn::<Uploader>(operator.clone(), cancel.child_token());
        tracing::info!("uploader spawned");

        let mut catalog_builder = ComponentHandle::spawn::<CatalogBuilder>(
            CatalogBuilderArgs {
                catalog_base_dir: catalog_base_dir.clone(),
                rotation_count: logs_config.catalog.rotation_count,
            },
            cancel.child_token(),
        );
        tracing::info!("catalog builder spawned");

        // Populate routing and run recovery per tenant.
        //
        // Recovery order matters:
        //   1. Delete orphaned WALs (have .sfst, WAL is redundant)
        //   2. Index unindexed WALs (no .sfst yet)
        //   3. Seed rotated / uploaded state from local catalog files
        //   4. LIST remote (if enabled) → mark uploaded and
        //      re-send uncataloged entries to the catalog builder
        //   5. Upload un-uploaded .sfst files (sends AddEntry on success)
        //   6. Evaluate retention (rotated state already reflects disk)
        let mut seq_routes: Vec<(u64, String)> = Vec::new();
        for (tenant_id, registry) in registries.iter_mut() {
            for file in registry.wal.archived_files() {
                seq_routes.push((file.id.seq, tenant_id.clone()));
            }
            for file in registry.sfst.values() {
                seq_routes.push((file.id.seq, tenant_id.clone()));
            }

            recover_orphaned_wals(registry, &mut cleaner).await?;
            recover_unindexed(registry, &mut indexer, &mut cleaner).await?;
            drain_wal_deletes(registry, &mut cleaner).await?;

            crate::recovery::seed_from_catalog_files(registry);

            if logs_config.storage.enabled {
                match crate::recovery::reconcile_remote_uploads(
                    registry,
                    &mut catalog_builder,
                    &operator,
                    tenant_id,
                )
                .await
                {
                    Ok(()) => {
                        recover_unuploaded(
                            registry,
                            &mut uploader,
                            &mut catalog_builder,
                            tenant_id,
                        )
                        .await?;
                    }
                    Err(e) => {
                        tracing::warn!(
                            tenant = tenant_id.as_str(),
                            "remote storage unreachable, skipping upload recovery: {e}"
                        );
                    }
                }
            }

            let retention =
                bridge::config::RetentionConfig::resolve(&logs_config.index.retention, tenant_id);
            recover_retention(
                registry,
                &mut cleaner,
                &retention,
                logs_config.storage.enabled,
            )
            .await?;
        }

        tracing::info!("recovery complete");

        for (seq, tenant_id) in seq_routes {
            registries.route_seq_to(seq, tenant_id);
        }

        let ingestor = crate::ipc::accept_writer(writer_socket_path).await?;
        tracing::info!("ingestor connected");

        Ok(Self {
            supervisor,
            ingestor,
            indexer,
            cleaner,
            uploader,
            catalog_builder,
            registries,
            logs_config: logs_config.clone(),
            pending_metadata: HashMap::new(),
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
                resp = self.catalog_builder.recv() => match resp {
                    Some(r) => LedgerEvent::CatalogBuilderResp(r),
                    None => break Ok(()),
                },
                req = self.supervisor.recv() => LedgerEvent::SupervisorReq(req?),
            };

            match event {
                LedgerEvent::WalMsg(msg) => self.handle_ingestor_msg(msg).await,
                LedgerEvent::IndexerResp(resp) => self.handle_indexer_resp(resp).await,
                LedgerEvent::CleanerResp(resp) => self.handle_cleaner_resp(resp),
                LedgerEvent::UploaderResp(resp) => self.handle_uploader_resp(resp),
                LedgerEvent::CatalogBuilderResp(resp) => self.handle_catalog_builder_resp(resp),
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

    async fn handle_ingestor_msg(&mut self, msg: wal::Message) {
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
            wal::FileEvent::Created { file_id, .. } => {
                tracing::info!(tenant = %tenant_id, "FileCreated seq={seq} id={file_id}");
                self.registries.route_seq_to(file_id.seq, tenant_id.clone());
            }
            wal::FileEvent::Synced {
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
            wal::FileEvent::Completed {
                file_id,
                frame_count,
                size,
                ..
            } => {
                tracing::info!(
                    tenant = %tenant_id,
                    "FileCompleted seq={seq} id={file_id} frames={frame_count} size={size}",
                );
                self.registries.route_seq_to(file_id.seq, tenant_id.clone());
            }
        }

        let registry = self.registries.get_or_create(&tenant_id);

        // Apply the event to the registry.
        if let Err(e) = registry.wal.apply_event(&msg.event) {
            tracing::error!("failed to apply WAL event: {e}");
            return;
        }

        // Trigger indexing on file completion.
        if let wal::FileEvent::Completed { file_id, .. } = msg.event {
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
            IndexerResponse::IndexFinalized {
                seq,
                min_date,
                metadata,
                ..
            } => {
                tracing::info!("index finalized seq={seq}");

                self.pending_metadata.insert(seq, metadata);

                let tenant_id = match self.registries.for_seq(seq) {
                    Some((t, _)) => t.to_string(),
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
                            let sfst_path = registry.sfst.file_path(id);
                            let sfst_size = ByteSize(
                                std::fs::metadata(&sfst_path).map(|m| m.len()).unwrap_or(0),
                            );
                            registry.sfst.track(id, wf.created_at_ns, sfst_size);
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
                let (tenant_id, sfst_info) = match self.registries.for_seq_mut(seq) {
                    Some((tid, registry)) => {
                        let info = registry.sfst.get(seq).map(|entry| (entry.id, entry.size));
                        registry.mark_uploaded(seq);
                        (tid, info)
                    }
                    None => return,
                };

                // Build the catalog entry from metadata the indexer cached on
                // IndexFinalized and forward it to the catalog builder. Both
                // lookups are expected to hit in steady state; the guard is
                // defensive against races on restart.
                if let (Some((file_id, size)), Some(metadata)) =
                    (sfst_info, self.pending_metadata.remove(&seq))
                {
                    let date = derive_date_from_metadata(&metadata);
                    let uploaded_at_ns = TimestampNs(now_ns());
                    let entry =
                        build_catalog_entry(file_id, remote_key, &metadata, size, uploaded_at_ns);

                    let req = CatalogBuilderRequest::AddEntry {
                        tenant_id,
                        date,
                        entry,
                    };
                    if let Err(e) = self.catalog_builder.send(req) {
                        tracing::error!("failed to send catalog add entry seq={seq}: {e}");
                    }
                }
            }
            UploaderResponse::UploadFailed { seq, error } => {
                tracing::error!("upload failed seq={seq}: {error}");
            }
            UploaderResponse::CatalogUploaded { local_path, remote_key } => {
                tracing::info!(
                    path = %local_path.display(),
                    remote_key = %remote_key,
                    "catalog upload complete",
                );
            }
            UploaderResponse::CatalogUploadFailed {
                local_path,
                remote_key,
                error,
            } => {
                tracing::error!(
                    path = %local_path.display(),
                    remote_key = %remote_key,
                    "catalog upload failed: {error}",
                );
            }
        }
    }

    fn handle_catalog_builder_resp(&mut self, resp: CatalogBuilderResponse) {
        match resp {
            CatalogBuilderResponse::EntryAccepted { seq } => {
                tracing::debug!(seq, "catalog entry accepted");
            }
            CatalogBuilderResponse::Rotated {
                tenant_id,
                date,
                machine_id,
                boot_id,
                max_seq,
                path,
                size,
                created_at_ns,
                seqs,
            } => {
                tracing::info!(
                    tenant = %tenant_id,
                    max_seq,
                    path = %path.display(),
                    "catalog rotated",
                );

                let remote_key = format!(
                    "{}/{}/catalog/{}",
                    date.format("%Y-%m-%d"),
                    tenant_id,
                    otel_catalog::registry::filename(machine_id, boot_id, max_seq),
                );

                if let Some(registry) = self.registries.get_mut(&tenant_id) {
                    let file = otel_catalog::registry::File::new(
                        date,
                        machine_id,
                        boot_id,
                        max_seq,
                        created_at_ns,
                        size,
                    );
                    registry.catalog_files.track(file, path.clone());
                    registry.mark_rotated_many(seqs.iter().copied());
                }

                if self.logs_config.storage.enabled {
                    let req = UploaderRequest::UploadCatalog {
                        local_path: path,
                        remote_key,
                    };
                    if let Err(e) = self.uploader.send(req) {
                        tracing::error!("failed to send catalog upload request: {e}");
                    }
                }
            }
            CatalogBuilderResponse::RotationFailed {
                tenant_id,
                max_seq,
                error,
                ..
            } => {
                tracing::error!(
                    tenant = %tenant_id,
                    max_seq,
                    "catalog rotation failed: {error}",
                );
            }
        }
    }

    fn handle_cleaner_resp(&mut self, resp: CleanerResponse) {
        match resp {
            CleanerResponse::WalFileDeleted { sequence } => {
                if let Some((_, registry)) = self.registries.for_seq_mut(sequence) {
                    registry.wal.remove_by_seq(sequence);
                }
                tracing::info!("WAL file deleted seq={sequence}");
            }
            CleanerResponse::IndexFileDeleted { sequence } => {
                if let Some((_, registry)) = self.registries.for_seq_mut(sequence) {
                    registry.evict_seq(sequence);
                }
                self.registries.forget_seq(sequence);
                self.pending_metadata.remove(&sequence);
                tracing::info!("index file evicted seq={sequence}");
            }
            CleanerResponse::WalFileFailed { sequence, error } => {
                tracing::error!("WAL file deletion failed seq={sequence} error={error}");
            }
            CleanerResponse::IndexFileFailed { sequence, error } => {
                tracing::error!("index file deletion failed seq={sequence} error={error}");
                if let Some((_, registry)) = self.registries.for_seq_mut(sequence) {
                    registry.sfst.clear_pending_deletion(sequence);
                }
            }
            CleanerResponse::CatalogFileDeleted { path } => {
                // Catalog files are path-keyed, not seq-keyed, and the
                // catalog_files registry is per-tenant. The path is unique
                // across tenants, so calling `remove` on every tenant's
                // registry is safe — only the owning tenant's entry matches.
                for (_, registry) in self.registries.iter_mut() {
                    if registry.catalog_files.remove(&path).is_some() {
                        break;
                    }
                }
                tracing::info!(path = %path.display(), "catalog file evicted");
            }
            CleanerResponse::CatalogFileFailed { path, error } => {
                tracing::error!(
                    path = %path.display(),
                    "catalog file deletion failed: {error}",
                );
                for (_, registry) in self.registries.iter_mut() {
                    registry.catalog_files.clear_pending_deletion(&path);
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
        let remote_key = format!("{}/sfst/{}/{}", tenant_id, date, id.to_filename("sfst"));
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
        let retention =
            bridge::config::RetentionConfig::resolve(&self.logs_config.index.retention, tenant_id);
        let catalog_days = catalog_retention_days(&retention);
        let today = chrono::Utc::now().date_naive();

        let registry = match self.registries.get_mut(tenant_id) {
            Some(r) => r,
            None => return,
        };

        // SFST retention pass. Uses the three-knob policy
        // (max_files / max_total_size / max_age).
        let to_evict = registry.sfst.evaluate_retention(&retention, now_ns());
        for seq in to_evict {
            // Don't evict the local SFST unless its entry is already in a
            // closed, on-disk catalog file. This covers both "not yet
            // uploaded" and "uploaded but catalog rotation hasn't happened
            // yet." Recovery can't reconstruct an in-flight accumulator
            // entry after the local SFST is gone.
            if self.logs_config.storage.enabled && !registry.is_rotated(seq) {
                tracing::warn!(
                    "retention: deferring eviction of seq={seq} (upload or catalog pending)"
                );
                continue;
            }

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

        // Catalog retention pass. Day-count derived from the tenant's
        // SFST `max_age`; see `catalog_retention_days`. A catalog file is
        // evicted when its date is strictly older than `today - max_days`.
        let to_evict_catalog = registry.catalog_files.evaluate_retention(catalog_days, today);
        for path in to_evict_catalog {
            registry.catalog_files.mark_pending_deletion(&path);
            tracing::info!("retention: evicting catalog path={}", path.display());
            let req = CleanerRequest::DeleteCatalogFile { path: path.clone() };
            if let Err(e) = self.cleaner.send(req) {
                tracing::error!(
                    path = %path.display(),
                    "failed to send catalog eviction: {e}",
                );
                registry.catalog_files.clear_pending_deletion(&path);
            }
        }
    }
}

fn derive_date_from_metadata(metadata: &log_index::IndexMetadata) -> chrono::NaiveDate {
    match metadata.histogram.timestamps.first() {
        Some(&sec) => chrono::DateTime::from_timestamp(sec as i64, 0)
            .map(|dt| dt.date_naive())
            .unwrap_or_else(|| chrono::Utc::now().date_naive()),
        None => chrono::Utc::now().date_naive(),
    }
}

/// Derive the catalog retention window (in whole days) from a tenant's
/// resolved SFST retention policy. Uses ceiling division so a non-integer
/// `max_age` in days doesn't trim catalog coverage below SFST coverage.
///
/// This is the single source of truth for "how long do local catalog
/// files live?" — there is no independent knob. See `CatalogConfig` in
/// bridge/config.rs and the doc on `evaluate_retention` for the rationale.
pub(crate) fn catalog_retention_days(retention: &bridge::config::RetentionConfig) -> u32 {
    retention
        .max_age
        .as_secs()
        .div_ceil(86_400)
        .try_into()
        .unwrap_or(u32::MAX)
}

pub(crate) fn build_catalog_entry(
    id: FileId,
    remote_key: String,
    metadata: &log_index::IndexMetadata,
    size: ByteSize,
    uploaded_at_ns: TimestampNs,
) -> otel_catalog::CatalogEntry {
    // On an empty histogram (an SFST with no logs — shouldn't happen in
    // practice) the 0 fallback yields a [0, 0] epoch range that no real
    // query will match.
    let min_timestamp_s = metadata.histogram.timestamps.first().copied().unwrap_or(0);
    let max_timestamp_s = metadata.histogram.timestamps.last().copied().unwrap_or(0);
    let streams = metadata
        .streams
        .iter()
        .map(|s| otel_catalog::StreamEntry {
            namespace: s.namespace.clone(),
            name: s.name.clone(),
        })
        .collect();
    otel_catalog::CatalogEntry {
        id,
        remote_key,
        min_timestamp_s,
        max_timestamp_s,
        total_logs: metadata.total_logs,
        streams,
        size,
        uploaded_at_ns,
    }
}
