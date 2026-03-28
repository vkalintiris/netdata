//! Uploader worker that copies index files to remote object storage.
//!
//! The actual upload runs in a spawned async task and sends a result
//! back through the worker response channel when complete.

use opendal::Operator;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::ipc::{UploaderRequest, UploaderResponse};
use crate::worker::Worker;

pub struct UploaderWorker {
    operator: Operator,
}

impl Worker for UploaderWorker {
    type Request = UploaderRequest;
    type Response = UploaderResponse;
    type Args = Operator;

    fn new(operator: Operator) -> Self {
        Self { operator }
    }

    fn handle(&self, req: Self::Request, tx: mpsc::UnboundedSender<Self::Response>) {
        match req {
            UploaderRequest::Upload {
                seq,
                local_path,
                remote_key,
            } => {
                let operator = self.operator.clone();
                tokio::spawn(async move {
                    let start = Instant::now();
                    tracing::info!("upload started seq={seq} remote_key={remote_key}");

                    let resp = match tokio::fs::read(&local_path).await {
                        Ok(data) => match operator.write(&remote_key, data).await {
                            Ok(_) => {
                                tracing::info!(
                                    "upload complete seq={seq} remote_key={remote_key} elapsed_ms={}",
                                    start.elapsed().as_millis(),
                                );
                                UploaderResponse::Uploaded { seq, remote_key }
                            }
                            Err(e) => {
                                tracing::error!("upload failed seq={seq}: {e}");
                                UploaderResponse::UploadFailed {
                                    seq,
                                    error: e.to_string(),
                                }
                            }
                        },
                        Err(e) => {
                            tracing::error!(
                                "failed to read local file {}: {e}",
                                local_path.display()
                            );
                            UploaderResponse::UploadFailed {
                                seq,
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
