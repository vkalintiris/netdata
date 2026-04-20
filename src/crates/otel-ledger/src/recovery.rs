//! Startup recovery: replays pending work that was interrupted by a previous
//! shutdown or crash. Each function sends requests through the normal component
//! path via [`batch_recover`], so recovery and steady-state use the same code.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use file_registry::ByteSize;
use otel_catalog::Catalog;

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
    storage_enabled: bool,
) -> anyhow::Result<()> {
    let to_evict = registry.sfst.evaluate_retention(retention, now_ns());
    if to_evict.is_empty() {
        return Ok(());
    }

    // Mirror the steady-state retention guard at ledger.rs: when remote
    // storage is enabled, defer eviction unless the catalog entry is
    // Persisted. A Pending entry means the catalog Record hasn't been
    // acknowledged -- evicting the local SFST now would make the next
    // restart unable to rebuild the catalog entry from its header.
    let (evictable, deferred): (Vec<u64>, Vec<u64>) = to_evict
        .into_iter()
        .partition(|&seq| !storage_enabled || registry.catalog.is_persisted(seq));
    for seq in deferred {
        tracing::warn!("recovery: deferring eviction of seq={seq} (upload or catalog pending)");
    }
    if evictable.is_empty() {
        return Ok(());
    }

    tracing::info!("retention: evicting {} old index files", evictable.len());

    let requests: Vec<_> = evictable
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
            registry.catalog.remove(sequence);
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
            let (id, size, sfst_path) = match registry.sfst.get(seq) {
                Some(entry) => (entry.id, entry.size, registry.sfst.file_path(entry.id)),
                None => {
                    tracing::warn!("recovery: upload complete for unknown seq={seq}");
                    return;
                }
            };
            let uploaded_at_ns = file_registry::TimestampNs(now_ns());
            match build_catalog_entry_from_sfst(&sfst_path, size, &remote_key, uploaded_at_ns, id) {
                Ok(entry) => {
                    registry.catalog.insert_pending(entry);
                }
                Err(e) => {
                    tracing::warn!(
                        seq,
                        path = %sfst_path.display(),
                        "recovery: failed to rebuild catalog entry: {e}",
                    );
                }
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

/// Load entries from local catalog files on disk and insert them into
/// `registry.catalog` as `Persisted`.
///
/// Walks `{catalog_base_dir}/{tenant_id}/catalog/*/` and parses each
/// `*.catalog` JSON file. Only entries whose `seq` is in the current
/// `sfst` registry are registered — historical entries for evicted SFSTs
/// remain in the on-disk file but don't occupy registry memory.
///
/// Corrupt catalog files are logged and skipped; the `CatalogWriter`'s
/// lazy-load handles the `.bad` rename on its next write.
pub fn load_local_catalogs(
    catalog_base_dir: &Path,
    tenant_id: &str,
    registry: &mut Registry,
) -> anyhow::Result<()> {
    let tenant_dir = catalog_base_dir.join(tenant_id).join("catalog");
    if !tenant_dir.exists() {
        return Ok(());
    }

    cleanup_catalog_tmp_files(&tenant_dir);

    let mut loaded = 0usize;
    let date_entries = match std::fs::read_dir(&tenant_dir) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(path = %tenant_dir.display(), "failed to read catalog dir: {e}");
            return Ok(());
        }
    };

    for date_entry in date_entries.flatten() {
        if !date_entry
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
            continue;
        }
        let files = match std::fs::read_dir(date_entry.path()) {
            Ok(d) => d,
            Err(_) => continue,
        };
        for file_entry in files.flatten() {
            let path = file_entry.path();
            let is_catalog = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(".catalog"));
            if !is_catalog {
                continue;
            }

            let bytes = match std::fs::read(&path) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to read catalog: {e}");
                    continue;
                }
            };
            let catalog = match Catalog::from_json(&bytes) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to parse catalog: {e}");
                    continue;
                }
            };

            for entry in catalog.entries.values() {
                if registry.sfst.get(entry.id.seq).is_none() {
                    continue;
                }
                registry.catalog.insert_persisted(entry.clone());
                loaded += 1;
            }
        }
    }

    if loaded > 0 {
        tracing::info!(
            tenant = tenant_id,
            "loaded {loaded} catalog entries from local disk",
        );
    }
    Ok(())
}

