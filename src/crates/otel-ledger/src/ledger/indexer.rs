//! Indexer response handling.
//!
//! On `Indexed`, tracks the new SFST on the tenant's registry, then fans
//! out to the cleaner (delete the now-redundant WAL file) and the uploader
//! (upload the SFST, if storage is enabled). On `IndexFailed`, logs.

use file_registry::{ByteSize, FileId};

use crate::ipc::{CleanerRequest, IndexerResponse, UploaderRequest};

use super::Ledger;

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
