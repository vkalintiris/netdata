use std::collections::{BTreeSet, HashMap};
use std::path::Path;

use file_registry::{FileId, TenantId};

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

pub struct Registry {
    pub wal: wal::Registry,
    pub sfst: sfst::Registry,
    /// Immutable catalog files present on local disk.
    pub catalog_files: otel_catalog::Registry,
    /// SFST sequence numbers that have been successfully uploaded to remote
    /// object storage. Gated access via [`Registry::mark_uploaded`] etc.
    uploaded_seqs: BTreeSet<u64>,
    /// SFST sequence numbers whose catalog entry has been written to a
    /// closed on-disk catalog file. Retention defers SFST eviction until
    /// this set contains the seq. Gated access via [`Registry::mark_rotated`] etc.
    rotated_seqs: BTreeSet<u64>,
}

impl Registry {
    pub fn new(
        wal: wal::Registry,
        sfst: sfst::Registry,
        catalog_files: otel_catalog::Registry,
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

    /// Mark this SFST sequence as uploaded to remote object storage.
    pub fn mark_uploaded(&mut self, seq: u64) {
        self.uploaded_seqs.insert(seq);
    }

    /// Whether this SFST sequence has been uploaded.
    pub fn is_uploaded(&self, seq: u64) -> bool {
        self.uploaded_seqs.contains(&seq)
    }

    /// Mark this SFST sequence as written into a closed, on-disk catalog file.
    /// Retention consults this set before evicting local SFSTs.
    pub fn mark_rotated(&mut self, seq: u64) {
        self.rotated_seqs.insert(seq);
    }

    /// Mark many SFST sequences as rotated in one call.
    pub fn mark_rotated_many(&mut self, seqs: impl IntoIterator<Item = u64>) {
        self.rotated_seqs.extend(seqs);
    }

    /// Whether this SFST sequence's catalog entry is in a closed catalog file.
    pub fn is_rotated(&self, seq: u64) -> bool {
        self.rotated_seqs.contains(&seq)
    }

    /// Drop all per-seq state for this sequence. Any new per-seq state
    /// added in the future must also be cleaned up here.
    pub fn evict_seq(&mut self, seq: u64) {
        self.sfst.remove(seq);
        self.uploaded_seqs.remove(&seq);
        self.rotated_seqs.remove(&seq);
    }
}

// ---------------------------------------------------------------------------
// TenantRegistries
// ---------------------------------------------------------------------------

/// Manages per-tenant `Registry` instances, one per tenant subdirectory,
/// and the sequence-number → tenant routing table used to dispatch
/// component responses back to the owning tenant.
pub struct TenantRegistries {
    pub tenants: HashMap<TenantId, Registry>,
    /// Maps an SFST sequence number to the tenant that owns it. Populated
    /// as files are created / discovered on disk and consumed by every
    /// seq-keyed response handler.
    seq_to_tenant: HashMap<u64, TenantId>,
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
            seq_to_tenant: HashMap::new(),
            wal_base_dir,
            index_base_dir,
            catalog_base_dir,
        }
    }

    /// Record that `seq` belongs to `tenant_id`. Subsequent component
    /// responses carrying this `seq` can be routed back to the right tenant
    /// via [`Self::for_seq`] / [`Self::for_seq_mut`].
    pub fn route_seq_to(&mut self, seq: u64, tenant_id: TenantId) {
        self.seq_to_tenant.insert(seq, tenant_id);
    }

    /// Apply a WAL event for `tenant_id`, creating the per-tenant registry
    /// on first sight and routing the seq on file-lifecycle events.
    pub fn apply_wal_event(
        &mut self,
        tenant_id: &TenantId,
        event: &wal::FileEvent,
    ) -> wal::Result<()> {
        // Synced fires mid-file and adds no new (seq, tenant) mapping.
        if let wal::FileEvent::Created { file_id, .. } | wal::FileEvent::Closed { file_id, .. } =
            event
        {
            self.route_seq_to(file_id.seq, tenant_id.clone());
        }
        self.get_or_create(tenant_id).wal.apply_event(event)
    }

    /// Look up the registry that owns `seq`. Returns the tenant id and a
    /// shared reference to its registry, or `None` if `seq` isn't routed.
    pub fn for_seq(&self, seq: u64) -> Option<(&TenantId, &Registry)> {
        let tenant_id = self.seq_to_tenant.get(&seq)?;
        let registry = self.tenants.get(tenant_id)?;
        Some((tenant_id, registry))
    }

    /// Mutable variant of [`Self::for_seq`]. Returns an owned [`TenantId`]
    /// so the caller can safely hold it across further mutations of `self`
    /// (cloning is a refcount bump).
    pub fn for_seq_mut(&mut self, seq: u64) -> Option<(TenantId, &mut Registry)> {
        let tenant_id = self.seq_to_tenant.get(&seq)?.clone();
        let registry = self.tenants.get_mut(&tenant_id)?;
        Some((tenant_id, registry))
    }

    /// Remove the routing entry for `seq` and return the tenant it pointed
    /// at. Used after eviction when the seq is no longer reachable.
    pub fn forget_seq(&mut self, seq: u64) -> Option<TenantId> {
        self.seq_to_tenant.remove(&seq)
    }

    /// Get or lazily create the `Registry` for a tenant. The new registry
    /// is **not** recovered from disk — callers that need on-disk state
    /// must call `Registry::recover` themselves.
    pub(crate) fn get_or_create(&mut self, tenant_id: &TenantId) -> &mut Registry {
        if !self.tenants.contains_key(tenant_id) {
            let wal_dir = self.wal_base_dir.join(tenant_id.as_str());
            let index_dir = self.index_base_dir.join(tenant_id.as_str());
            let wal = wal::Registry::new(&wal_dir);
            std::fs::create_dir_all(&index_dir).ok();
            let index = sfst::Registry::new(&index_dir);
            // Catalog files live under `{catalog_base_dir}/{date}/{tenant}/`.
            // Per-date subdirs are created lazily by the catalog builder on
            // first rotation.
            let catalog_files =
                otel_catalog::Registry::new(&self.catalog_base_dir, tenant_id.clone());
            let registry = Registry::new(wal, index, catalog_files);
            self.tenants.insert(tenant_id.clone(), registry);
        }
        self.tenants.get_mut(tenant_id).unwrap()
    }

    /// Discover tenants by scanning base directories for subdirectories
    /// and recovering their registries from disk.
    ///
    /// Must be called once at startup, before the ingestor connects.
    pub fn discover_tenants(&mut self) {
        let mut tenant_names: Vec<TenantId> = Vec::new();
        for base in [&self.wal_base_dir, &self.index_base_dir] {
            let entries = match std::fs::read_dir(base) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                if entry.file_type().map_or(false, |ft| ft.is_dir()) {
                    if let Some(name) = entry.file_name().to_str() {
                        tenant_names.push(TenantId::from(name));
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

    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&TenantId, &mut Registry)> {
        self.tenants.iter_mut()
    }

    pub fn get(&self, tenant_id: &TenantId) -> Option<&Registry> {
        self.tenants.get(tenant_id)
    }

    pub fn get_mut(&mut self, tenant_id: &TenantId) -> Option<&mut Registry> {
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
        let sfst = sfst::Registry::new(sfst_dir.path());
        let catalog_files =
            otel_catalog::Registry::new(catalog_dir.path(), TenantId::from("tenant1"));
        // Keep tempdirs alive for the test's lifetime.
        std::mem::forget((wal_dir, sfst_dir, catalog_dir));
        Registry::new(wal, sfst, catalog_files)
    }

    fn empty_summary() -> sfst::FileSummary {
        sfst::FileSummary {
            min_timestamp_s: 0,
            max_timestamp_s: 0,
            total_logs: 0,
            stream: sfst::StreamEntry::new("", ""),
        }
    }

    #[test]
    fn unuploaded_ids_excludes_uploaded_seqs() {
        let mut reg = make_registry();

        for seq in [1u64, 2, 3] {
            let id = FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), seq, 0);
            reg.sfst.track(id, TimestampNs(0), ByteSize(1), empty_summary());
        }
        reg.mark_uploaded(2);
        reg.mark_uploaded(3);

        let unuploaded: Vec<u64> = reg.unuploaded_ids().iter().map(|id| id.seq).collect();
        assert_eq!(unuploaded, vec![1]);
    }

    #[test]
    fn unuploaded_ids_is_empty_when_all_uploaded() {
        let mut reg = make_registry();
        let id = FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), 5, 0);
        reg.sfst.track(id, TimestampNs(0), ByteSize(1), empty_summary());
        reg.mark_uploaded(5);

        assert!(reg.unuploaded_ids().is_empty());
    }

    #[test]
    fn rotated_seqs_tracks_membership() {
        let mut reg = make_registry();
        assert!(!reg.is_rotated(1));
        reg.mark_rotated(1);
        assert!(reg.is_rotated(1));
        reg.evict_seq(1);
        assert!(!reg.is_rotated(1));
    }

    #[test]
    fn evict_seq_clears_all_per_seq_state() {
        let mut reg = make_registry();
        let id = FileId::new(Uuid::from_u128(1), Uuid::from_u128(2), 42, 0);
        reg.sfst.track(id, TimestampNs(0), ByteSize(1), empty_summary());
        reg.mark_uploaded(42);
        reg.mark_rotated(42);

        reg.evict_seq(42);

        assert!(reg.sfst.get(42).is_none());
        assert!(!reg.is_uploaded(42));
        assert!(!reg.is_rotated(42));
    }

    #[test]
    fn for_seq_mut_round_trips_routing() {
        let wal_base = tempfile::tempdir().unwrap();
        let index_base = tempfile::tempdir().unwrap();
        let catalog_base = tempfile::tempdir().unwrap();

        let mut tr = TenantRegistries::new(
            wal_base.path().to_path_buf(),
            index_base.path().to_path_buf(),
            catalog_base.path().to_path_buf(),
        );
        let tenant_a = TenantId::from("tenant-a");
        tr.get_or_create(&tenant_a);
        tr.route_seq_to(10, tenant_a.clone());

        let (tid, registry) = tr.for_seq_mut(10).expect("routed");
        assert_eq!(tid, tenant_a);
        registry.mark_uploaded(10);
        assert!(tr.for_seq(10).unwrap().1.is_uploaded(10));

        let forgotten = tr.forget_seq(10);
        assert_eq!(forgotten, Some(tenant_a));
        assert!(tr.for_seq(10).is_none());
    }
}
