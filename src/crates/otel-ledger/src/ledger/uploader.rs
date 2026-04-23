//! Uploader response handling.
//!
//! On SFST `Uploaded`, marks the seq uploaded on the tenant's registry
//! and forwards a catalog `AddEntry` to the catalog builder (using the
//! metadata cached on `Indexed`). Catalog-file uploads are terminal:
//! logged on success and failure, no further dispatch.

use file_registry::TimestampNs;

use crate::ipc::{CatalogBuilderRequest, UploaderResponse};
use crate::recovery::now_ns;

use super::Ledger;
use super::helpers::{build_catalog_entry, derive_date_from_metadata};

impl Ledger {
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
}
