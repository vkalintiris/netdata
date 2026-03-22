//! Cleaner component that deletes index files on retention eviction.
//!
//! Runs as a tokio task. Deletions are performed synchronously in the
//! recv loop — `remove_file` is a single syscall.

use std::path::Path;

use ferryboat::{Connection, Endpoint, Listener};

use tokio_util::sync::CancellationToken;

use crate::ipc::{CLEANER_ENDPOINT, CleanerRequest, CleanerResponse};

/// Spawns the cleaner as a tokio task.
///
/// Returns a [`Connection`] the ledger can use to send requests and
/// receive responses. The task exits cleanly when `cancel` is cancelled.
pub async fn spawn(
    cancel: CancellationToken,
) -> Result<Connection<CleanerRequest, CleanerResponse>, ferryboat::Error> {
    let listener =
        Listener::<CleanerResponse, CleanerRequest>::bind(Endpoint::in_process(CLEANER_ENDPOINT))
            .open()?;

    tokio::spawn(cleaner_task(listener, cancel));

    let conn = Connection::<CleanerRequest, CleanerResponse>::connect(Endpoint::in_process(
        CLEANER_ENDPOINT,
    ))
    .open()
    .await?;

    Ok(conn)
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

async fn cleaner_task(mut listener: Listener<CleanerResponse, CleanerRequest>, cancel: CancellationToken) {
    let mut conn = match listener.accept().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to accept connection: {e}");
            return;
        }
    };

    tracing::info!("cleaner task connected to ledger event loop");

    loop {
        let req = tokio::select! {
            _ = cancel.cancelled() => break,
            r = conn.recv() => match r {
                Ok(req) => req,
                Err(e) => {
                    tracing::error!("cleaner recv failed: {e}");
                    break;
                }
            },
        };

        if conn.send(CleanerResponse::Accepted).await.is_err() {
            break;
        }

        let resp = match req {
            CleanerRequest::DeleteIndexFile { sequence, path } => match remove_file(&path) {
                Ok(()) => CleanerResponse::IndexFileDeleted { sequence },
                Err(error) => CleanerResponse::IndexFileFailed { sequence, error },
            },
        };

        if conn.send(resp).await.is_err() {
            break;
        }
    }
}
