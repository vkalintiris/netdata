//! Startup recovery: replays pending work that was interrupted by a previous
//! shutdown or crash. Each function sends requests through the normal component
//! path via [`batch_recover`], so recovery and steady-state use the same code.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use file_registry::ByteSize;
use otel_catalog::Catalog;
use uuid::Uuid;

use crate::catalog_writer;
use crate::component::{ComponentHandle, batch_recover, drain_pending};
use crate::ipc::{
    CatalogWriterRequest, CatalogWriterResponse, CleanerRequest, CleanerResponse, IndexerRequest,
    IndexerResponse, UploaderRequest, UploaderResponse,
};
use crate::registry::Registry;

/// Index any WAL files that were archived but not yet indexed.
pub async fn recover_unindexed(
    registry: &mut Registry,
    indexer: &mut ComponentHandle<IndexerRequest, IndexerResponse>,
    cleaner: &mut ComponentHandle<CleanerRequest, CleanerResponse>,
) -> anyhow::Result<()> {
    let unindexed = registry.unindexed_ids();
    if unindexed.is_empty() {
        return Ok(());
    }

    tracing::info!("indexing {} unindexed WAL files", unindexed.len());

    let requests: Vec<_> = unindexed
        .iter()
        .map(|&id| IndexerRequest::FinalizeIndex {
            wal_path: registry.wal.file_path(id),
            sfst_path: registry.sfst.file_path(id),
        })
        .collect();

    batch_recover(requests, indexer, |resp| match resp {
        IndexerResponse::IndexFinalized { seq, .. } => {
            let wf = match registry.wal.get(seq) {
                Some(wf) => wf,
                None => {
                    tracing::warn!(
                        "recovery: index finalized for unknown WAL seq={seq}, skipping cleanup"
                    );
                    return;
                }
            };
            let id = wf.id;
            let created_at_ns = wf.created_at_ns;

            // Delete the now-redundant WAL file via the cleaner.
            // The WAL entry is removed from the registry when the cleaner confirms.
            let wal_path = registry.wal.file_path(id);
            let req = CleanerRequest::DeleteWalFile {
                sequence: seq,
                path: wal_path,
            };
            if let Err(e) = cleaner.send(req) {
                tracing::error!("recovery: failed to send WAL delete seq={seq}: {e}");
            }

            let index_file_path = registry.sfst.file_path(id);
            let index_size = ByteSize(
                std::fs::metadata(&index_file_path)
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
            registry.sfst.track(id, created_at_ns, index_size);
            tracing::info!("recovery: index finalized seq={seq}");
        }
        IndexerResponse::IndexFailed {
            ref path,
            ref error,
        } => {
            tracing::error!(
                "recovery: indexing failed path={} error={error}",
                path.display()
            );
        }
    })
    .await?;

    tracing::info!("recovery indexing complete");
    Ok(())
}

/// Drain pending WAL delete responses from the cleaner.
///
/// `recover_unindexed` sends `DeleteWalFile` requests to the cleaner as a
/// side effect of indexer responses. These must be drained before any
/// subsequent `batch_recover` on the cleaner, otherwise the responses
/// interleave and get processed by the wrong handler.
pub async fn drain_wal_deletes(
    registry: &mut Registry,
    cleaner: &mut ComponentHandle<CleanerRequest, CleanerResponse>,
) -> anyhow::Result<()> {
    drain_pending(cleaner, |resp| match resp {
        CleanerResponse::WalFileDeleted { sequence } => {
            registry.wal.remove_by_seq(sequence);
            tracing::info!("recovery: WAL deleted seq={sequence}");
        }
        CleanerResponse::WalFileFailed { sequence, error } => {
            tracing::error!("recovery: WAL deletion failed seq={sequence}: {error}");
        }
        resp => {
            tracing::warn!("unexpected cleaner response during WAL drain: {resp:?}");
        }
    })
    .await
}

/// Delete WAL files that already have a corresponding .sfst index.
///
/// These are orphaned by a crash between index finalization and WAL deletion.
/// The .sfst is written atomically (via tmp + rename), so its presence
/// guarantees the index is complete and the WAL is safe to delete.
pub async fn recover_orphaned_wals(
    registry: &mut Registry,
    cleaner: &mut ComponentHandle<CleanerRequest, CleanerResponse>,
) -> anyhow::Result<()> {
    let orphaned = registry.orphaned_wal_ids();
    if orphaned.is_empty() {
        return Ok(());
    }

    tracing::info!("deleting {} orphaned WAL files", orphaned.len());

    let requests: Vec<_> = orphaned
        .iter()
        .map(|&id| CleanerRequest::DeleteWalFile {
            sequence: id.seq,
            path: registry.wal.file_path(id),
        })
        .collect();

    batch_recover(requests, cleaner, |resp| match resp {
        CleanerResponse::WalFileDeleted { sequence } => {
            registry.wal.remove_by_seq(sequence);
            tracing::info!("recovery: orphaned WAL deleted seq={sequence}");
        }
        CleanerResponse::WalFileFailed { sequence, error } => {
            tracing::error!("recovery: orphaned WAL deletion failed seq={sequence}: {error}");
        }
        resp => {
            tracing::warn!("unexpected cleaner response during orphan recovery: {resp:?}");
        }
    })
    .await
}

/// Evict index files that exceed the retention policy.
pub async fn recover_retention(
    registry: &mut Registry,
    cleaner: &mut ComponentHandle<CleanerRequest, CleanerResponse>,
    retention: &bridge::config::RetentionConfig,
) -> anyhow::Result<()> {
    let to_evict = registry.sfst.evaluate_retention(retention, now_ns());
    if to_evict.is_empty() {
        return Ok(());
    }

    tracing::info!("retention: evicting {} old index files", to_evict.len());

    let requests: Vec<_> = to_evict
        .iter()
        .filter_map(|&seq| {
            registry.sfst.get(seq).map(|entry| {
                let path = registry.sfst.file_path(entry.id);
                CleanerRequest::DeleteIndexFile {
                    sequence: seq,
                    path,
                }
            })
        })
        .collect();

    batch_recover(requests, cleaner, |resp| match resp {
        CleanerResponse::IndexFileDeleted { sequence } => {
            registry.sfst.remove(sequence);
            tracing::info!("recovery: index file evicted seq={sequence}");
        }
        CleanerResponse::IndexFileFailed { sequence, error } => {
            tracing::error!("recovery: index eviction failed seq={sequence} error={error}");
        }
        resp => {
            tracing::warn!("unexpected cleaner response during retention recovery: {resp:?}");
        }
    })
    .await
}

/// Upload index files that haven't been uploaded to remote storage yet.
pub async fn recover_unuploaded(
    registry: &mut Registry,
    uploader: &mut ComponentHandle<UploaderRequest, UploaderResponse>,
    tenant_id: &str,
) -> anyhow::Result<()> {
    let unuploaded = registry.unuploaded_ids();
    if unuploaded.is_empty() {
        return Ok(());
    }

    tracing::info!(
        tenant = tenant_id,
        "uploading {} un-uploaded index files",
        unuploaded.len()
    );

    let requests: Vec<_> = unuploaded
        .iter()
        .map(|&id| {
            let local_path = registry.sfst.file_path(id);
            let date = read_min_date(&local_path)
                .unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
            let remote_key = format!("{tenant_id}/sfst/{date}/{}", id.to_filename("sfst"));
            UploaderRequest::Upload {
                seq: id.seq,
                local_path,
                remote_key,
            }
        })
        .collect();

    batch_recover(requests, uploader, |resp| match resp {
        UploaderResponse::Uploaded { seq, remote_key } => {
            if let Some(entry) = registry.sfst.get(seq) {
                registry.remote.track(entry.id, remote_key);
            }
            tracing::info!("recovery: upload complete seq={seq}");
        }
        UploaderResponse::UploadFailed { seq, error } => {
            tracing::error!("recovery: upload failed seq={seq}: {error}");
        }
    })
    .await?;

    tracing::info!("recovery uploads complete");
    Ok(())
}

pub(crate) fn now_ns() -> u64 {
    // `Duration::as_nanos()` returns `u128`; the `u64` cast is safe until
    // year 2554 (current nanos are ~1.7e18, `u64::MAX` is ~1.8e19).
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}

/// Read the earliest log date from a `.sfst` index file's metadata.
fn read_min_date(index_path: &std::path::Path) -> Option<String> {
    let data = std::fs::read(index_path).ok()?;
    let reader = sfst::Reader::open(&data).ok()?;
    let meta = reader
        .metadata::<log_index::fst_builder::IndexMetadata>()
        .ok()?;
    let min_sec = *meta.histogram.timestamps.first()? as i64;
    chrono::DateTime::from_timestamp(min_sec, 0).map(|dt| dt.format("%Y-%m-%d").to_string())
}

type ScopeKey = (String, NaiveDate, Uuid, Uuid);

/// Reconcile catalog state against the remote registry.
///
/// For each SFST that is locally present AND already uploaded (tracked
/// in `RemoteRegistry`), check whether the scope's local catalog file
/// already contains an entry for the file. If not, read the SFST META
/// chunk to reconstruct `IndexMetadata`, build a `CatalogEntry`, and
/// forward a `Record` through the `CatalogWriter` component. The
/// component's lazy-load picks up the existing local catalog so prior
/// entries are preserved.
///
/// Also cleans up orphan `.catalog.tmp` files from interrupted writes.
pub async fn recover_catalog(
    registry: &Registry,
    catalog_writer: &mut ComponentHandle<CatalogWriterRequest, CatalogWriterResponse>,
    tenant_id: &str,
    catalog_base_dir: &Path,
) -> anyhow::Result<()> {
    let catalog_root = catalog_base_dir.join(tenant_id).join("catalog");
    cleanup_catalog_tmp_files(&catalog_root);

    // Cache one `try_load_local_catalog` call per scope.
    let mut loaded: HashMap<ScopeKey, Option<Catalog>> = HashMap::new();
    let mut requests: Vec<CatalogWriterRequest> = Vec::new();

    for sfst in registry.sfst.values() {
        let remote = match registry.remote.get(sfst.id.seq) {
            Some(r) => r,
            None => continue,
        };

        let date = match parse_date_from_remote_key(&remote.remote_key) {
            Some(d) => d,
            None => {
                tracing::warn!(
                    seq = sfst.id.seq,
                    remote_key = %remote.remote_key,
                    "catalog recovery: could not parse date from remote_key, skipping",
                );
                continue;
            }
        };

        let scope: ScopeKey = (
            tenant_id.to_string(),
            date,
            sfst.id.machine_id,
            sfst.id.boot_id,
        );

        let existing = loaded.entry(scope.clone()).or_insert_with(|| {
            try_load_local_catalog(catalog_base_dir, &scope)
        });

        if existing.as_ref().is_some_and(|c| c.entries.contains_key(&sfst.id)) {
            continue;
        }

        let sfst_path = registry.sfst.file_path(sfst.id);
        let entry = match build_catalog_entry_from_sfst(&sfst_path, sfst.size, &remote.remote_key,
                                                         remote.uploaded_at_ns, sfst.id) {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    seq = sfst.id.seq,
                    path = %sfst_path.display(),
                    "catalog recovery: skipping SFST: {e}",
                );
                continue;
            }
        };

        requests.push(CatalogWriterRequest::Record {
            seq: sfst.id.seq,
            tenant_id: tenant_id.to_string(),
            date,
            entry,
        });
    }

    if requests.is_empty() {
        return Ok(());
    }

    tracing::info!(
        tenant = tenant_id,
        "catalog recovery: replaying {} entries",
        requests.len(),
    );

    batch_recover(requests, catalog_writer, |resp| match resp {
        CatalogWriterResponse::Recorded { seq } => {
            tracing::debug!(seq, "catalog recovery: recorded");
        }
        CatalogWriterResponse::RecordFailed { seq, stage, error } => {
            tracing::error!(seq, stage = ?stage, "catalog recovery: failed: {error}");
        }
    })
    .await
}

