use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::index;
use file_registry::{FileId, TimestampNs};

// ---------------------------------------------------------------------------
// Remote files (uploaded to object storage)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub id: FileId,
    pub remote_key: String,
    pub uploaded_at_ns: TimestampNs,
}

pub struct RemoteRegistry {
    files: BTreeMap<u64, RemoteFile>,
}

impl RemoteRegistry {
    pub fn new() -> Self {
        Self {
            files: BTreeMap::new(),
        }
    }

    /// Recover remote state by listing files under a tenant prefix in object storage.
    ///
    /// Returns `Ok` with the recovered registry, or `Err` if the remote
    /// is unreachable. The caller should skip upload recovery on failure
    /// — uploads will happen naturally during normal operation once the
    /// remote becomes available.
    pub async fn recover(
        operator: &opendal::Operator,
        tenant_id: &str,
    ) -> Result<Self, opendal::Error> {
        let mut registry = Self::new();
        let prefix = format!("{tenant_id}/");
        let entries = operator.list(&prefix).await?;

        for entry in entries {
            let path = entry.path();
            let filename = path.strip_prefix(&prefix).unwrap_or(path);
            if let Some(id) = FileId::parse(std::path::Path::new(filename)) {
                registry.track(id, path.to_string());
            }
        }

        if !registry.is_empty() {
            tracing::info!(
                tenant = tenant_id,
                "recovered {} remote files",
                registry.len()
            );
        }

        Ok(registry)
    }

    pub fn track(&mut self, id: FileId, remote_key: String) {
        self.files.insert(
            id.seq,
            RemoteFile {
                id,
                remote_key,
                uploaded_at_ns: TimestampNs(
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64,
                ),
            },
        );
    }

    pub fn contains(&self, seq: u64) -> bool {
        self.files.contains_key(&seq)
    }

    pub fn get(&self, seq: u64) -> Option<&RemoteFile> {
        self.files.get(&seq)
    }

    pub fn remove(&mut self, seq: u64) -> Option<RemoteFile> {
        self.files.remove(&seq)
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Composition
// ---------------------------------------------------------------------------

pub struct Registry {
    pub wal: wal::Registry,
    pub index: index::Registry,
    pub remote: RemoteRegistry,
}

impl Registry {
    pub fn new(wal: wal::Registry, index: index::Registry) -> Self {
        Self {
            wal,
            index,
            remote: RemoteRegistry::new(),
        }
    }

    /// Recover registries from disk.
    ///
    /// Cleans up stale `.tmp` files (from interrupted index writes) before
    /// scanning.
    pub fn recover(&mut self) {
        cleanup_temp_files(self.index.dir());

        self.wal.recover().unwrap_or_else(|e| {
            tracing::error!("failed to recover WAL registry: {e}");
            panic!("WAL registry recovery failed");
        });
        self.index.recover();

        if !self.wal.is_empty() || !self.index.is_empty() {
            tracing::info!(
                "recovered files from disk: wal_files={} index_files={}",
                self.wal.len(),
                self.index.len(),
            );
        }
    }

    /// Returns FileIds of archived WAL files that have no corresponding index.
    pub fn unindexed_ids(&self) -> Vec<FileId> {
        self.wal
            .archived_files()
            .filter(|entry| self.index.get(entry.id.seq).is_none())
            .map(|entry| entry.id)
            .collect()
    }

    /// Returns FileIds of indexed files that have not been uploaded to remote storage.
    pub fn unuploaded_ids(&self) -> Vec<FileId> {
        self.index
            .values()
            .filter(|entry| !self.remote.contains(entry.id.seq))
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
            let index = crate::index::Registry::new(&index_dir);
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
