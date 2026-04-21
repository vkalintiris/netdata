use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use file_registry::FileId;

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

pub struct Registry {
    pub wal: wal::Registry,
    pub sfst: sfst::registry::Registry,
    /// Immutable catalog files present on local disk.
    pub catalog_files: otel_catalog::registry::Registry,
    /// SFST sequence numbers that have been successfully uploaded to remote
    /// object storage. Used to answer "do I need to upload this SFST?" and
    /// to seed the catalog builder's accumulator on recovery.
    pub uploaded_seqs: BTreeSet<u64>,
    /// SFST sequence numbers whose catalog entry has been written to a
    /// closed on-disk catalog file. Retention defers SFST eviction until
    /// this set contains the seq.
    pub rotated_seqs: BTreeSet<u64>,
}

impl Registry {
    pub fn new(
        wal: wal::Registry,
        sfst: sfst::registry::Registry,
        catalog_files: otel_catalog::registry::Registry,
    ) -> Self {
        Self {
            wal,
            sfst,
            catalog_files,
            uploaded_seqs: BTreeSet::new(),
            rotated_seqs: BTreeSet::new(),
        }
    }

    /// Recover registries from disk.
    ///
    /// Cleans up stale `.tmp` files (from interrupted index writes) before
    /// scanning.
    pub fn recover(&mut self) {
        cleanup_temp_files(self.sfst.dir());

        self.wal.recover().unwrap_or_else(|e| {
            tracing::error!("failed to recover WAL registry: {e}");
            panic!("WAL registry recovery failed");
        });
        self.sfst.recover();
        self.catalog_files.recover();

        if !self.wal.is_empty() || !self.sfst.is_empty() || !self.catalog_files.is_empty() {
            tracing::info!(
                "recovered files from disk: wal_files={} index_files={} catalog_files={}",
                self.wal.len(),
                self.sfst.len(),
                self.catalog_files.len(),
            );
        }
    }

    /// Returns FileIds of archived WAL files that have no corresponding index.
    pub fn unindexed_ids(&self) -> Vec<FileId> {
        self.wal
            .archived_files()
            .filter(|f| self.sfst.get(f.id.seq).is_none())
            .map(|f| f.id)
            .collect()
    }

    /// Returns FileIds of archived WAL files that already have a corresponding index.
    ///
    /// These are orphaned WAL files left behind by a crash between indexing
    /// completion and WAL deletion.
    pub fn orphaned_wal_ids(&self) -> Vec<FileId> {
        self.wal
            .archived_files()
            .filter(|f| self.sfst.get(f.id.seq).is_some())
            .map(|f| f.id)
            .collect()
    }

    /// Returns FileIds of indexed files that have not yet been uploaded to
    /// remote object storage.
    pub fn unuploaded_ids(&self) -> Vec<FileId> {
        self.sfst
            .values()
            .filter(|entry| !self.uploaded_seqs.contains(&entry.id.seq))
            .map(|entry| entry.id)
            .collect()
    }
}

// ---------------------------------------------------------------------------
// TenantRegistries
// ---------------------------------------------------------------------------

/// Manages per-tenant `Registry` instances, one per tenant subdirectory.
pub struct TenantRegistries {
    pub tenants: HashMap<String, Registry>,
    wal_base_dir: std::path::PathBuf,
    index_base_dir: std::path::PathBuf,
    catalog_base_dir: std::path::PathBuf,
}