fn cleanup_catalog_tmp_files(catalog_root: &Path) {
    let dates = match std::fs::read_dir(catalog_root) {
        Ok(d) => d,
        Err(_) => return,
    };
    for date_entry in dates.flatten() {
        if !date_entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
            continue;
        }
        let files = match std::fs::read_dir(date_entry.path()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".catalog.tmp"))
            {
                match std::fs::remove_file(&path) {
                    Ok(()) => tracing::info!("removed stale catalog tmp path={}", path.display()),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        tracing::warn!(
                            "failed to remove stale catalog tmp path={}: {e}",
                            path.display()
                        );
                    }
                }
            }
        }
    }
}

fn parse_date_from_remote_key(key: &str) -> Option<NaiveDate> {
    // Expected shape: `{tenant}/sfst/{YYYY-MM-DD}/{file_id}.sfst`
    let mut parts = key.split('/');
    let _tenant = parts.next()?;
    let prefix = parts.next()?;
    if prefix != "sfst" {
        return None;
    }
    let date_str = parts.next()?;
    NaiveDate::parse_from_str(date_str, "%Y-%m-%d").ok()
}

fn try_load_local_catalog(base: &Path, scope: &ScopeKey) -> Option<Catalog> {
    let (tenant_id, date, machine_id, boot_id) = scope;
    let path: PathBuf =
        catalog_writer::local_path(base, tenant_id, *date, *machine_id, *boot_id);
    let bytes = std::fs::read(&path).ok()?;
    Catalog::from_json(&bytes).ok()
}

