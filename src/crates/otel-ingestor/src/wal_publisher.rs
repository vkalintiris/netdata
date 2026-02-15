use std::sync::atomic::{AtomicU64, Ordering};

use ferryboat::{Connection, Endpoint};
use tokio::sync::mpsc;
use wal::format::{WalEvent, WalMessage};

/// Publishes WAL events to the ledger over a direct ferryboat IPC socket.
///
/// Messages are fire-and-forget: silently dropped if the connection is lost.
pub struct WalPublisher {
    tx: mpsc::UnboundedSender<WalMessage>,
    seq: AtomicU64,
}

impl WalPublisher {
    /// Creates a new publisher that connects to the ledger on the given socket path.
    pub fn new(socket_path: &str) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(publisher_task(rx, socket_path.to_string()));
        Self {
            tx,
            seq: AtomicU64::new(1),
        }
    }

    /// Publishes all events from a [`WalEvent`] slice (as returned by
    /// [`WalWriter::take_events`]).
    pub fn publish_events(&self, events: &[WalEvent]) {
        for event in events {
            let msg = WalMessage {
                seq: self.next_seq(),
                event: event.clone(),
            };
            let _ = self.tx.send(msg);
        }
    }

    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed)
    }
}

async fn publisher_task(mut rx: mpsc::UnboundedReceiver<WalMessage>, socket_path: String) {
    let endpoint = Endpoint::ipc(&socket_path);

    let mut conn = match Connection::<WalMessage, ()>::connect(endpoint).open().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to connect to ledger at {socket_path}: {e}");
            return;
        }
    };

    while let Some(msg) = rx.recv().await {
        if conn.send(msg).await.is_err() {
            tracing::error!("ledger IPC connection lost");
            break;
        }
    }
}
