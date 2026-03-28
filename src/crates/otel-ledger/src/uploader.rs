//! Uploader component that copies index files to remote object storage.
//!
//! Every request is acknowledged immediately. The actual upload runs in a
//! spawned async task and sends an `Uploaded` or `UploadFailed` notification
//! back to the ledger when complete.

use ferryboat::{Connection, Endpoint, Listener};
use opendal::Operator;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::ipc::{UPLOADER_ENDPOINT, UploaderRequest, UploaderResponse};

/// Spawns the uploader as a tokio task.
///
/// Returns a [`Connection`] the ledger can use to send requests and
/// receive responses. The task exits cleanly when `cancel` is cancelled.
pub async fn spawn(
    cancel: CancellationToken,
    operator: Operator,
) -> Result<Connection<UploaderRequest, UploaderResponse>, ferryboat::Error> {
    let listener = Listener::<UploaderResponse, UploaderRequest>::bind(Endpoint::in_process(
        UPLOADER_ENDPOINT,
    ))
    .open()?;

    tokio::spawn(uploader_task(listener, cancel, operator));

    let conn = Connection::<UploaderRequest, UploaderResponse>::connect(Endpoint::in_process(
        UPLOADER_ENDPOINT,
    ))
    .open()
    .await?;

    Ok(conn)
}

fn upload(
    operator: Operator,
    seq: u64,
    local_path: std::path::PathBuf,
    remote_key: String,
    done_tx: mpsc::UnboundedSender<UploaderResponse>,
) {
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
                tracing::error!("failed to read local file {}: {e}", local_path.display());
                UploaderResponse::UploadFailed {
                    seq,
                    error: e.to_string(),
                }
            }
        };

        let _ = done_tx.send(resp);
    });
}

async fn uploader_task(
    mut listener: Listener<UploaderResponse, UploaderRequest>,
    cancel: CancellationToken,
    operator: Operator,
) {
    let mut conn = match listener.accept().await {
        Ok(c) => {
            tracing::info!("uploader task connected to ledger event loop");
            c
        }
        Err(e) => {
            tracing::error!("uploader: failed to accept connection: {e}");
            return;
        }
    };

    let (done_tx, mut done_rx) = mpsc::unbounded_channel::<UploaderResponse>();

    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            r = conn.recv() => {
                let req = match r {
                    Ok(req) => req,
                    Err(e) => {
                        tracing::error!("uploader recv failed: {e}");
                        break;
                    }
                };

                if conn.send(UploaderResponse::Accepted).await.is_err() {
                    break;
                }

                match req {
                    UploaderRequest::Upload { seq, local_path, remote_key } => {
                        upload(operator.clone(), seq, local_path, remote_key, done_tx.clone());
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