// TODO: reads the entire SFST file into memory just to decode the META
// chunk. Fine at current SFST sizes (few MB to tens of MB, bounded by WAL
// rotation), but wasteful per entry. A bounded header-range read (or mmap
// wrapper) would cap memory regardless of file size.
fn build_catalog_entry_from_sfst(
    sfst_path: &Path,
    size: ByteSize,
    remote_key: &str,
    uploaded_at_ns: file_registry::TimestampNs,
    id: file_registry::FileId,
) -> anyhow::Result<otel_catalog::CatalogEntry> {
    let data = std::fs::read(sfst_path)
        .map_err(|e| anyhow::anyhow!("read sfst: {e}"))?;
    let reader = sfst::Reader::open(&data)
        .map_err(|e| anyhow::anyhow!("open sfst: {e}"))?;
    let metadata: log_index::IndexMetadata = reader
        .metadata()
        .map_err(|e| anyhow::anyhow!("read sfst metadata: {e}"))?;
    Ok(crate::ledger::build_catalog_entry(
        id,
        remote_key.to_string(),
        &metadata,
        size,
        uploaded_at_ns,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_date_from_remote_key_happy_path() {
        let key = "tenant1/sfst/2026-04-17/abc123.sfst";
        let date = parse_date_from_remote_key(key).unwrap();
        assert_eq!(date, NaiveDate::from_ymd_opt(2026, 4, 17).unwrap());
    }

    #[test]
    fn parse_date_from_remote_key_rejects_unknown_shapes() {
        assert!(parse_date_from_remote_key("").is_none());
        assert!(parse_date_from_remote_key("tenant1").is_none());
        assert!(parse_date_from_remote_key("tenant1/catalog/2026-04-17/x").is_none());
        assert!(parse_date_from_remote_key("tenant1/sfst/not-a-date/x").is_none());
        assert!(parse_date_from_remote_key("tenant1/sfst").is_none());
    }
}
