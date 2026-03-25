pub mod cleaner;
pub mod indexer;
pub mod ipc;
pub mod registry;

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bridge::config::RetentionConfig;
use bridge::{LedgerRequest, LedgerResponse};
use ferryboat::{Connection, Endpoint};
use tokio_util::sync::CancellationToken;
use wal::{ByteSize, WalDir};

use ipc::{
    CleanerRequest, CleanerResponse, IndexerRequest, IndexerResponse, WalListener, WalReceiver,
};
use registry::Registry;

/// Ledger worker entry point.
///
/// Connects to the supervisor's IPC socket, performs the Configure → Ready
/// handshake, then runs the journal ledger event loop.
pub async fn run_worker(socket_path: &str) -> Result<()> {
    tracing::info!("connecting to supervisor socket={socket_path}");

    let mut conn: Connection<LedgerResponse, LedgerRequest> =
        Connection::connect(Endpoint::ipc(socket_path))
            .open()
            .await?;

    // Wait for Configure message from supervisor
    let config = match conn.recv().await? {
        LedgerRequest::Configure(config) => {
            tracing::info!("received plugin configuration from supervisor");
            config
        }
        other => {
            anyhow::bail!("expected Configure, got {:?}", other);
        }
    };

    // Signal ready — no function declarations yet.
    conn.send(LedgerResponse::Ready {
        declarations: vec![],
    })
    .await?;
    tracing::info!("signaled ready to supervisor");

    let mut ledger = JournalLedger::new(
        &config.writer_socket_path,
        &config.logs.wal.dir,
        &config.logs.index.dir,
        config.logs.retention.clone(),
    )
    .await
    .context("failed to initialize ledger")?;

    // Run the ledger event loop alongside supervisor IPC handling
    tokio::select! {
        result = ledger.run() => {
            result.context("ledger event loop error")?;
        }
        _req = async {
            loop {
                match conn.recv().await {
                    Ok(LedgerRequest::Call { transaction, .. }) => {
                        // No function handlers yet — return 404
                        let resp = LedgerResponse::Result(netdata_plugin_types::FunctionResult {
                            transaction,
                            status: 404,
                            format: "text/plain".to_string(),
                            expires: 0,
                            payload: b"no functions registered".to_vec(),
                        });
                        if let Err(e) = conn.send(resp).await {
                            tracing::error!("failed to send result to supervisor: {e}");
                            break;
                        }
                    }
                    Ok(LedgerRequest::Cancel { .. }) => {}
                    Ok(LedgerRequest::Shutdown) => {
                        tracing::info!("received Shutdown from supervisor");
                        break;
                    }
                    Ok(LedgerRequest::Configure(_)) => {
                        tracing::warn!("unexpected late Configure message");
                    }
                    Err(e) => {
                        tracing::error!("supervisor connection lost: {e}");
                        break;
                    }
                }
            }
        } => {}
    }

    ledger.cancel.cancel();

    Ok(())
}

pub struct JournalLedger {
    writer: WalReceiver,
    indexer: Connection<IndexerRequest, IndexerResponse>,
    cleaner: Connection<CleanerRequest, CleanerResponse>,
    registry: Registry,
    retention: RetentionConfig,
    expected_seq: u64,
    cancel: CancellationToken,
}

