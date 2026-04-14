//! Startup recovery: replays pending work that was interrupted by a previous
//! shutdown or crash. Each function sends requests through the normal component
//! path via [`batch_recover`], so recovery and steady-state use the same code.

use std::time::{SystemTime, UNIX_EPOCH};

use file_registry::ByteSize;

use crate::component::{ComponentHandle, batch_recover, drain_pending};
use crate::ipc::{
    CleanerRequest, CleanerResponse, IndexerRequest, IndexerResponse, UploaderRequest,
    UploaderResponse,
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
            let remote_key = format!("{tenant_id}/{date}/{}", id.to_filename("sfst"));
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
