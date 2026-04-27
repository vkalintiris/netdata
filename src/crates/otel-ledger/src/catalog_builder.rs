//! Catalog builder component: accumulates `CatalogEntry` rows in memory
//! per `(tenant, date, machine, boot)` scope and rotates to an immutable
//! catalog file on disk once the accumulator reaches `rotation_count`.
//!
//! Catalog files are atomic (tmp + rename). Upload to remote is handled
//! by the ledger via the existing `Uploader` component.
//!
//! Processing is sequential: a single receiver drives a single task body,
//! so mutations and rotations for the same scope cannot interleave.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use file_registry::{ByteSize, TenantId, TimestampNs};
use otel_catalog::Catalog;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::component::Component;
use crate::ipc::{CatalogBuilderRequest, CatalogBuilderResponse};
use crate::recovery::now_ns;

pub struct CatalogBuilderArgs {
    /// Tenant-prefix root for catalog storage (typically `logs_config.index.dir`).
    /// Per-tenant subdirectories `{tenant}/catalog/{date}/` are created lazily.
    pub catalog_base_dir: PathBuf,
    /// Number of entries that triggers a rotation for a scope.
    pub rotation_count: usize,
}

pub struct CatalogBuilder;

type ScopeKey = (TenantId, NaiveDate, Uuid, Uuid);

impl Component for CatalogBuilder {
    type Request = CatalogBuilderRequest;
    type Response = CatalogBuilderResponse;
    type Args = CatalogBuilderArgs;

    async fn run(
        args: CatalogBuilderArgs,
        mut rx: mpsc::UnboundedReceiver<CatalogBuilderRequest>,
        tx: mpsc::UnboundedSender<CatalogBuilderResponse>,
        cancel: CancellationToken,
    ) {
        let mut accumulators: HashMap<ScopeKey, Catalog> = HashMap::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                req = rx.recv() => match req {
                    Some(req) => {
                        let resp = handle_request(&mut accumulators, &args, req).await;
                        let _ = tx.send(resp);
                    }
                    None => break,
                }
            }
        }
    }
}

async fn handle_request(
    accumulators: &mut HashMap<ScopeKey, Catalog>,
    args: &CatalogBuilderArgs,
    req: CatalogBuilderRequest,
) -> CatalogBuilderResponse {
    let CatalogBuilderRequest::AddEntry {
        tenant_id,
        date,
        entry,
    } = req;

    let seq = entry.id.seq;
    let machine_id = entry.id.machine_id;
    let boot_id = entry.id.boot_id;
    let now = TimestampNs(now_ns());

    let key: ScopeKey = (tenant_id.clone(), date, machine_id, boot_id);
    let catalog = accumulators
        .entry(key.clone())
        .or_insert_with(|| Catalog::new(tenant_id.clone(), date, machine_id, boot_id, now));
    catalog.add(entry, now);

    if catalog.entries.len() < args.rotation_count {
        return CatalogBuilderResponse::EntryAccepted { seq };
    }

    let max_seq = catalog
        .entries
        .values()
        .map(|e| e.id.seq)
        .max()
        .unwrap_or(seq);
    let seqs: Vec<u64> = catalog.entries.values().map(|e| e.id.seq).collect();

    let bytes = match catalog.to_json() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(
                tenant = %tenant_id,
                max_seq,
                "catalog serialization failed: {e}",
            );
            return CatalogBuilderResponse::RotationFailed {
                tenant_id,
                date,
                machine_id,
                boot_id,
                max_seq,
                error: e.to_string(),
            };
        }
    };
    let size = ByteSize(bytes.len() as u64);

    let path = scope_path(
        &args.catalog_base_dir,
        &tenant_id,
        date,
        machine_id,
        boot_id,
        max_seq,
    );
    if let Err(e) = write_local_atomic(&path, &bytes).await {
        tracing::error!(
            tenant = %tenant_id,
            path = %path.display(),
            "catalog local write failed: {e}",
        );
        return CatalogBuilderResponse::RotationFailed {
            tenant_id,
            date,
            machine_id,
            boot_id,
            max_seq,
            error: e.to_string(),
        };
    }

    accumulators.remove(&key);

    tracing::info!(
        tenant = %tenant_id,
        date = %date,
        max_seq,
        path = %path.display(),
        entries = seqs.len(),
        "catalog rotated",
    );

    CatalogBuilderResponse::Rotated {
        tenant_id,
        date,
        machine_id,
        boot_id,
        max_seq,
        path,
        size,
        created_at_ns: now,
        seqs,
    }
}

