//! Cleaner worker that deletes index files on retention eviction.
//!
//! Deletions are performed synchronously — `remove_file` is a single syscall.

use std::path::Path;

use tokio::sync::mpsc;

use crate::ipc::{CleanerRequest, CleanerResponse};
use crate::worker::Worker;

pub struct CleanerWorker;

impl Worker for CleanerWorker {
    type Request = CleanerRequest;
    type Response = CleanerResponse;
    type Args = ();

    fn new(_: ()) -> Self {
        Self
    }

    fn handle(&self, req: Self::Request, tx: mpsc::UnboundedSender<Self::Response>) {
        match req {
            CleanerRequest::DeleteIndexFile { sequence, path } => {
                let resp = match remove_file(&path) {
                    Ok(()) => CleanerResponse::IndexFileDeleted { sequence },
                    Err(error) => CleanerResponse::IndexFileFailed { sequence, error },
                };
                let _ = tx.send(resp);
            }
        }
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
