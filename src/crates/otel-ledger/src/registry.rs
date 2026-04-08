use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use file_registry::{FileId, TimestampNs};
use crate::index;

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

    /// Recover remote state by listing files in object storage.
    ///
    /// Returns `Ok` with the recovered registry, or `Err` if the remote
    /// is unreachable. The caller should skip upload recovery on failure
    /// — uploads will happen naturally during normal operation once the
    /// remote becomes available.
    pub async fn recover(operator: &opendal::Operator) -> Result<Self, opendal::Error> {
        let mut registry = Self::new();
        let entries = operator.list("").await?;

        for entry in entries {
            let path = entry.path();
            if let Some(id) = FileId::parse(std::path::Path::new(path)) {
                registry.track(id, path.to_string());
            }
        }

        if !registry.is_empty() {
            tracing::info!("recovered {} remote files", registry.len());
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