/// Full on-disk path for a catalog file.
///
/// Layout: `{base}/{YYYY-MM-DD}/{tenant_id}/{machine}-{boot}-{max_seq}.catalog`.
/// The base directory (`logs_config.catalog.dir`) is dedicated to catalog
/// files, so there's no `catalog/` subdir — same convention as WAL and SFST.
///
/// `tenant_id` is expected to be pre-validated by
/// `otel_ingestor::logs_service::validate_tenant_id`.
pub(crate) fn scope_path(
    base: &Path,
    tenant_id: &TenantId,
    date: NaiveDate,
    machine_id: Uuid,
    boot_id: Uuid,
    max_seq: u64,
) -> PathBuf {
    base.join(date.format("%Y-%m-%d").to_string())
        .join(tenant_id.as_str())
        .join(otel_catalog::filename(machine_id, boot_id, max_seq))
}

async fn write_local_atomic(final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp_path = final_path.with_extension("catalog.tmp");
    let mut file = tokio::fs::File::create(&tmp_path).await?;
    use tokio::io::AsyncWriteExt;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    tokio::fs::rename(&tmp_path, final_path).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::ComponentHandle;
    use file_registry::FileId;
    use otel_catalog::{CatalogEntry, StreamEntry};

    fn machine() -> Uuid {
        Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff)
    }

    fn boot() -> Uuid {
        Uuid::from_u128(0xaaaa_bbbb_cccc_dddd_eeee_ffff_0000_1111)
    }

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 4, 17).unwrap()
    }

    fn entry_for(seq: u64) -> CatalogEntry {
        CatalogEntry {
            id: FileId::new(machine(), boot(), seq, 0),
            remote_key: format!("tenant1/sfst/2026-04-17/{seq}.sfst"),
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 100,
            stream: StreamEntry::new("prod", "api"),
            size: ByteSize(1024),
            uploaded_at_ns: TimestampNs(2_000_000_000),
        }
    }

    fn add_request(seq: u64) -> CatalogBuilderRequest {
        CatalogBuilderRequest::AddEntry {
            tenant_id: TenantId::from("tenant1"),
            date: date(),
            entry: entry_for(seq),
        }
    }

    struct Harness {
        handle: ComponentHandle<CatalogBuilderRequest, CatalogBuilderResponse>,
        cancel: CancellationToken,
        _tmp: tempfile::TempDir,
        base: PathBuf,
    }

    impl Harness {
        fn new(rotation_count: usize) -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let base = tmp.path().to_path_buf();
            let args = CatalogBuilderArgs {
                catalog_base_dir: base.clone(),
                rotation_count,
            };
            let cancel = CancellationToken::new();
            let handle = ComponentHandle::spawn::<CatalogBuilder>(args, cancel.child_token());
            Self {
                handle,
                cancel,
                _tmp: tmp,
                base,
            }
        }

        async fn send_recv(&mut self, req: CatalogBuilderRequest) -> CatalogBuilderResponse {
            self.handle.send(req).unwrap();
            self.handle.recv().await.unwrap()
        }
    }

    impl Drop for Harness {
        fn drop(&mut self) {
            self.cancel.cancel();
        }
    }

    #[tokio::test]
    async fn add_entry_below_threshold_is_accepted() {
        let mut h = Harness::new(3);
        let resp = h.send_recv(add_request(1)).await;
        assert!(matches!(
            resp,
            CatalogBuilderResponse::EntryAccepted { seq: 1 }
        ));

        let expected_path = scope_path(
            &h.base,
            &TenantId::from("tenant1"),
            date(),
            machine(),
            boot(),
            1,
        );
        assert!(!expected_path.exists(), "must not rotate below threshold");
    }

    #[tokio::test]
    async fn rotation_fires_at_threshold_and_writes_file() {
        let mut h = Harness::new(3);
        assert!(matches!(
            h.send_recv(add_request(1)).await,
            CatalogBuilderResponse::EntryAccepted { .. }
        ));
        assert!(matches!(
            h.send_recv(add_request(2)).await,
            CatalogBuilderResponse::EntryAccepted { .. }
        ));

        let resp = h.send_recv(add_request(3)).await;
        match resp {
            CatalogBuilderResponse::Rotated {
                tenant_id,
                max_seq,
                path,
                seqs,
                ..
            } => {
                assert_eq!(tenant_id.as_str(), "tenant1");
                assert_eq!(max_seq, 3);
                let mut seen = seqs.clone();
                seen.sort();
                assert_eq!(seen, vec![1, 2, 3]);
                assert!(path.exists(), "rotated catalog file must exist on disk");
                let bytes = std::fs::read(&path).unwrap();
                let catalog = Catalog::from_json(&bytes).unwrap();
                assert_eq!(catalog.entries.len(), 3);
            }
            other => panic!("expected Rotated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn accumulator_is_drained_on_rotation() {
        let mut h = Harness::new(2);
        assert!(matches!(
            h.send_recv(add_request(1)).await,
            CatalogBuilderResponse::EntryAccepted { .. }
        ));
        // Hits threshold
        let r = h.send_recv(add_request(2)).await;
        let first_path = match r {
            CatalogBuilderResponse::Rotated { path, .. } => path,
            other => panic!("expected Rotated, got {other:?}"),
        };

        // Next entry starts a fresh accumulator; one more to hit threshold again.
        assert!(matches!(
            h.send_recv(add_request(3)).await,
            CatalogBuilderResponse::EntryAccepted { .. }
        ));
        let r = h.send_recv(add_request(4)).await;
        let second_path = match r {
            CatalogBuilderResponse::Rotated { path, max_seq, .. } => {
                assert_eq!(max_seq, 4);
                path
            }
            other => panic!("expected Rotated, got {other:?}"),
        };

        assert_ne!(first_path, second_path);
        let second = Catalog::from_json(&std::fs::read(&second_path).unwrap()).unwrap();
        let seqs: Vec<u64> = second.entries.values().map(|e| e.id.seq).collect();
        let mut sorted = seqs.clone();
        sorted.sort();
        assert_eq!(sorted, vec![3, 4]);
    }

    #[tokio::test]
    async fn rotation_below_one_is_still_honored() {
        // rotation_count = 1 rotates on every AddEntry.
        let mut h = Harness::new(1);
        let resp = h.send_recv(add_request(5)).await;
        match resp {
            CatalogBuilderResponse::Rotated { max_seq, seqs, .. } => {
                assert_eq!(max_seq, 5);
                assert_eq!(seqs, vec![5]);
            }
            other => panic!("expected Rotated, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rotation_failure_preserves_accumulator() {
        // Point catalog_base_dir at a regular file. mkdir_all under it will
        // fail with ENOTDIR, causing the rotation to fail.
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("not_a_dir");
        std::fs::write(&sentinel, b"").unwrap();

        let cancel = CancellationToken::new();
        let mut handle = ComponentHandle::spawn::<CatalogBuilder>(
            CatalogBuilderArgs {
                catalog_base_dir: sentinel,
                rotation_count: 1,
            },
            cancel.child_token(),
        );

        handle.send(add_request(1)).unwrap();
        let resp = handle.recv().await.unwrap();
        match resp {
            CatalogBuilderResponse::RotationFailed { max_seq, .. } => {
                assert_eq!(max_seq, 1);
            }
            other => panic!("expected RotationFailed, got {other:?}"),
        }

        // Next AddEntry should find the accumulator intact and try to rotate
        // again (still fails, but the entry was not lost).
        handle.send(add_request(2)).unwrap();
        let resp = handle.recv().await.unwrap();
        match resp {
            CatalogBuilderResponse::RotationFailed { max_seq, .. } => {
                // Accumulator now has seq 1 and 2; max_seq is 2.
                assert_eq!(max_seq, 2);
            }
            other => panic!("expected RotationFailed, got {other:?}"),
        }

        cancel.cancel();
    }

    #[tokio::test]
    async fn distinct_scopes_rotate_independently() {
        let mut h = Harness::new(2);
        // tenant1 seq=1
        assert!(matches!(
            h.send_recv(add_request(1)).await,
            CatalogBuilderResponse::EntryAccepted { .. }
        ));

        // A different scope (different machine_id) — shouldn't trigger
        // tenant1's rotation.
        let other_machine = Uuid::from_u128(0x1111);
        let other_entry = CatalogEntry {
            id: FileId::new(other_machine, boot(), 1, 0),
            ..entry_for(1)
        };
        assert!(matches!(
            h.send_recv(CatalogBuilderRequest::AddEntry {
                tenant_id: TenantId::from("tenant1"),
                date: date(),
                entry: other_entry,
            })
            .await,
            CatalogBuilderResponse::EntryAccepted { .. }
        ));

        // tenant1 seq=2 — now hits threshold for the original scope.
        let resp = h.send_recv(add_request(2)).await;
        match resp {
            CatalogBuilderResponse::Rotated {
                machine_id, seqs, ..
            } => {
                assert_eq!(machine_id, machine());
                let mut sorted = seqs.clone();
                sorted.sort();
                assert_eq!(sorted, vec![1, 2]);
            }
            other => panic!("expected Rotated, got {other:?}"),
        }
    }
}
