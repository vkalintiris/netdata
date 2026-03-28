//! Indexer worker that builds split-FST indexes from completed WAL files.
//!
//! Index finalization runs in a dedicated blocking task and sends the
//! result back through the worker response channel when complete.

use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::ipc::{IndexerRequest, IndexerResponse};
use crate::worker::Worker;

pub struct IndexerWorker;

impl Worker for IndexerWorker {
    type Request = IndexerRequest;
    type Response = IndexerResponse;
    type Args = ();

    fn new(_: ()) -> Self {
        Self
    }

    fn handle(&self, req: Self::Request, tx: mpsc::UnboundedSender<Self::Response>) {
        match req {
            IndexerRequest::FinalizeIndex {
                wal_path,
                index_path,
            } => {
                tokio::task::spawn_blocking(move || {
                    let seq = wal::FileId::parse(&wal_path)
                        .map(|id| id.seq)
                        .unwrap_or(0);
                    let start = Instant::now();

                    tracing::info!(
                        "FinalizeIndex started wal={} index={}",
                        wal_path.display(),
                        index_path.display(),
                    );

                    let resp = match log_index::index_wal_file(&wal_path, &index_path) {
                        Ok(()) => {
                            tracing::info!(
                                "FinalizeIndex complete seq={seq} elapsed_ms={}",
                                start.elapsed().as_millis(),
                            );
                            IndexerResponse::IndexFinalized {
                                seq,
                                path: index_path,
                            }
                        }
                        Err(e) => {
                            tracing::error!(
                                "FinalizeIndex failed wal={}: {e}",
                                wal_path.display(),
                            );
                            IndexerResponse::IndexFailed {
                                path: wal_path,
                                error: e.to_string(),
                            }
                        }
                    };

                    let _ = tx.send(resp);
                });
            }
        }
    }
}
