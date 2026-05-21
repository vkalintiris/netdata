//! Cleaner component that deletes index files on retention eviction.
//!
//! Deletions are performed synchronously — `remove_file` is a single syscall.

use std::path::Path;

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::component::Component;
use crate::ipc::{CleanerRequest, CleanerResponse};

pub struct Cleaner;

impl Component for Cleaner {
    type Request = CleanerRequest;
    type Response = CleanerResponse;
    type Args = ();

    async fn run(
        _args: (),
        mut rx: mpsc::UnboundedReceiver<CleanerRequest>,
        tx: mpsc::UnboundedSender<CleanerResponse>,
        cancel: CancellationToken,
    ) {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                req = rx.recv() => match req {
                    Some(req) => {
                        let _ = tx.send(process(req));
                    }
                    None => break,
                },
            }
        }
    }
}

fn process(req: CleanerRequest) -> CleanerResponse {
    match req {
        CleanerRequest::DeleteWalFile { sequence, path } => match remove_file(&path) {
            Ok(()) => CleanerResponse::WalFileDeleted { sequence },
            Err(error) => CleanerResponse::WalFileFailed { sequence, error },
        },
        CleanerRequest::DeleteIndexFile { sequence, path } => match remove_file(&path) {
            Ok(()) => CleanerResponse::IndexFileDeleted { sequence },
            Err(error) => CleanerResponse::IndexFileFailed { sequence, error },
        },
        CleanerRequest::DeleteCatalogFile { path } => match remove_file(&path) {
            Ok(()) => {
                // Catalog layout is `{base}/{date}/{tenant}/{file}.catalog`.
                // After deleting the file, prune the now-possibly-empty
                // `{tenant}` and `{date}` dirs. SFST/WAL layouts are flat
                // per-tenant so they don't need this; only catalogs have
                // date-bucketed dirs that accumulate over retention.
                prune_empty_parents(&path, 2);
                CleanerResponse::CatalogFileDeleted { path }
            }
            Err(error) => CleanerResponse::CatalogFileFailed { path, error },
        },
    }
}

fn remove_file(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            tracing::info!("deleted path={}", path.display());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("failed to delete {}: {e}", path.display())),
    }
}

/// Best-effort: walk up to `max_levels` ancestor dirs from `path` and
/// `remove_dir` each one. `std::fs::remove_dir` only succeeds on empty
/// dirs, so a non-empty parent silently aborts the walk.
///
/// Catalog rotations never target past dates, so dirs we're about to
/// prune have no writer racing against us.
fn prune_empty_parents(path: &Path, max_levels: usize) {
    let mut cursor = path.parent();
    for _ in 0..max_levels {
        let Some(dir) = cursor else { return };
        match std::fs::remove_dir(dir) {
            Ok(()) => tracing::debug!(dir = %dir.display(), "pruned empty catalog dir"),
            Err(_) => return, // Non-empty or other failure: stop.
        }
        cursor = dir.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn write_catalog(base: &Path, date: &str, tenant: &str, name: &str) -> PathBuf {
        let dir = base.join(date).join(tenant);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        path
    }

    #[test]
    fn delete_catalog_leaves_dir_when_siblings_remain() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let p1 = write_catalog(base, "2026-04-17", "tenant1", "a.catalog");
        let _p2 = write_catalog(base, "2026-04-17", "tenant1", "b.catalog");

        let resp = process(CleanerRequest::DeleteCatalogFile { path: p1.clone() });
        assert!(matches!(resp, CleanerResponse::CatalogFileDeleted { .. }));
        assert!(!p1.exists());

        // Tenant dir still has b.catalog; date dir still has tenant1.
        assert!(base.join("2026-04-17").join("tenant1").is_dir());
        assert!(base.join("2026-04-17").is_dir());
    }

    #[test]
    fn delete_last_catalog_prunes_tenant_and_date_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let p = write_catalog(base, "2026-04-17", "tenant1", "a.catalog");

        let resp = process(CleanerRequest::DeleteCatalogFile { path: p.clone() });
        assert!(matches!(resp, CleanerResponse::CatalogFileDeleted { .. }));
        assert!(!p.exists());

        // Both tenant and date dirs were empty post-delete → pruned.
        assert!(!base.join("2026-04-17").join("tenant1").exists());
        assert!(!base.join("2026-04-17").exists());
        // Base dir itself stays — pruning stops at max_levels=2.
        assert!(base.is_dir());
    }

    #[test]
    fn delete_last_catalog_keeps_date_dir_if_other_tenant_present() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let p1 = write_catalog(base, "2026-04-17", "tenant1", "a.catalog");
        let _p2 = write_catalog(base, "2026-04-17", "tenant2", "a.catalog");

        let resp = process(CleanerRequest::DeleteCatalogFile { path: p1.clone() });
        assert!(matches!(resp, CleanerResponse::CatalogFileDeleted { .. }));

        // tenant1/ pruned (empty); date dir kept (tenant2/ still there).
        assert!(!base.join("2026-04-17").join("tenant1").exists());
        assert!(base.join("2026-04-17").join("tenant2").is_dir());
        assert!(base.join("2026-04-17").is_dir());
    }

    #[test]
    fn delete_missing_catalog_is_noop_and_does_not_prune() {
        // If the file is already gone, remove_file returns Ok (NotFound is
        // treated as success). The prune walk then runs against the
        // surviving sibling-bearing dir and finds it non-empty → no-op.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        let _sibling = write_catalog(base, "2026-04-17", "tenant1", "a.catalog");
        let missing = base
            .join("2026-04-17")
            .join("tenant1")
            .join("missing.catalog");

        let resp = process(CleanerRequest::DeleteCatalogFile {
            path: missing.clone(),
        });
        assert!(matches!(resp, CleanerResponse::CatalogFileDeleted { .. }));
        assert!(base.join("2026-04-17").join("tenant1").is_dir());
    }
}