impl JournalLedger {
    pub async fn new(
        writer_socket_path: &str,
        wal_dir: &str,
        index_dir: &str,
        retention: RetentionConfig,
    ) -> Result<Self, ferryboat::Error> {
        let wal_path = std::path::Path::new(wal_dir);
        let index_path = std::path::Path::new(index_dir);

        let machine_id = journal_common::load_machine_id()
            .expect("failed to load machine ID");
        let boot_id = journal_common::load_boot_id()
            .expect("failed to load boot ID");
        let wal_dir = WalDir::new(wal_path, machine_id, boot_id);

        // Phase 1: Recover registries from existing files on disk.
        let mut registry = Registry::recover(wal_dir, index_path);

        let cancel = CancellationToken::new();

        // Phase 2a: Index all unindexed WAL files.
        let mut indexer = indexer::spawn(cancel.child_token()).await?;
        tracing::info!("indexer spawned");

        let unindexed = registry.unindexed_ids();
        if !unindexed.is_empty() {
            tracing::info!("indexing {} unindexed WAL files", unindexed.len());
            for &id in &unindexed {
                let path = registry.wal.dir().wal_path(id);
                let req = IndexerRequest::FinalizeIndex { path };
                indexer.send(req).await?;
            }

            let mut remaining = unindexed.len();
            while remaining > 0 {
                let resp = indexer.recv().await?;
                match resp {
                    IndexerResponse::Accepted => {}
                    IndexerResponse::IndexFinalized { seq, .. } => {
                        // The indexer already deleted the .bin.
                        let created_at_ns = registry
                            .wal
                            .remove_by_seq(seq)
                            .map(|w| w.created_at_ns)
                            .unwrap_or_default();
                        let index_path = registry.index.path(seq);
                        let index_size =
                            ByteSize(std::fs::metadata(&index_path).map(|m| m.len()).unwrap_or(0));
                        registry.index.track(seq, created_at_ns, index_size);
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
        }

        // Phase 2b: Enforce retention on index files.
        let mut cleaner = cleaner::spawn(cancel.child_token()).await?;
        tracing::info!("cleaner spawned");
        let to_evict = registry.index.evaluate_retention(
            retention.max_files,
            retention.max_total_size.as_u64(),
            retention.max_age.as_nanos() as u64,
            now_ns(),
        );
        if !to_evict.is_empty() {
            tracing::info!("retention: evicting {} old index files", to_evict.len());
            for &seq in &to_evict {
                let path = registry.index.path(seq);
                cleaner
                    .send(CleanerRequest::DeleteIndexFile {
                        sequence: seq,
                        path,
                    })
                    .await?;
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
                        tracing::error!(
                            "recovery: index eviction failed seq={sequence} error={error}"
                        );
                        remaining -= 1;
                    }
                }
            }
        }

        tracing::info!("recovery complete");

        // Accept the ingestor's connection on the writer socket.
        let mut listener = WalListener::new(writer_socket_path)?;
        let writer = listener.accept().await?;
        tracing::info!("ingestor connected to writer socket");

        Ok(Self {
            writer,
            indexer,
            cleaner,
            registry,
            retention,
            expected_seq: 1,
            cancel,
        })
    }

    pub async fn run(&mut self) -> Result<(), ferryboat::Error> {
        loop {
            tokio::select! {
                msg = self.writer.receive() => {
                    match msg {
                        Ok(msg) => self.handle_writer_msg(msg).await,
                        Err(e) => {
                            tracing::error!("publisher disconnected: {e}");
                            return Err(e);
                        }
                    }
                }
                resp = self.indexer.recv() => {
                    match resp {
                        Ok(resp) => self.handle_indexer_resp(resp).await,
                        Err(e) => {
                            tracing::error!("indexer connection lost: {e}");
                            return Err(e);
                        }
                    }
                }
                resp = self.cleaner.recv() => {
                    match resp {
                        Ok(resp) => self.handle_cleaner_resp(resp).await,
                        Err(e) => {
                            tracing::error!("cleaner connection lost: {e}");
                            return Err(e);
                        }
                    }
                }
            }
        }
    }

    async fn handle_writer_msg(&mut self, msg: wal::format::WalMessage) {
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
                tracing::info!(
                    "FileCompleted seq={seq} id={id} frames={frame_count} size={size}",
                );
            }
        }

        // Apply the event to the registry.
        if let Err(e) = self.registry.wal.apply_event(&msg.event) {
            tracing::error!("failed to apply WAL event: {e}");
            return;
        }

        // Trigger indexing on file completion.
        if let wal::format::WalEvent::FileCompleted { id, .. } = &msg.event {
            let path = self.registry.wal.dir().wal_path(*id);
            let req = IndexerRequest::FinalizeIndex { path };
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

                // The indexer already deleted the .bin — remove it from the registry.
                let created_at_ns = self
                    .registry
                    .wal
                    .remove_by_seq(seq)
                    .map(|w| w.created_at_ns)
                    .unwrap_or_default();
                let index_path = self.registry.index.path(seq);
                let index_size = ByteSize(std::fs::metadata(&index_path).map(|m| m.len()).unwrap_or(0));
                self.registry.index.track(seq, created_at_ns, index_size);

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
        let to_evict = self.registry.index.evaluate_retention(
            self.retention.max_files,
            self.retention.max_total_size.as_u64(),
            self.retention.max_age.as_nanos() as u64,
            now_ns(),
        );

        for seq in to_evict {
            self.registry.index.mark_pending_deletion(seq);
            let path = self.registry.index.path(seq);
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

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}
