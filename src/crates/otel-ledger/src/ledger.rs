use std::time::{SystemTime, UNIX_EPOCH};

use bridge::config::LogsConfig;
use bridge::{LedgerRequest, LedgerResponse};
use ferryboat::Connection;
use tokio_util::sync::CancellationToken;
use wal::{ByteSize, FileId, WalDir};

use crate::event::LedgerEvent;
use crate::ipc::{CleanerRequest, CleanerResponse, IndexerRequest, IndexerResponse};
use crate::registry::Registry;
use crate::{cleaner, indexer, ipc};

pub struct Ledger {
    supervisor: Connection<LedgerResponse, LedgerRequest>,
    ingestor: Connection<(), wal::format::WalMessage>,
    indexer: Connection<IndexerRequest, IndexerResponse>,
    cleaner: Connection<CleanerRequest, CleanerResponse>,
    registry: Registry,
    logs_config: LogsConfig,
    expected_seq: u64,
    pub(crate) cancel: CancellationToken,
}

impl Ledger {
    pub async fn new(
        supervisor: Connection<LedgerResponse, LedgerRequest>,
        writer_socket_path: &str,
        logs_config: &LogsConfig,
    ) -> Result<Self, ferryboat::Error> {
        let machine_id = journal_common::load_machine_id().expect("failed to load machine ID");
        let boot_id = journal_common::load_boot_id().expect("failed to load boot ID");

        let wal_dir = WalDir::new(
            std::path::Path::new(&logs_config.wal.dir),
            machine_id,
            boot_id,
        );
        let index_dir = std::path::Path::new(&logs_config.index.dir);

        let mut registry = Registry::recover(wal_dir, index_dir);
        let cancel = CancellationToken::new();

        let mut indexer = indexer::spawn(cancel.child_token()).await?;
        tracing::info!("indexer spawned");
        recover_unindexed(&mut registry, &mut indexer).await?;

        let mut cleaner = cleaner::spawn(cancel.child_token()).await?;
        tracing::info!("cleaner spawned");
        recover_retention(&mut registry, &mut cleaner, &logs_config.retention).await?;

        tracing::info!("recovery complete");

        let ingestor = ipc::accept_writer(writer_socket_path).await?;
        tracing::info!("ingestor connected");

        Ok(Self {
            supervisor,
            ingestor,
            indexer,
            cleaner,
            registry,
            logs_config: logs_config.clone(),
            expected_seq: 1,
            cancel,
        })
    }

