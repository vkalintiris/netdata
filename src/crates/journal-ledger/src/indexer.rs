//! Indexer component that builds split-FST indexes from completed WAL files.
//!
//! Every request is acknowledged immediately. Index finalization runs in
//! a dedicated blocking task and sends an `IndexFinalized` notification
//! back to the ledger when complete.

use std::path::PathBuf;

use ferryboat::{Connection, Endpoint, Listener};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::ipc::{INDEXER_ENDPOINT, IndexerRequest, IndexerResponse};

/// Spawns the indexer as a tokio task.
///
/// Returns a [`Connection`] the ledger can use to send requests and
/// receive responses. The task exits cleanly when `cancel` is cancelled.
pub async fn spawn(
    cancel: CancellationToken,
) -> Result<Connection<IndexerRequest, IndexerResponse>, ferryboat::Error> {
    let listener =
        Listener::<IndexerResponse, IndexerRequest>::bind(Endpoint::in_process(INDEXER_ENDPOINT))
            .open()?;

    tokio::spawn(indexer_task(listener, cancel));

    let conn = Connection::<IndexerRequest, IndexerResponse>::connect(Endpoint::in_process(
        INDEXER_ENDPOINT,
    ))
    .open()
    .await?;

    Ok(conn)
}

fn finalize(
    wal_path: PathBuf,
    index_path: PathBuf,
    done_tx: mpsc::UnboundedSender<IndexerResponse>,
) {
    tracing::info!(
        "FinalizeIndex started wal={} index={}",
        wal_path.display(),
        index_path.display()
    );

    let seq = wal::FileId::parse(&wal_path)
        .map(|id| id.seq)
        .unwrap_or(0);

    tokio::task::spawn_blocking(move || {
        let start = Instant::now();

        let resp = match log_index::index_wal_file(&wal_path, &index_path) {
            Ok(()) => {
                // Delete the now-redundant WAL file.
                match std::fs::remove_file(&wal_path) {
                    Ok(()) => {
                        tracing::info!("WAL file deleted path={}", wal_path.display());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        tracing::warn!(
                            "failed to delete WAL file path={}: {e}",
                            wal_path.display()
                        );
                    }
                }

                tracing::info!(
                    "FinalizeIndex complete wal={} index={} elapsed_ms={}",
                    wal_path.display(),
                    index_path.display(),
                    start.elapsed().as_millis(),
                );
                IndexerResponse::IndexFinalized {
                    seq,
                    path: index_path,
                }
            }
            Err(e) => {
                tracing::error!("FinalizeIndex failed wal={}: {e}", wal_path.display());
                IndexerResponse::IndexFailed {
                    path: wal_path,
                    error: e.to_string(),
                }
            }
        };

        let _ = done_tx.send(resp);
    });
}

async fn indexer_task(
    mut listener: Listener<IndexerResponse, IndexerRequest>,
    cancel: CancellationToken,
) {
    let mut conn = match listener.accept().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to accept connection: {e}");
            return;
        }
    };

    tracing::info!("indexer task connected to ledger event loop");

    // Blocking tasks send completed responses here; we forward them to the ledger.
    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<IndexerResponse>();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = conn.recv() => {
                let req = match r {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("indexer recv failed: {e}");
                        break;
                    }
                };

                if conn.send(IndexerResponse::Accepted).await.is_err() {
                    break;
                }

                match req {
                    IndexerRequest::FinalizeIndex { wal_path, index_path } => {
                        finalize(wal_path, index_path, done_tx.clone());
                    }
                }
            }
            Some(resp) = done_rx.recv() => {
                if conn.send(resp).await.is_err() {
                    break;
                }
            }
        }
    }
}
