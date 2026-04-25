//! Startup recovery: replays pending work that was interrupted by a previous
//! shutdown or crash. Each function sends requests through the normal component
//! path via [`batch_recover`], so recovery and steady-state use the same code.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::NaiveDate;
use file_registry::{ByteSize, TenantId};
use otel_catalog::Catalog;

use crate::component::{ComponentHandle, batch_recover, drain_pending};
use crate::ipc::{
    CatalogBuilderRequest, CatalogBuilderResponse, CleanerRequest, CleanerResponse, IndexerRequest,
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
        .map(|&id| IndexerRequest::Index {
            wal_path: registry.wal.file_path(id),
            sfst_path: registry.sfst.file_path(id),
        })
        .collect();

    batch_recover(requests, indexer, |resp| match resp {
        IndexerResponse::Indexed { seq, .. } => {
            let wf = match registry.wal.get(seq) {
                Some(wf) => wf,
                None => {
                    tracing::warn!(
                        "recovery: indexed unknown WAL seq={seq}, skipping cleanup"
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
            tracing::info!("recovery: indexed seq={seq}");
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

/// Evict SFST and catalog files that exceed their retention policies.
///
/// SFST retention uses the three-knob policy (`max_files` /
/// `max_total_size` / `max_age`). Catalog retention is derived from the
/// tenant's SFST `max_age` — see
/// [`crate::ledger::catalog_retention_days`].
pub async fn recover_retention(
    registry: &mut Registry,
    cleaner: &mut ComponentHandle<CleanerRequest, CleanerResponse>,
    retention: &bridge::config::RetentionConfig,
    storage_enabled: bool,
) -> anyhow::Result<()> {
    // SFST pass.
    let to_evict_sfst = registry.sfst.evaluate_retention(retention, now_ns());
    // Defer eviction when remote storage is enabled and the SFST's entry
    // isn't yet in a closed, on-disk catalog file (see the identical guard
    // in `evaluate_retention`).
    let (evictable_sfst, deferred_sfst): (Vec<u64>, Vec<u64>) = to_evict_sfst
        .into_iter()
        .partition(|&seq| !storage_enabled || registry.is_rotated(seq));
    for seq in deferred_sfst {
        tracing::warn!("recovery: deferring eviction of seq={seq} (upload or catalog pending)");
    }

    // Catalog pass. Day-count derived from SFST max_age.
    let catalog_days = crate::ledger::catalog_retention_days(retention);
    let today = chrono::Utc::now().date_naive();
    let evictable_catalog = registry
        .catalog_files
        .evaluate_retention(catalog_days, today);

    if evictable_sfst.is_empty() && evictable_catalog.is_empty() {
        return Ok(());
    }

    tracing::info!(
        "retention: evicting {} index file(s) and {} catalog file(s)",
        evictable_sfst.len(),
        evictable_catalog.len(),
    );

    // Note: unlike the steady-state `Ledger::evaluate_retention` path,
    // we don't `mark_pending_deletion` here. `batch_recover` sends all
    // requests and synchronously drains all responses before returning,
    // and the ledger event loop hasn't started yet — so there's no
    // concurrent retention pass that could double-schedule.
    let mut requests: Vec<CleanerRequest> =
        Vec::with_capacity(evictable_sfst.len() + evictable_catalog.len());
    for &seq in &evictable_sfst {
        if let Some(entry) = registry.sfst.get(seq) {
            let path = registry.sfst.file_path(entry.id);
            requests.push(CleanerRequest::DeleteIndexFile {
                sequence: seq,
                path,
            });
        }
    }
    for path in evictable_catalog {
        requests.push(CleanerRequest::DeleteCatalogFile { path });
    }

    batch_recover(requests, cleaner, |resp| match resp {
        CleanerResponse::IndexFileDeleted { sequence } => {
            registry.evict_seq(sequence);
            tracing::info!("recovery: index file evicted seq={sequence}");
        }
        CleanerResponse::IndexFileFailed { sequence, error } => {
            tracing::error!("recovery: index eviction failed seq={sequence} error={error}");
        }
        CleanerResponse::CatalogFileDeleted { path } => {
            registry.catalog_files.remove(&path);
            tracing::info!(path = %path.display(), "recovery: catalog file evicted");
        }
        CleanerResponse::CatalogFileFailed { path, error } => {
            tracing::error!(
                path = %path.display(),
                "recovery: catalog eviction failed: {error}",
            );
        }
        resp => {
            tracing::warn!("unexpected cleaner response during retention recovery: {resp:?}");
        }
    })
    .await
}

/// Upload index files that haven't been uploaded to remote storage yet.
///
/// On each `Uploaded` response, rebuilds a full `CatalogEntry` from the
/// SFST header and forwards it to the catalog builder as an `AddEntry`.
pub async fn recover_unuploaded(
    registry: &mut Registry,
    uploader: &mut ComponentHandle<UploaderRequest, UploaderResponse>,
    catalog_builder: &mut ComponentHandle<CatalogBuilderRequest, CatalogBuilderResponse>,
    tenant_id: &TenantId,
) -> anyhow::Result<()> {
    let unuploaded = registry.unuploaded_ids();
    if unuploaded.is_empty() {
        return Ok(());
    }

    tracing::info!(
        tenant = %tenant_id,
        "uploading {} un-uploaded index files",
        unuploaded.len()
    );

    let requests: Vec<_> = unuploaded
        .iter()
        .map(|&id| {
            let local_path = registry.sfst.file_path(id);
            let date =
                read_min_date(&local_path).unwrap_or_else(|| chrono::Utc::now().date_naive());
            let remote_key = crate::remote_keys::sfst(tenant_id, date, id);
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
            let entry = match build_catalog_entry_from_sfst(
                &sfst_path,
                size,
                &remote_key,
                uploaded_at_ns,
                id,
            ) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        seq,
                        path = %sfst_path.display(),
                        "recovery: failed to rebuild catalog entry: {e}",
                    );
                    return;
                }
            };
            registry.mark_uploaded(seq);
            let date = match crate::remote_keys::parse_sfst_date(&remote_key) {
                Some(d) => d,
                None => {
                    tracing::warn!(
                        seq,
                        remote_key = %remote_key,
                        "recovery: could not parse date from remote_key",
                    );
                    return;
                }
            };
            let req = CatalogBuilderRequest::AddEntry {
                tenant_id: tenant_id.clone(),
                date,
                entry,
            };
            if let Err(e) = catalog_builder.send(req) {
                tracing::error!(seq, "recovery: failed to send AddEntry: {e}");
            }
            tracing::info!("recovery: upload complete seq={seq}");
        }
        UploaderResponse::UploadFailed { seq, error } => {
            tracing::error!("recovery: upload failed seq={seq}: {error}");
        }
        resp => {
            tracing::warn!("unexpected uploader response during unuploaded recovery: {resp:?}");
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
fn read_min_date(index_path: &std::path::Path) -> Option<NaiveDate> {
    let data = std::fs::read(index_path).ok()?;
    let reader = sfst::Reader::open(&data).ok()?;
    let meta = reader
        .metadata::<log_index::fst_builder::IndexMetadata>()
        .ok()?;
    let min_sec = *meta.histogram.timestamps.first()? as i64;
    chrono::DateTime::from_timestamp(min_sec, 0).map(|dt| dt.date_naive())
}

/// Replay the catalog files already present on local disk (discovered by
/// `catalog_files.recover()`) into the registry's in-memory uploaded /
/// rotated state.
///
/// Each catalog file is parsed; every entry's seq is marked as both
/// uploaded and rotated. Rotated state satisfies the retention guard so
/// those SFSTs can be evicted; uploaded state prevents re-upload of
/// already-known-uploaded SFSTs.
pub fn seed_from_catalog_files(registry: &mut Registry) {
    let paths: Vec<std::path::PathBuf> = registry
        .catalog_files
        .iter()
        .map(|(p, _)| p.clone())
        .collect();
    if paths.is_empty() {
        return;
    }

    let mut seeded = 0usize;
    for path in paths {
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
            registry.mark_uploaded(entry.id.seq);
            registry.mark_rotated(entry.id.seq);
            seeded += 1;
        }
    }

    if seeded > 0 {
        tracing::info!(
            "seeded {seeded} entries from {} local catalog file(s)",
            registry.catalog_files.len(),
        );
    }
}

/// List SFSTs in today's remote prefix. For each one, mark it uploaded;
/// for those not yet rotated into a closed catalog, read the local SFST
/// header and forward a fresh `AddEntry` to the catalog builder so the
/// entry ends up in a future closed catalog file.
///
/// Returns `Err` if the remote is unreachable — the caller should skip
/// further remote-dependent recovery.
///
/// SFSTs discovered in remote storage whose local file is missing are
/// logged and skipped — the catalog entry cannot be reconstructed
/// without the file's header.
pub async fn reconcile_remote_uploads(
    registry: &mut Registry,
    catalog_builder: &mut ComponentHandle<CatalogBuilderRequest, CatalogBuilderResponse>,
    operator: &opendal::Operator,
    tenant_id: &TenantId,
) -> Result<(), opendal::Error> {
    let today = chrono::Utc::now().date_naive();
    let prefix = crate::remote_keys::sfst_prefix(tenant_id, today);
    let entries = operator.list(&prefix).await?;

    let uploaded_at_ns = file_registry::TimestampNs(now_ns());
    let mut reconciled = 0usize;

    for entry in entries {
        let path = entry.path();
        let filename = path.strip_prefix(&prefix).unwrap_or(path);
        let id = match file_registry::FileId::parse(Path::new(filename)) {
            Some(id) => id,
            None => continue,
        };

        registry.mark_uploaded(id.seq);

        if registry.is_rotated(id.seq) {
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
        let catalog_entry =
            match build_catalog_entry_from_sfst(&sfst_path, size, path, uploaded_at_ns, id) {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(
                        seq = id.seq,
                        path = %sfst_path.display(),
                        "failed to rebuild catalog entry from SFST: {e}",
                    );
                    continue;
                }
            };
        let date = match crate::remote_keys::parse_sfst_date(path) {
            Some(d) => d,
            None => {
                tracing::warn!(
                    seq = id.seq,
                    remote_key = %path,
                    "could not parse date from remote_key, skipping",
                );
                continue;
            }
        };
        let req = CatalogBuilderRequest::AddEntry {
            tenant_id: tenant_id.clone(),
            date,
            entry: catalog_entry,
        };
        if let Err(e) = catalog_builder.send(req) {
            tracing::error!(seq = id.seq, "failed to enqueue AddEntry: {e}");
            continue;
        }
        reconciled += 1;
    }

    if reconciled > 0 {
        tracing::info!(
            tenant = %tenant_id,
            "reconciled {reconciled} uncataloged remote uploads",
        );
    }
    Ok(())
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
    use otel_catalog::StreamEntry;

    fn machine() -> uuid::Uuid {
        uuid::Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }

    fn boot() -> uuid::Uuid {
        uuid::Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111)
    }

    fn make_entry(seq: u64) -> otel_catalog::CatalogEntry {
        let id = file_registry::FileId::new(machine(), boot(), seq, 0);
        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        otel_catalog::CatalogEntry {
            id,
            remote_key: crate::remote_keys::sfst(
                &TenantId::from("tenant1"),
                date,
                id,
            ),
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 10,
            streams: vec![StreamEntry::new("prod", "api")],
            size: ByteSize(1024),
            uploaded_at_ns: file_registry::TimestampNs(2_000_000_000),
        }
    }

    fn make_registry(catalog_dir: &Path) -> Registry {
        let wal_dir = tempfile::tempdir().unwrap();
        let sfst_dir = tempfile::tempdir().unwrap();
        let wal = wal::Registry::new(wal_dir.path());
        let sfst = sfst::Registry::new(sfst_dir.path());
        let catalog_files =
            otel_catalog::Registry::new(catalog_dir, TenantId::from("tenant1"));
        // Keep tempdirs alive for the test's lifetime.
        std::mem::forget((wal_dir, sfst_dir));
        Registry::new(wal, sfst, catalog_files)
    }

    fn write_catalog_file(
        catalog_dir: &Path,
        date: NaiveDate,
        entries: &[otel_catalog::CatalogEntry],
    ) -> std::path::PathBuf {
        let dir = catalog_dir
            .join(date.format("%Y-%m-%d").to_string())
            .join("tenant1");
        std::fs::create_dir_all(&dir).unwrap();
        let max_seq = entries.iter().map(|e| e.id.seq).max().unwrap();
        let path = dir.join(otel_catalog::filename(machine(), boot(), max_seq));
        let mut catalog = Catalog::new(
            TenantId::from("tenant1"),
            date,
            machine(),
            boot(),
            file_registry::TimestampNs(0),
        );
        for entry in entries {
            catalog.add(entry.clone(), file_registry::TimestampNs(0));
        }
        std::fs::write(&path, catalog.to_json().unwrap()).unwrap();
        path
    }

    #[test]
    fn seed_from_catalog_files_populates_both_sets() {
        let catalog_dir = tempfile::tempdir().unwrap();
        let mut reg = make_registry(catalog_dir.path());

        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        write_catalog_file(
            catalog_dir.path(),
            date,
            &[make_entry(1), make_entry(2), make_entry(3)],
        );
        reg.catalog_files.recover();

        seed_from_catalog_files(&mut reg);

        for seq in [1u64, 2, 3] {
            assert!(reg.is_uploaded(seq));
            assert!(reg.is_rotated(seq));
        }
    }

    #[test]
    fn seed_from_catalog_files_skips_corrupt_files() {
        let catalog_dir = tempfile::tempdir().unwrap();
        let mut reg = make_registry(catalog_dir.path());

        let date = NaiveDate::from_ymd_opt(2026, 4, 17).unwrap();
        let dir = catalog_dir
            .path()
            .join(date.format("%Y-%m-%d").to_string())
            .join("tenant1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(otel_catalog::filename(machine(), boot(), 1)),
            b"not valid json",
        )
        .unwrap();
        reg.catalog_files.recover();

        seed_from_catalog_files(&mut reg);
        assert!(!reg.is_uploaded(1));
        assert!(!reg.is_rotated(1));
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
    async fn recover_retention_defers_unrotated_and_evicts_rotated() {
        let catalog_dir = tempfile::tempdir().unwrap();
        let mut reg = make_registry(catalog_dir.path());

        for seq in [1u64, 2] {
            let id = file_registry::FileId::new(machine(), boot(), seq, 0);
            reg.sfst
                .track(id, file_registry::TimestampNs(0), ByteSize(1));
        }
        // Only seq=2 is in a closed catalog; seq=1 is not.
        reg.mark_rotated(2);

        run_recover_retention(&mut reg, &evict_all_retention(), true).await;

        assert!(
            reg.sfst.get(1).is_some(),
            "unrotated seq must not be evicted"
        );
        assert!(!reg.is_rotated(1));
        assert!(reg.sfst.get(2).is_none(), "rotated seq must be evicted");
        assert!(!reg.is_rotated(2));
    }

    #[tokio::test]
    async fn recover_retention_evicts_all_when_storage_disabled() {
        let catalog_dir = tempfile::tempdir().unwrap();
        let mut reg = make_registry(catalog_dir.path());

        for seq in [1u64, 2] {
            let id = file_registry::FileId::new(machine(), boot(), seq, 0);
            reg.sfst
                .track(id, file_registry::TimestampNs(0), ByteSize(1));
        }

        run_recover_retention(&mut reg, &evict_all_retention(), false).await;

        assert!(reg.sfst.get(1).is_none());
        assert!(reg.sfst.get(2).is_none());
    }
}
