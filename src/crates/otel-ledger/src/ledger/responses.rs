//! Handlers for responses from the four worker components.

use file_registry::{ByteSize, FileId, TimestampNs};

use crate::ipc::{
    CatalogBuilderRequest, CatalogBuilderResponse, CleanerRequest, CleanerResponse,
    IndexerResponse, UploaderRequest, UploaderResponse,
};
use crate::recovery::now_ns;

use super::Ledger;
use super::helpers::{build_catalog_entry, derive_date_from_metadata};

impl Ledger {
    pub(super) async fn handle_indexer_resp(&mut self, resp: IndexerResponse) {
        match resp {
            IndexerResponse::Indexed {
                seq,
                min_date,
                metadata,
                ..
            } => {
                tracing::info!("indexed seq={seq}");

                self.pending_metadata.insert(seq, metadata);

                let tenant_id = match self.registries.for_seq(seq) {
                    Some((t, _)) => t.to_string(),
                    None => {
                        tracing::warn!("indexed unknown seq={seq}, no tenant mapping");
                        return;
                    }
                };

                let (wal_file_id, wal_path) = {
                    let registry = self
                        .registries
                        .get_mut(&tenant_id)
                        .expect("tenant present after for_seq");
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
                            tracing::warn!("indexed unknown WAL seq={seq}");
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

    pub(super) fn handle_cleaner_resp(&mut self, resp: CleanerResponse) {
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
                // Catalog files are path-keyed and paths are globally unique
                // across tenants, so the first hit is the owning tenant.
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
                    if registry.catalog_files.clear_pending_deletion(&path) {
                        break;
                    }
                }
            }
        }
    }

    pub(super) fn handle_uploader_resp(&mut self, resp: UploaderResponse) {
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

                // Both lookups hit in steady state; the guard is defensive
                // against races on restart.
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
            UploaderResponse::CatalogUploaded {
                local_path,
                remote_key,
            } => {
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

    pub(super) fn handle_catalog_builder_resp(&mut self, resp: CatalogBuilderResponse) {
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

                let remote_key =
                    crate::remote_keys::catalog(date, &tenant_id, machine_id, boot_id, max_seq);

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

    pub(super) fn request_wal_delete(&mut self, sequence: u64, path: std::path::PathBuf) {
        let req = CleanerRequest::DeleteWalFile { sequence, path };
        if let Err(e) = self.cleaner.send(req) {
            tracing::error!("failed to send WAL delete request seq={sequence}: {e}");
        }
    }

    pub(super) fn request_upload(&mut self, id: FileId, tenant_id: &str, min_date: Option<&str>) {
        if !self.logs_config.storage.enabled {
            return;
        }
        let registry = match self.registries.get(tenant_id) {
            Some(r) => r,
            None => return,
        };
        let local_path = registry.sfst.file_path(id);
        let date = min_date
            .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok())
            .unwrap_or_else(|| chrono::Utc::now().date_naive());
        let remote_key = crate::remote_keys::sfst(tenant_id, date, id);
        let req = UploaderRequest::Upload {
            seq: id.seq,
            local_path,
            remote_key,
        };
        if let Err(e) = self.uploader.send(req) {
            tracing::error!("failed to send upload request seq={}: {e}", id.seq);
        }
    }
}
