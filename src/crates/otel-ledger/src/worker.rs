//! Generic worker abstraction for in-process task coordination.
//!
//! Replaces ferryboat in-process IPC with plain tokio mpsc channels.
//! Workers implement the [`Worker`] trait; the ledger communicates with
//! them through a [`WorkerHandle`].

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A stateless worker that processes requests and produces responses.
///
/// Workers run in their own tokio task. The `handle` method receives a
/// response sender so it can spawn async or blocking work that sends
/// results back when complete.
pub trait Worker: Send + 'static {
    type Request: Send + 'static;
    type Response: Send + 'static;
    type Args: Send + 'static;

    fn new(args: Self::Args) -> Self;
    fn handle(&self, req: Self::Request, tx: mpsc::UnboundedSender<Self::Response>);
}

/// Handle for communicating with a spawned [`Worker`].
pub struct WorkerHandle<Req, Resp> {
    tx: mpsc::UnboundedSender<Req>,
    rx: mpsc::UnboundedReceiver<Resp>,
}

impl<Req: Send + 'static, Resp: Send + 'static> WorkerHandle<Req, Resp> {
    /// Spawn a worker in a new tokio task and return a handle to it.
    pub fn spawn<W>(args: W::Args, cancel: CancellationToken) -> Self
    where
        W: Worker<Request = Req, Response = Resp>,
    {
        let (req_tx, mut req_rx) = mpsc::unbounded_channel::<Req>();
        let (resp_tx, resp_rx) = mpsc::unbounded_channel::<Resp>();

        tokio::spawn(async move {
            let worker = W::new(args);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    req = req_rx.recv() => match req {
                        Some(req) => worker.handle(req, resp_tx.clone()),
                        None => break,
                    },
                }
            }
        });

        Self { tx: req_tx, rx: resp_rx }
    }

    /// Send a request to the worker. Never blocks (unbounded channel).
    pub fn send(&self, req: Req) -> Result<(), mpsc::error::SendError<Req>> {
        self.tx.send(req)
    }

    /// Receive the next response from the worker.
    pub async fn recv(&mut self) -> Option<Resp> {
        self.rx.recv().await
    }
}

/// Send a batch of requests to a worker and process all responses.
///
/// Used during recovery to replay pending work through the normal
/// worker path instead of duplicating the processing logic.
pub async fn batch_recover<Req: Send + 'static, Resp: Send + 'static>(
    requests: Vec<Req>,
    handle: &mut WorkerHandle<Req, Resp>,
    mut process: impl FnMut(Resp),
) -> anyhow::Result<()> {
    if requests.is_empty() {
        return Ok(());
    }

    let count = requests.len();
    for req in requests {
        handle
            .send(req)
            .map_err(|_| anyhow::anyhow!("worker died during recovery"))?;
    }

    for _ in 0..count {
        let resp = handle
            .recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("worker died during recovery"))?;
        process(resp);
    }

    Ok(())
}
