use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use file_registry::FileId;

// ---------------------------------------------------------------------------
// Catalog registry (ledger-side view of uploaded + cataloged state)
// ---------------------------------------------------------------------------

/// Whether a catalog `Record` for an uploaded SFST has been acknowledged by
/// the `CatalogWriter`. `Pending` entries block retention eviction because
/// recovery may need to re-read the local SFST header to rebuild the entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Pending,
    Persisted,
}

#[derive(Debug, Clone)]
pub struct CatalogRegistryEntry {
    pub entry: otel_catalog::CatalogEntry,
    pub visibility: Visibility,
}

pub struct CatalogRegistry {
    entries: BTreeMap<u64, CatalogRegistryEntry>,
}

impl CatalogRegistry {
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Insert a newly-uploaded SFST as `Pending`. Overwrites any existing
    /// entry for the same `seq` (idempotent on retry / duplicate Uploaded).
    pub fn insert_pending(&mut self, entry: otel_catalog::CatalogEntry) {
        self.entries.insert(
            entry.id.seq,
            CatalogRegistryEntry {
                entry,
                visibility: Visibility::Pending,
            },
        );
    }

    /// Insert an entry as already `Persisted`. Used at startup when loading
    /// entries from local catalog files on disk.
    pub fn insert_persisted(&mut self, entry: otel_catalog::CatalogEntry) {
        self.entries.insert(
            entry.id.seq,
            CatalogRegistryEntry {
                entry,
                visibility: Visibility::Persisted,
            },
        );
    }

    /// Flip the entry for `seq` to `Persisted`. No-op if not present.
    pub fn mark_persisted(&mut self, seq: u64) {
        if let Some(e) = self.entries.get_mut(&seq) {
            e.visibility = Visibility::Persisted;
        }
    }

    pub fn contains(&self, seq: u64) -> bool {
        self.entries.contains_key(&seq)
    }

    pub fn get(&self, seq: u64) -> Option<&CatalogRegistryEntry> {
        self.entries.get(&seq)
    }

