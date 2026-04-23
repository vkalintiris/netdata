//! Ledger actor.
//!
//! Owns the four worker components (indexer, uploader, cleaner, catalog
//! builder), the per-tenant registries, and the event loop that dispatches
//! WAL messages from the ingestor, responses from the workers, and
//! requests from the supervisor.

mod helpers;
mod ingestor;
mod responses;
mod retention;
mod rpc;

pub(crate) use helpers::{build_catalog_entry, catalog_retention_days};

use std::collections::HashMap;

use bridge::config::LogsConfig;
use bridge::{LedgerRequest, LedgerResponse};
use ferryboat::Connection;
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
    drain_wal_deletes, recover_orphaned_wals, recover_retention, recover_unindexed,
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
    /// Populated on `Indexed`. Drained on `Uploaded` when storage is
    /// enabled (the normal path). When storage is disabled, no `Uploaded`
    /// will ever fire, so entries are cleaned up on `IndexFileDeleted`
    /// when retention evicts the local SFST instead.
    pending_metadata: HashMap<u64, log_index::IndexMetadata>,
    expected_frame_seq: u64,
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
            expected_frame_seq: 1,
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
}