    pub async fn run(&mut self) -> Result<(), ferryboat::Error> {
        loop {
            let event = tokio::select! {
                msg = self.ingestor.recv() => LedgerEvent::WalMsg(msg?),
                resp = self.indexer.recv() => LedgerEvent::IndexerResp(resp?),
                resp = self.cleaner.recv() => LedgerEvent::CleanerResp(resp?),
                req = self.supervisor.recv() => LedgerEvent::SupervisorReq(req?),
            };

            match event {
                LedgerEvent::WalMsg(msg) => self.handle_ingestor_msg(msg).await,
                LedgerEvent::IndexerResp(resp) => self.handle_indexer_resp(resp).await,
                LedgerEvent::CleanerResp(resp) => self.handle_cleaner_resp(resp).await,
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
                let payload = format!(
                    "otel-logs called with args: {args:?}\nwal_files={} index_files={}",
                    self.registry.wal.len(),
                    self.registry.index.len(),
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
            wal::format::WalEvent::FileCreated { id, .. } => {
                tracing::info!("FileCreated seq={seq} id={id}");
            }
            wal::format::WalEvent::FileSynced {
                id,
                frame_count,
                entry_count,
                ..
            } => {
                tracing::info!(
                    "DataSynced seq={seq} id={id} frames={frame_count} entries={entry_count}",
                );
            }
            wal::format::WalEvent::FileCompleted {
                id,
                frame_count,
                size,
                ..
            } => {
                tracing::info!("FileCompleted seq={seq} id={id} frames={frame_count} size={size}",);
            }
        }

        // Apply the event to the registry.
        if let Err(e) = self.registry.wal.apply_event(&msg.event) {
            tracing::error!("failed to apply WAL event: {e}");
            return;
        }

        // Trigger indexing on file completion.
        if let wal::format::WalEvent::FileCompleted { id, .. } = msg.event {
            let req = IndexerRequest::FinalizeIndex {
                wal_path: self.registry.wal.dir().wal_path(id),
                index_path: self.registry.index.path(id),
            };

            if let Err(e) = self.indexer.send(req).await {
                tracing::error!("failed to send to indexer: {e}");
            }
        }
    }

    async fn handle_indexer_resp(&mut self, resp: IndexerResponse) {
        match resp {
            IndexerResponse::Accepted => {}
            IndexerResponse::IndexFinalized { seq, .. } => {
                tracing::info!("index finalized seq={seq}");

                let wal_entry = self.registry.wal.remove_by_seq(seq);
                let created_at_ns = wal_entry
                    .as_ref()
                    .map(|w| w.created_at_ns)
                    .unwrap_or_default();
                if let Some(wal_entry) = wal_entry {
                    // Delete the now-redundant WAL file.
                    let wal_path = self.registry.wal.dir().wal_path(wal_entry.id);
                    remove_file(&wal_path);

                    let index_file_path = self.registry.index.path(wal_entry.id);
                    let index_size = ByteSize(
                        std::fs::metadata(&index_file_path)
                            .map(|m| m.len())
                            .unwrap_or(0),
                    );
                    self.registry
                        .index
                        .track(wal_entry.id, created_at_ns, index_size);
                } else {
                    tracing::warn!("index finalized for unknown WAL seq={seq}");
                }

                self.evaluate_retention().await;
            }
            IndexerResponse::IndexFailed { path, error } => {
                tracing::error!("indexing failed path={} error={error}", path.display());
            }
        }
    }

    async fn handle_cleaner_resp(&mut self, resp: CleanerResponse) {
        match resp {
            CleanerResponse::Accepted => {}
            CleanerResponse::IndexFileDeleted { sequence } => {
                self.registry.index.remove(sequence);
                tracing::info!("index file evicted seq={sequence}");
            }
            CleanerResponse::IndexFileFailed { sequence, error } => {
                tracing::error!("index file eviction failed seq={sequence} error={error}");
                self.registry.index.clear_pending_deletion(sequence);
            }
        }
    }

    async fn evaluate_retention(&mut self) {
        let to_evict = self
            .registry
            .index
            .evaluate_retention(&self.logs_config.retention, now_ns());

        for seq in to_evict {
            self.registry.index.mark_pending_deletion(seq);
            if let Some(entry) = self.registry.index.get(seq) {
                let path = self.registry.index.path(entry.id);
                tracing::info!("retention: evicting seq={seq} path={}", path.display());
                let req = CleanerRequest::DeleteIndexFile {
                    sequence: seq,
                    path,
                };
                if let Err(e) = self.cleaner.send(req).await {
                    tracing::error!("failed to send index eviction to cleaner seq={seq}: {e}");
                    self.registry.index.clear_pending_deletion(seq);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Recovery helpers
// ---------------------------------------------------------------------------

/// Index any WAL files that were archived but not yet indexed.
async fn recover_unindexed(
    registry: &mut Registry,
    indexer: &mut Connection<IndexerRequest, IndexerResponse>,
) -> Result<(), ferryboat::Error> {
    let unindexed = registry.unindexed_ids();
    if unindexed.is_empty() {
        return Ok(());
    }

    tracing::info!("indexing {} unindexed WAL files", unindexed.len());
    for &id in &unindexed {
        let req = IndexerRequest::FinalizeIndex {
            wal_path: registry.wal.dir().wal_path(id),
            index_path: registry.index.path(id),
        };
        indexer.send(req).await?;
    }

    let mut remaining = unindexed.len();
    while remaining > 0 {
        let resp = indexer.recv().await?;
        match resp {
            IndexerResponse::Accepted => {}
            IndexerResponse::IndexFinalized { seq, .. } => {
                let wal_entry = registry.wal.remove_by_seq(seq);
                let created_at_ns = wal_entry
                    .as_ref()
                    .map(|w| w.created_at_ns)
                    .unwrap_or_default();
                let id = wal_entry.map(|w| w.id).unwrap_or_else(|| {
                    let dir = registry.wal.dir();
                    FileId::new(dir.machine_id(), dir.boot_id(), seq)
                });

                // Delete the now-redundant WAL file.
                let wal_path = registry.wal.dir().wal_path(id);
                remove_file(&wal_path);

                let index_file_path = registry.index.path(id);
                let index_size = ByteSize(
                    std::fs::metadata(&index_file_path)
                        .map(|m| m.len())
                        .unwrap_or(0),
                );
                registry.index.track(id, created_at_ns, index_size);
                tracing::info!("recovery: index finalized seq={seq}");
                remaining -= 1;
            }
            IndexerResponse::IndexFailed {
                ref path,
                ref error,
            } => {
                tracing::error!(
                    "recovery: indexing failed path={} error={error}",
                    path.display()
                );
                remaining -= 1;
            }
        }
    }
    tracing::info!("recovery indexing complete");
    Ok(())
}

/// Evict index files that exceed the retention policy.
async fn recover_retention(
    registry: &mut Registry,
    cleaner: &mut Connection<CleanerRequest, CleanerResponse>,
    retention: &bridge::config::RetentionConfig,
) -> Result<(), ferryboat::Error> {
    let to_evict = registry.index.evaluate_retention(retention, now_ns());
    if to_evict.is_empty() {
        return Ok(());
    }

    tracing::info!("retention: evicting {} old index files", to_evict.len());
    for &seq in &to_evict {
        if let Some(entry) = registry.index.get(seq) {
            let path = registry.index.path(entry.id);
            cleaner
                .send(CleanerRequest::DeleteIndexFile {
                    sequence: seq,
                    path,
                })
                .await?;
        }
    }

    let mut remaining = to_evict.len();
    while remaining > 0 {
        let resp = cleaner.recv().await?;
        match resp {
            CleanerResponse::Accepted => {}
            CleanerResponse::IndexFileDeleted { sequence } => {
                registry.index.remove(sequence);
                tracing::info!("recovery: index file evicted seq={sequence}");
                remaining -= 1;
            }
            CleanerResponse::IndexFileFailed { sequence, error } => {
                tracing::error!("recovery: index eviction failed seq={sequence} error={error}");
                remaining -= 1;
            }
        }
    }
    Ok(())
}

fn remove_file(path: &std::path::Path) {
    match std::fs::remove_file(path) {
        Ok(()) => tracing::info!("deleted {}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("failed to delete {}: {e}", path.display()),
    }
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}
