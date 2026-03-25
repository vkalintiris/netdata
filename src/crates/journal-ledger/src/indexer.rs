//! Indexer component that builds split-FST indexes from completed WAL files.
//!
//! Every request is acknowledged immediately. Index finalization runs in
//! a dedicated blocking task and sends an `IndexFinalized` notification
//! back to the ledger when complete.

use std::path::PathBuf;
use std::sync::Arc;

use ferryboat::{Connection, Endpoint, Listener};
use tokio::sync::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::ipc::{INDEXER_ENDPOINT, IndexerRequest, IndexerResponse};

#[derive(Clone)]
struct Sender(Arc<Mutex<Connection<IndexerResponse, IndexerRequest>>>);

impl Sender {
    async fn send(&self, resp: IndexerResponse) -> Result<(), ferryboat::Error> {
        let mut conn = self.0.lock().await;
        conn.send(resp).await
    }

    async fn recv(&self) -> Result<IndexerRequest, ferryboat::Error> {
        let mut conn = self.0.lock().await;
        conn.recv().await
    }
}

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

struct Indexer {
    sender: Sender,
}

impl Indexer {
    fn new(sender: Sender) -> Self {
        Self { sender }
    }

    fn finalize(&mut self, path: PathBuf) {
        tracing::info!("FinalizeIndex started path={}", path.display());

        let seq = extract_sequence(&path);
        let sender = self.sender.clone();
        tokio::task::spawn_blocking(move || {
            let start = Instant::now();

            let resp = match log_index::index_wal_file(&path) {
                Ok(index_path) => {
                    // Delete the now-redundant WAL file.
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            tracing::info!("WAL file deleted path={}", path.display());
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            tracing::warn!(
                                "failed to delete WAL file path={}: {e}",
                                path.display()
                            );
                        }
                    }

                    tracing::info!(
                        "FinalizeIndex complete wal={} index={} elapsed_ms={}",
                        path.display(),
                        index_path.display(),
                        start.elapsed().as_millis(),
                    );
                    IndexerResponse::IndexFinalized { seq, path }
                }
                Err(e) => {
                    tracing::error!("FinalizeIndex failed path={}: {e}", path.display(),);
                    IndexerResponse::IndexFailed {
                        path,
                        error: e.to_string(),
                    }
                }
            };

            tokio::runtime::Handle::current().block_on(async {
                if let Err(e) = sender.send(resp).await {
                    tracing::warn!("failed to send indexer response: {e}");
                }
            });
        });
    }
}

fn extract_sequence(path: &std::path::Path) -> u64 {
    wal::FileId::parse(path).map(|id| id.seq).unwrap_or(0)
}

async fn indexer_task(mut listener: Listener<IndexerResponse, IndexerRequest>, cancel: CancellationToken) {
    let conn = match listener.accept().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to accept connection: {e}");
            return;
        }
    };

    tracing::info!("indexer task connected to ledger event loop");

    let sender = Sender(Arc::new(Mutex::new(conn)));
    let mut indexer = Indexer::new(sender.clone());

    loop {
        let req = tokio::select! {
            _ = cancel.cancelled() => break,
            r = sender.recv() => match r {
                Ok(req) => req,
                Err(e) => {
                    tracing::error!("indexer recv failed: {e}");
                    break;
                }
            },
        };

        if sender.send(IndexerResponse::Accepted).await.is_err() {
            break;
        }

        match req {
            IndexerRequest::FinalizeIndex { path } => {
                indexer.finalize(path);
            }
        }
    }
}
