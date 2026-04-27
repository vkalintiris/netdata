//! Indexer response handling.

use file_registry::{FileId, TenantId};

use crate::ipc::{CleanerRequest, IndexerResponse, UploaderRequest};

use super::Ledger;

impl Ledger {
    #[tracing::instrument(skip_all)]
    pub(super) async fn handle_indexer_resp(&mut self, resp: IndexerResponse) {
        match resp {
            IndexerResponse::IndexFailed { path, error } => {
                tracing::error!(path = %path.display(), "indexing failed: {error}");
            }
            IndexerResponse::Indexed {
                seq,
                min_date,
                summary,
                size,
                ..
            } => {
                tracing::info!(seq, "indexed");

                // Lookup tenant registry
                let Some((tenant_id, registry)) = self.registries.for_seq_mut(seq) else {
                    tracing::error!(seq, "indexed unknown seq; no tenant mapping");
                    return;
                };
                let Some(wal_file) = registry.wal.get(seq) else {
                    tracing::error!(seq, "indexed unknown WAL");
                    return;
                };
                let file_id = wal_file.id;
                let created_at_ns = wal_file.created_at_ns;

                let wal_path = registry.wal.file_path(file_id);

                // Track SFST file in registry. Summary fields (timestamps,
                // total logs, streams) live on the registry entry; the
                // uploader response handler reads them back from there.
                registry.sfst.track(file_id, created_at_ns, size, summary);

                // Delete WAL file
                let req = CleanerRequest::DeleteWalFile {
                    sequence: file_id.seq,
                    path: wal_path,
                };
                if let Err(e) = self.cleaner.send(req) {
                    tracing::error!(seq = file_id.seq, "failed to send WAL delete request: {e}");
                }

                // Upload it to remote storage
                self.request_upload(file_id, &tenant_id, min_date.as_deref());

                // Run retention for the tenant
                self.evaluate_retention(&tenant_id);
            }
        }
    }

    fn request_upload(&mut self, id: FileId, tenant_id: &TenantId, min_date: Option<&str>) {
        if !self.logs_config.storage.enabled {
            return;
        }
        let registry = self
            .registries
            .get(tenant_id)
            .expect("tenant present after for_seq_mut");
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
            tracing::error!(seq = id.seq, "failed to send upload request: {e}");
        }
    }
}