/// List SFSTs in today's remote prefix and, for each one not already in
/// `registry.catalog`, read the local SFST header and insert a full
/// `CatalogEntry` as `Pending`.
///
/// This handles the crash-between-upload-and-catalog-write case: the
/// SFST is in the bucket but no catalog file records it. Subsequent
/// `recover_catalog` sends a `Record` to finish the job.
///
/// Returns `Err` if the remote is unreachable — the caller should skip
/// further remote-dependent recovery (uploads, catalog replay) and let
/// steady-state operation retry.
///
/// SFSTs discovered in remote storage whose local file is missing are
/// logged and skipped — the catalog entry cannot be reconstructed
/// without the file's header.
pub async fn reconcile_remote_uploads(
    registry: &mut Registry,
    operator: &opendal::Operator,
    tenant_id: &str,
) -> Result<(), opendal::Error> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let prefix = format!("{tenant_id}/sfst/{today}/");
    let entries = operator.list(&prefix).await?;

    let uploaded_at_ns = file_registry::TimestampNs(now_ns());
    let mut discovered = 0usize;

    for entry in entries {
        let path = entry.path();
        let filename = path.strip_prefix(&prefix).unwrap_or(path);
        let id = match file_registry::FileId::parse(Path::new(filename)) {
            Some(id) => id,
            None => continue,
        };

        if registry.catalog.contains(id.seq) {
            continue;
        }

        let sfst_entry = match registry.sfst.get(id.seq) {
            Some(e) => e,
            None => {
                tracing::warn!(
                    seq = id.seq,
                    remote_key = %path,
                    "remote SFST has no local file, skipping catalog reconstruction"
                );
                continue;
            }
        };

        let size = sfst_entry.size;
        let sfst_path = registry.sfst.file_path(id);
        match build_catalog_entry_from_sfst(&sfst_path, size, path, uploaded_at_ns, id) {
            Ok(entry) => {
                registry.catalog.insert_pending(entry);
                discovered += 1;
            }
            Err(e) => {
                tracing::warn!(
                    seq = id.seq,
                    path = %sfst_path.display(),
                    "failed to rebuild catalog entry from SFST: {e}",
                );
            }
        }
    }

    if discovered > 0 {
        tracing::info!(
            tenant = tenant_id,
            "reconciled {discovered} pending remote uploads",
        );
    }
    Ok(())
}