impl TenantRegistries {
    pub fn new(
        wal_base_dir: std::path::PathBuf,
        index_base_dir: std::path::PathBuf,
        catalog_base_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            tenants: HashMap::new(),
            wal_base_dir,
            index_base_dir,
            catalog_base_dir,
        }
    }

    /// Get or lazily create the `Registry` for a tenant.
    ///
    /// The returned registry is **not** recovered from disk. During startup,
    /// use [`discover_tenants`] which calls `recover()` on each registry.
    /// During normal operation, the ingestor sends all events via IPC so
    /// recovery is unnecessary (and would conflict with in-flight events).
    pub fn get_or_create(&mut self, tenant_id: &str) -> &mut Registry {
        if !self.tenants.contains_key(tenant_id) {
            let wal_dir = self.wal_base_dir.join(tenant_id);
            let index_dir = self.index_base_dir.join(tenant_id);
            let wal = wal::Registry::new(&wal_dir);
            std::fs::create_dir_all(&index_dir).ok();
            let index = sfst::registry::Registry::new(&index_dir);
            // Catalog files live under `{catalog_base_dir}/{date}/{tenant}/`.
            // Per-date subdirs are created lazily by the catalog builder on
            // first rotation.
            let catalog_files = otel_catalog::registry::Registry::new(
                &self.catalog_base_dir,
                tenant_id.to_string(),
            );
            let registry = Registry::new(wal, index, catalog_files);
            self.tenants.insert(tenant_id.to_string(), registry);
        }
        self.tenants.get_mut(tenant_id).unwrap()
    }

    /// Discover tenants by scanning base directories for subdirectories
    /// and recovering their registries from disk.
    ///
    /// Must be called once at startup, before the ingestor connects.
    pub fn discover_tenants(&mut self) {
        let mut tenant_names = Vec::new();
        for base in [&self.wal_base_dir, &self.index_base_dir] {
            let entries = match std::fs::read_dir(base) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    if let Some(name) = entry.file_name().to_str() {
                        tenant_names.push(name.to_string());
                    }
                }
            }
        }
        for name in tenant_names {
            let registry = self.get_or_create(&name);
            registry.recover();
        }
        if !self.tenants.is_empty() {
            tracing::info!(
                "discovered {} tenant(s): {:?}",
                self.tenants.len(),
                self.tenants.keys().collect::<Vec<_>>(),
            );
        }
    }

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&String, &mut Registry)> {
        self.tenants.iter_mut()
    }

    pub fn get(&self, tenant_id: &str) -> Option<&Registry> {
        self.tenants.get(tenant_id)
    }

    pub fn get_mut(&mut self, tenant_id: &str) -> Option<&mut Registry> {
        self.tenants.get_mut(tenant_id)
    }
}

fn cleanup_temp_files(dir: &Path) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for dir_entry in entries.flatten() {
        let path = dir_entry.path();
        if path.extension().is_some_and(|ext| ext == "tmp") {
            match std::fs::remove_file(&path) {
                Ok(()) => tracing::info!("removed stale tmp file path={}", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(
                        "failed to remove stale tmp file path={}: {e}",
                        path.display()
                    )
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use file_registry::{ByteSize, TimestampNs};
    use uuid::Uuid;

    fn make_registry() -> Registry {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let catalog_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let catalog_files =
            otel_catalog::registry::Registry::new(catalog_dir.path(), "tenant1".to_string());
        // Keep tempdirs alive for the test's lifetime.
        std::mem::forget((wal_dir, sfst_dir, catalog_dir));
        Registry::new(wal, sfst, catalog_files)
    }

    #[test]
    fn unuploaded_ids_excludes_uploaded_seqs() {
        let mut reg = make_registry();

        for seq in [1u64, 2, 3] {
            let id = FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), seq, 0);
            reg.sfst.track(id, TimestampNs(0), ByteSize(1));
        }
        reg.uploaded_seqs.insert(2);
        reg.uploaded_seqs.insert(3);

        let unuploaded: Vec<u64> =
            reg.unuploaded_ids().iter().map(|id| id.seq).collect();
        assert_eq!(unuploaded, vec![1]);
    }

    #[test]
    fn unuploaded_ids_is_empty_when_all_uploaded() {
        let mut reg = make_registry();
        let id = FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), 5, 0);
        reg.sfst.track(id, TimestampNs(0), ByteSize(1));
        reg.uploaded_seqs.insert(5);

        assert!(reg.unuploaded_ids().is_empty());
    }

    #[test]
    fn rotated_seqs_tracks_membership() {
        let mut reg = make_registry();
        assert!(!reg.rotated_seqs.contains(&1));
        reg.rotated_seqs.insert(1);
        assert!(reg.rotated_seqs.contains(&1));
        reg.rotated_seqs.remove(&1);
        assert!(!reg.rotated_seqs.contains(&1));
    }
}
