//! Catalog writer component: persists catalog entries locally (atomic
//! tmp + rename) and uploads them to remote object storage via opendal.
//!
//! Maintains an in-memory map of `Catalog` values keyed by
//! `(tenant_id, date, machine_id, boot_id)`. Each incoming `Record`
//! updates the catalog for its scope and writes the full (monotonically
//! growing) JSON representation to both local disk and remote storage.
//!
//! Processing is sequential: a single receiver drives a single task body,
//! so catalog mutations and writes for the same scope cannot interleave.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use opendal::Operator;
use otel_catalog::Catalog;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::component::Component;
use crate::ipc::{CatalogStage, CatalogWriterRequest, CatalogWriterResponse};

/// Arguments required to spawn a [`CatalogWriter`].
pub struct CatalogWriterArgs {
    /// Base directory under which per-tenant catalog subdirectories live.
    /// Typically `logs_config.index.dir`.
    pub catalog_base_dir: PathBuf,
    /// opendal operator rooted at the remote object-storage backend.
    pub operator: Operator,
}

pub struct CatalogWriter;

type CatalogKey = (String, NaiveDate, Uuid, Uuid);

impl Component for CatalogWriter {
    type Request = CatalogWriterRequest;
    type Response = CatalogWriterResponse;
    type Args = CatalogWriterArgs;

    async fn run(
        args: CatalogWriterArgs,
        mut rx: mpsc::UnboundedReceiver<CatalogWriterRequest>,
        tx: mpsc::UnboundedSender<CatalogWriterResponse>,
        cancel: CancellationToken,
    ) {
        let mut catalogs: HashMap<CatalogKey, Catalog> = HashMap::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                req = rx.recv() => match req {
                    Some(req) => {
                        let resp = handle_record(&mut catalogs, &args, req).await;
                        let _ = tx.send(resp);
                    }
                    None => break,
                }
            }
        }
    }
}

async fn handle_record(
    catalogs: &mut HashMap<CatalogKey, Catalog>,
    args: &CatalogWriterArgs,
    req: CatalogWriterRequest,
) -> CatalogWriterResponse {
    let CatalogWriterRequest::Record {
        seq,
        tenant_id,
        date,
        entry,
    } = req;

    let start = Instant::now();
    let machine_id = entry.id.machine_id;
    let boot_id = entry.id.boot_id;
    let now_ns = file_registry::TimestampNs(now_ns());

    let key = (tenant_id.clone(), date, machine_id, boot_id);
    let catalog = catalogs
        .entry(key)
        .or_insert_with(|| Catalog::new(tenant_id.clone(), date, machine_id, boot_id, now_ns));

    catalog.add(entry, now_ns);

    let bytes = match catalog.to_json() {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(seq, error = %e, "catalog serialization failed");
            return CatalogWriterResponse::RecordFailed {
                seq,
                stage: CatalogStage::Local,
                error: e.to_string(),
            };
        }
    };

    let local_final = local_path(&args.catalog_base_dir, &tenant_id, date, machine_id, boot_id);
    if let Err(e) = write_local_atomic(&local_final, &bytes).await {
        tracing::error!(seq, path = %local_final.display(), "catalog local write failed: {e}");
        return CatalogWriterResponse::RecordFailed {
            seq,
            stage: CatalogStage::Local,
            error: e.to_string(),
        };
    }

    let remote_key = remote_key(&tenant_id, date, machine_id, boot_id);
    if let Err(e) = args.operator.write(&remote_key, bytes).await {
        tracing::error!(seq, remote_key = %remote_key, "catalog remote write failed: {e}");
        return CatalogWriterResponse::RecordFailed {
            seq,
            stage: CatalogStage::Remote,
            error: e.to_string(),
        };
    }

    tracing::info!(
        seq,
        tenant = %tenant_id,
        date = %date,
        remote_key = %remote_key,
        entries_in_catalog = catalog.entries.len(),
        latency_ms = start.elapsed().as_millis() as u64,
        "catalog record ok",
    );

    CatalogWriterResponse::Recorded { seq }
}

fn catalog_filename(machine_id: Uuid, boot_id: Uuid) -> String {
    format!("{}-{}.catalog", machine_id.as_simple(), boot_id.as_simple())
}

fn local_path(
    base: &Path,
    tenant_id: &str,
    date: NaiveDate,
    machine_id: Uuid,
    boot_id: Uuid,
) -> PathBuf {
    base.join(tenant_id)
        .join("catalog")
        .join(date.format("%Y-%m-%d").to_string())
        .join(catalog_filename(machine_id, boot_id))
}