    pub fn remove(&mut self, seq: u64) -> Option<CatalogRegistryEntry> {
        self.entries.remove(&seq)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter_pending(&self) -> impl Iterator<Item = &CatalogRegistryEntry> {
        self.entries
            .values()
            .filter(|e| e.visibility == Visibility::Pending)
    }

    pub fn is_persisted(&self, seq: u64) -> bool {
        self.entries
            .get(&seq)
            .is_some_and(|e| e.visibility == Visibility::Persisted)
    }
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

pub struct Registry {
    pub wal: wal::Registry,
    pub sfst: sfst::registry::Registry,
    pub catalog: CatalogRegistry,
}

impl Registry {
    pub fn new(wal: wal::Registry, sfst: sfst::registry::Registry) -> Self {
        Self {
            wal,
            sfst,
            catalog: CatalogRegistry::new(),
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

        if !self.wal.is_empty() || !self.sfst.is_empty() {
            tracing::info!(
                "recovered files from disk: wal_files={} index_files={}",
                self.wal.len(),
                self.sfst.len(),
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

    /// Returns FileIds of indexed files that have not been uploaded to remote storage.
    ///
    /// "Uploaded" means present in `catalog` regardless of visibility: a `Pending`
    /// entry was successfully uploaded but its catalog `Record` has not yet been
    /// acknowledged — re-uploading would not make progress.
    pub fn unuploaded_ids(&self) -> Vec<FileId> {
        self.sfst
            .values()
            .filter(|entry| !self.catalog.contains(entry.id.seq))
            .map(|entry| entry.id)
            .collect()
    }

    /// Returns FileIds of uploaded SFSTs whose catalog `Record` has not yet
    /// been acknowledged by the `CatalogWriter`.
    pub fn uncataloged_ids(&self) -> Vec<FileId> {
        self.catalog.iter_pending().map(|e| e.entry.id).collect()
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
}

impl TenantRegistries {
    pub fn new(wal_base_dir: std::path::PathBuf, index_base_dir: std::path::PathBuf) -> Self {
        Self {
            tenants: HashMap::new(),
            wal_base_dir,
            index_base_dir,
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
            let registry = Registry::new(wal, index);
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
    use otel_catalog::CatalogEntry;
    use uuid::Uuid;

    fn make_entry(seq: u64) -> CatalogEntry {
        CatalogEntry {
            id: FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), seq, 0),
            remote_key: format!("t/sfst/2026-04-17/{seq}.sfst"),
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 10,
            streams: vec![],
            size: ByteSize(1024),
            uploaded_at_ns: TimestampNs(1_700_000_000_000_000_000),
        }
    }

    #[test]
    fn insert_pending_is_visible_and_pending() {
        let mut cr = CatalogRegistry::new();
        cr.insert_pending(make_entry(1));

        assert!(cr.contains(1));
        assert_eq!(cr.len(), 1);
        assert_eq!(cr.get(1).unwrap().visibility, Visibility::Pending);
        assert_eq!(cr.iter_pending().count(), 1);
    }

    #[test]
    fn mark_persisted_flips_visibility_and_removes_from_pending_iter() {
        let mut cr = CatalogRegistry::new();
        cr.insert_pending(make_entry(1));
        cr.mark_persisted(1);

        assert_eq!(cr.get(1).unwrap().visibility, Visibility::Persisted);
        assert_eq!(cr.iter_pending().count(), 0);
        assert!(cr.contains(1));
    }

    #[test]
    fn mark_persisted_missing_seq_is_noop() {
        let mut cr = CatalogRegistry::new();
        cr.mark_persisted(42);
        assert!(!cr.contains(42));
        assert_eq!(cr.len(), 0);
    }

    #[test]
    fn insert_pending_overwrites_existing_entry() {
        let mut cr = CatalogRegistry::new();
        cr.insert_pending(make_entry(1));
        cr.mark_persisted(1);
        cr.insert_pending(make_entry(1));

        assert_eq!(cr.get(1).unwrap().visibility, Visibility::Pending);
        assert_eq!(cr.len(), 1);
    }

    #[test]
    fn remove_returns_entry_and_clears() {
        let mut cr = CatalogRegistry::new();
        cr.insert_pending(make_entry(1));
        let removed = cr.remove(1).unwrap();

        assert_eq!(removed.entry.id.seq, 1);
        assert!(!cr.contains(1));
        assert!(cr.is_empty());
    }

    #[test]
    fn iter_pending_filters_mixed_visibilities() {
        let mut cr = CatalogRegistry::new();
        cr.insert_pending(make_entry(1));
        cr.insert_pending(make_entry(2));
        cr.insert_pending(make_entry(3));
        cr.mark_persisted(2);

        let pending: Vec<u64> = cr.iter_pending().map(|e| e.entry.id.seq).collect();
        assert_eq!(pending, vec![1, 3]);
    }

    #[test]
    fn uncataloged_ids_returns_only_pending() {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        reg.catalog.insert_pending(make_entry(1));
        reg.catalog.insert_pending(make_entry(2));
        reg.catalog.mark_persisted(2);

        let pending: Vec<u64> = reg.uncataloged_ids().iter().map(|id| id.seq).collect();
        assert_eq!(pending, vec![1]);
    }

    #[test]
    fn is_persisted_only_true_for_persisted_entries() {
        let mut cr = CatalogRegistry::new();
        assert!(!cr.is_persisted(1));

        cr.insert_pending(make_entry(1));
        assert!(!cr.is_persisted(1));

        cr.mark_persisted(1);
        assert!(cr.is_persisted(1));
    }

    #[test]
    fn unuploaded_ids_excludes_pending_and_persisted() {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        // Three indexed SFSTs exist locally.
        let seqs = [1u64, 2, 3];
        for &seq in &seqs {
            let id = FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), seq, 0);
            reg.sfst.track(id, TimestampNs(0), ByteSize(1));
        }
        // seq=1 is still unuploaded; seq=2 is Pending; seq=3 is Persisted.
        reg.catalog.insert_pending(make_entry(2));
        reg.catalog.insert_pending(make_entry(3));
        reg.catalog.mark_persisted(3);

        let unuploaded: Vec<u64> = reg.unuploaded_ids().iter().map(|id| id.seq).collect();
        assert_eq!(unuploaded, vec![1]);
    }

}
