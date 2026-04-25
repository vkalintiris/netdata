//! Steady-state retention pass.

use file_registry::TenantId;

use crate::ipc::CleanerRequest;
use crate::recovery::now_ns;

use super::Ledger;
use super::helpers::catalog_retention_days;

impl Ledger {
    pub(super) fn evaluate_retention(&mut self, tenant_id: &TenantId) {
        let retention = bridge::config::RetentionConfig::resolve(
            &self.logs_config.index.retention,
            tenant_id.as_str(),
        );
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
        let to_evict_catalog = registry
            .catalog_files
            .evaluate_retention(catalog_days, today);
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