/// Replay `Record` requests for any `Pending` entries in `registry.catalog`.
///
/// Entries are marked `Persisted` as the writer acknowledges each.
pub async fn recover_catalog(
    registry: &mut Registry,
    catalog_writer: &mut ComponentHandle<CatalogWriterRequest, CatalogWriterResponse>,
    tenant_id: &str,
) -> anyhow::Result<()> {
    let requests: Vec<CatalogWriterRequest> = registry
        .catalog
        .iter_pending()
        .filter_map(|cre| {
            let date = parse_date_from_remote_key(&cre.entry.remote_key)?;
            Some(CatalogWriterRequest::Record {
                seq: cre.entry.id.seq,
                tenant_id: tenant_id.to_string(),
                date,
                entry: cre.entry.clone(),
            })
        })
        .collect();

    if requests.is_empty() {
        return Ok(());
    }

    tracing::info!(
        tenant = tenant_id,
        "catalog recovery: replaying {} pending entries",
        requests.len(),
    );

    batch_recover(requests, catalog_writer, |resp| match resp {
        CatalogWriterResponse::Recorded { seq } => {
            registry.catalog.mark_persisted(seq);
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
        if !date_entry
            .file_type()
            .map(|ft| ft.is_dir())
            .unwrap_or(false)
        {
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
    let data = std::fs::read(sfst_path).map_err(|e| anyhow::anyhow!("read sfst: {e}"))?;
    let reader = sfst::Reader::open(&data).map_err(|e| anyhow::anyhow!("open sfst: {e}"))?;
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

    fn make_entry(seq: u64) -> otel_catalog::CatalogEntry {
        otel_catalog::CatalogEntry {
            id: file_registry::FileId::new(
                uuid::Uuid::from_u128(1),
                uuid::Uuid::from_u128(2),
                seq,
                0,
            ),
            remote_key: format!("t/sfst/2026-04-17/{seq}.sfst"),
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 10,
            streams: vec![],
            size: ByteSize(1024),
            uploaded_at_ns: file_registry::TimestampNs(1_700_000_000_000_000_000),
        }
    }

    fn write_catalog_file(
        base: &Path,
        tenant_id: &str,
        date: NaiveDate,
        machine: uuid::Uuid,
        boot: uuid::Uuid,
        entries: &[otel_catalog::CatalogEntry],
    ) {
        let dir = base
            .join(tenant_id)
            .join("catalog")
            .join(date.format("%Y-%m-%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        let filename = format!("{}-{}.catalog", machine.as_simple(), boot.as_simple());
        let mut catalog = Catalog::new(
            tenant_id.to_string(),
            date,
            machine,
            boot,
            file_registry::TimestampNs(0),
        );
        for entry in entries {
            catalog.add(entry.clone(), file_registry::TimestampNs(0));
        }
        std::fs::write(dir.join(filename), catalog.to_json().unwrap()).unwrap();
    }

    #[test]
    fn load_local_catalogs_inserts_persisted_entries_for_tracked_sfsts() {
        let base = tempfile::tempdir().unwrap();
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();

        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        // Only seq=1 and seq=2 are tracked by the sfst registry. seq=3 is
        // "historical" — in the catalog file but no longer on disk locally.
        let machine = uuid::Uuid::from_u128(1);
        let boot = uuid::Uuid::from_u128(2);
        for seq in [1u64, 2] {
            let id = file_registry::FileId::new(machine, boot, seq, 0);
            reg.sfst
                .track(id, file_registry::TimestampNs(0), ByteSize(1));
        }

        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        write_catalog_file(
            base.path(),
            "tenant1",
            date,
            machine,
            boot,
            &[make_entry(1), make_entry(2), make_entry(3)],
        );

        load_local_catalogs(base.path(), "tenant1", &mut reg).unwrap();

        assert!(reg.catalog.is_persisted(1));
        assert!(reg.catalog.is_persisted(2));
        assert!(
            !reg.catalog.contains(3),
            "historical entry must be filtered out"
        );
    }

    #[test]
    fn load_local_catalogs_handles_missing_tenant_dir() {
        let base = tempfile::tempdir().unwrap();
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        load_local_catalogs(base.path(), "no-such-tenant", &mut reg).unwrap();
        assert!(reg.catalog.is_empty());
    }

    async fn run_recover_retention(
        registry: &mut Registry,
        retention: &bridge::config::RetentionConfig,
        storage_enabled: bool,
    ) {
        use crate::cleaner::Cleaner;
        use crate::component::ComponentHandle;
        use tokio_util::sync::CancellationToken;

        let cancel = CancellationToken::new();
        let mut cleaner = ComponentHandle::spawn::<Cleaner>((), cancel.child_token());
        recover_retention(registry, &mut cleaner, retention, storage_enabled)
            .await
            .unwrap();
        cancel.cancel();
    }

    fn evict_all_retention() -> bridge::config::RetentionConfig {
        bridge::config::RetentionConfig {
            max_files: 0,
            max_total_size: bytesize::ByteSize::b(u64::MAX),
            max_age: std::time::Duration::from_secs(u64::MAX / 2),
        }
    }

    #[tokio::test]
    async fn recover_retention_defers_pending_and_evicts_persisted() {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        let machine = uuid::Uuid::from_u128(1);
        let boot = uuid::Uuid::from_u128(2);
        for seq in [1u64, 2] {
            let id = file_registry::FileId::new(machine, boot, seq, 0);
            reg.sfst
                .track(id, file_registry::TimestampNs(0), ByteSize(1));
        }
        reg.catalog.insert_pending(make_entry(1));
        reg.catalog.insert_persisted(make_entry(2));

        run_recover_retention(&mut reg, &evict_all_retention(), true).await;

        assert!(reg.sfst.get(1).is_some(), "Pending seq must not be evicted");
        assert!(reg.catalog.contains(1));
        assert!(reg.sfst.get(2).is_none(), "Persisted seq must be evicted");
        assert!(!reg.catalog.contains(2));
    }

    #[tokio::test]
    async fn recover_retention_evicts_all_when_storage_disabled() {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        let machine = uuid::Uuid::from_u128(1);
        let boot = uuid::Uuid::from_u128(2);
        for seq in [1u64, 2] {
            let id = file_registry::FileId::new(machine, boot, seq, 0);
            reg.sfst
                .track(id, file_registry::TimestampNs(0), ByteSize(1));
        }

        run_recover_retention(&mut reg, &evict_all_retention(), false).await;

        assert!(reg.sfst.get(1).is_none());
        assert!(reg.sfst.get(2).is_none());
    }

    #[test]
    fn load_local_catalogs_skips_corrupt_files() {
        let base = tempfile::tempdir().unwrap();
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::registry::Registry::new(sfst_dir.path());
        let mut reg = Registry::new(wal, sfst);

        let machine = uuid::Uuid::from_u128(1);
        let boot = uuid::Uuid::from_u128(2);
        let id = file_registry::FileId::new(machine, boot, 1, 0);
        reg.sfst
            .track(id, file_registry::TimestampNs(0), ByteSize(1));

        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let dir = base
            .path()
            .join("tenant1")
            .join("catalog")
            .join(date.format("%Y-%m-%d").to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(format!(
                "{}-{}.catalog",
                machine.as_simple(),
                boot.as_simple()
            )),
            b"not valid json",
        )
        .unwrap();

        load_local_catalogs(base.path(), "tenant1", &mut reg).unwrap();
        assert!(
            reg.catalog.is_empty(),
            "corrupt file should be skipped, not loaded"
        );
    }
}