fn remote_key(tenant_id: &str, date: NaiveDate, machine_id: Uuid, boot_id: Uuid) -> String {
    format!(
        "{}/catalog/{}/{}",
        tenant_id,
        date.format("%Y-%m-%d"),
        catalog_filename(machine_id, boot_id),
    )
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

fn now_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX epoch")
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::ComponentHandle;
    use file_registry::{ByteSize, FileId, TimestampNs};
    use otel_catalog::{CatalogEntry, StreamEntry};
    use std::path::Path;

    fn build_operator(root: &Path) -> Operator {
        let uri = format!("fs://{}", root.display());
        Operator::from_uri(uri.as_str()).expect("build fs operator")
    }

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
            remote_key: format!(
                "tenant1/sfst/2026-04-17/{}",
                FileId::new(machine(), boot(), seq, 0).to_filename("sfst"),
            ),
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 100,
            streams: vec![StreamEntry::new("prod", "api")],
            size: ByteSize(1024),
            uploaded_at_ns: TimestampNs(2_000_000_000),
        }
    }

    fn record_for(seq: u64) -> CatalogWriterRequest {
        CatalogWriterRequest::Record {
            seq,
            tenant_id: "tenant1".to_string(),
            date: date(),
            entry: entry_for(seq),
        }
    }

    fn expected_local(base: &Path) -> PathBuf {
        base.join("tenant1")
            .join("catalog")
            .join("2026-04-17")
            .join(catalog_filename(machine(), boot()))
    }

    fn expected_remote() -> String {
        remote_key("tenant1", date(), machine(), boot())
    }

    struct Harness {
        handle: ComponentHandle<CatalogWriterRequest, CatalogWriterResponse>,
        cancel: CancellationToken,
        _local_dir: tempfile::TempDir,
        _remote_dir: tempfile::TempDir,
        local_root: PathBuf,
        remote_root: PathBuf,
    }

    impl Harness {
        fn new() -> Self {
            let local_dir = tempfile::tempdir().unwrap();
            let remote_dir = tempfile::tempdir().unwrap();
            let local_root = local_dir.path().to_path_buf();
            let remote_root = remote_dir.path().to_path_buf();
            let operator = build_operator(&remote_root);
            let args = CatalogWriterArgs {
                catalog_base_dir: local_root.clone(),
                operator,
            };
            let cancel = CancellationToken::new();
            let handle = ComponentHandle::spawn::<CatalogWriter>(args, cancel.child_token());
            Self {
                handle,
                cancel,
                _local_dir: local_dir,
                _remote_dir: remote_dir,
                local_root,
                remote_root,
            }
        }

        fn new_with_local_root(root: PathBuf) -> Self {
            let remote_dir = tempfile::tempdir().unwrap();
            let remote_root = remote_dir.path().to_path_buf();
            let operator = build_operator(&remote_root);
            let args = CatalogWriterArgs {
                catalog_base_dir: root.clone(),
                operator,
            };
            let cancel = CancellationToken::new();
            let handle = ComponentHandle::spawn::<CatalogWriter>(args, cancel.child_token());
            Self {
                handle,
                cancel,
                _local_dir: tempfile::tempdir().unwrap(),
                _remote_dir: remote_dir,
                local_root: root,
                remote_root,
            }
        }

        async fn send_and_recv(
            &mut self,
            req: CatalogWriterRequest,
        ) -> CatalogWriterResponse {
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
    async fn happy_path_writes_local_and_remote() {
        let mut h = Harness::new();
        let resp = h.send_and_recv(record_for(1)).await;
        assert!(
            matches!(resp, CatalogWriterResponse::Recorded { seq: 1 }),
            "expected Recorded, got {resp:?}"
        );

        let local = expected_local(&h.local_root);
        let bytes = std::fs::read(&local).expect("local catalog file exists");
        let catalog = Catalog::from_json(&bytes).unwrap();
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.tenant_id, "tenant1");
        assert_eq!(catalog.date, date());

        let remote_path = h.remote_root.join(expected_remote());
        let remote_bytes = std::fs::read(&remote_path).expect("remote catalog file exists");
        assert_eq!(remote_bytes, bytes);
    }

    #[tokio::test]
    async fn same_record_twice_yields_single_entry() {
        let mut h = Harness::new();
        let r1 = h.send_and_recv(record_for(1)).await;
        let r2 = h.send_and_recv(record_for(1)).await;
        assert!(matches!(r1, CatalogWriterResponse::Recorded { .. }));
        assert!(matches!(r2, CatalogWriterResponse::Recorded { .. }));

        let bytes = std::fs::read(expected_local(&h.local_root)).unwrap();
        let catalog = Catalog::from_json(&bytes).unwrap();
        assert_eq!(catalog.entries.len(), 1);
    }

    #[tokio::test]
    async fn two_distinct_entries_in_same_scope() {
        let mut h = Harness::new();
        let r1 = h.send_and_recv(record_for(1)).await;
        let r2 = h.send_and_recv(record_for(2)).await;
        assert!(matches!(r1, CatalogWriterResponse::Recorded { .. }));
        assert!(matches!(r2, CatalogWriterResponse::Recorded { .. }));

        let bytes = std::fs::read(expected_local(&h.local_root)).unwrap();
        let catalog = Catalog::from_json(&bytes).unwrap();
        assert_eq!(catalog.entries.len(), 2);
        let seqs: Vec<u64> = catalog.entries.values().map(|e| e.id.seq).collect();
        assert!(seqs.contains(&1));
        assert!(seqs.contains(&2));
    }

    #[tokio::test]
    async fn local_failure_when_base_dir_is_a_file() {
        // Point catalog_base_dir at a regular file. Attempting to create
        // subdirectories under it will fail with ENOTDIR.
        let local_dir = tempfile::tempdir().unwrap();
        let sentinel = local_dir.path().join("not_a_dir");
        std::fs::write(&sentinel, b"").unwrap();

        let mut h = Harness::new_with_local_root(sentinel.clone());
        let resp = h.send_and_recv(record_for(1)).await;

        match resp {
            CatalogWriterResponse::RecordFailed { seq: 1, stage, .. } => {
                assert_eq!(stage, CatalogStage::Local);
            }
            other => panic!("expected RecordFailed(Local), got {other:?}"),
        }

        // Remote should not have been touched.
        let remote_path = h.remote_root.join(expected_remote());
        assert!(!remote_path.exists());
    }
}
