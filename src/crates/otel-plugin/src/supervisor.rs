use std::collections::HashMap;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, bail};
use bridge::config::PluginConfig;
use bridge::{IngestorRequest, IngestorResponse, LedgerRequest, LedgerResponse};
use ferryboat::{Connection, Endpoint, Listener};
use netdata_plugin_protocol::{Message, MessageReader, MessageWriter};
use netdata_plugin_types::FunctionProgressResponse;
use tokio::process::Command;

/// Maximum time to wait for a worker to connect after spawning.
const WORKER_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

use crate::config;

/// Guard that kills a child process on drop.
struct ChildGuard {
    child: tokio::process::Child,
    name: &'static str,
}

impl ChildGuard {
    fn new(child: tokio::process::Child, name: &'static str) -> Self {
        Self { child, name }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let pid = self.child.id();
        if let Err(e) = self.child.start_kill() {
            tracing::warn!("failed to kill worker name={} pid={pid:?}: {e}", self.name);
        } else {
            tracing::info!("killed worker name={} pid={pid:?}", self.name);
        }
    }
}

/// Identifies which worker owns a function.
#[derive(Debug, Clone, Copy)]
enum Worker {
    Ingestor,
    Ledger,
}

struct Supervisor {
    ingestor: Connection<IngestorRequest, IngestorResponse>,
    /// Kills the ingestor process on drop — must outlive `ingestor`.
    #[allow(dead_code)]
    ingestor_child: ChildGuard,
    ledger: Connection<LedgerRequest, LedgerResponse>,
    /// Kills the ledger process on drop — must outlive `ledger`.
    #[allow(dead_code)]
    ledger_child: ChildGuard,
    /// Maps function name → owning worker.
    routing: HashMap<String, Worker>,
    reader: MessageReader<tokio::io::Stdin>,
    writer: MessageWriter<tokio::io::Stdout>,
    /// Removes socket files on drop.
    #[allow(dead_code)]
    sockets: [SocketGuard; 3],
}

impl Supervisor {
    /// Send Configure to the ingestor and wait for Ready.
    async fn configure_ingestor(&mut self, config: PluginConfig) -> anyhow::Result<()> {
        self.ingestor
            .send(IngestorRequest::Configure(config))
            .await
            .context("failed to send Configure to ingestor")?;

        match self.ingestor.recv().await.context("ingestor handshake")? {
            IngestorResponse::Ready { declarations } => {
                tracing::info!(
                    "ingestor reported ready with {} function declarations",
                    declarations.len()
                );
                for decl in declarations {
                    tracing::info!("registered ingestor function: {}", decl.name);
                    self.routing.insert(decl.name.clone(), Worker::Ingestor);
                    self.writer
                        .send(Message::FunctionDeclaration(Box::new(decl)))
                        .await
                        .context("failed to declare ingestor function to agent")?;
                }
            }
            other => {
                bail!("expected Ready from ingestor, got: {other:?}");
            }
        }
        Ok(())
    }

    /// Send Configure to the ledger and wait for Ready.
    async fn configure_ledger(&mut self, config: PluginConfig) -> anyhow::Result<()> {
        self.ledger
            .send(LedgerRequest::Configure(config))
            .await
            .context("failed to send Configure to ledger")?;

        match self.ledger.recv().await.context("ledger handshake")? {
            LedgerResponse::Ready { declarations } => {
                tracing::info!(
                    "ledger reported ready with {} function declarations",
                    declarations.len()
                );
                for decl in declarations {
                    tracing::info!("registered ledger function: {}", decl.name);
                    self.routing.insert(decl.name.clone(), Worker::Ledger);
                    self.writer
                        .send(Message::FunctionDeclaration(Box::new(decl)))
                        .await
                        .context("failed to declare ledger function to agent")?;
                }
            }
            other => {
                bail!("expected Ready from ledger, got: {other:?}");
            }
        }
        Ok(())
    }

    /// Route a function call from the agent to the appropriate worker.
    async fn handle_function_call(&mut self, call: netdata_plugin_types::FunctionCall) {
        let Some(&worker) = self.routing.get(&call.name) else {
            tracing::warn!("no handler for function: {}", call.name);
            return;
        };

        match worker {
            Worker::Ingestor => {
                let req = IngestorRequest::Call {
                    transaction: call.transaction,
                    timeout: call.timeout,
                    name: call.name,
                    args: call.args,
                    payload: call.payload,
                };
                if let Err(e) = self.ingestor.send(req).await {
                    tracing::error!("failed to send to ingestor: {e}");
                }
            }
            Worker::Ledger => {
                let req = LedgerRequest::Call {
                    transaction: call.transaction,
                    timeout: call.timeout,
                    name: call.name,
                    args: call.args,
                    payload: call.payload,
                };
                if let Err(e) = self.ledger.send(req).await {
                    tracing::error!("failed to send to ledger: {e}");
                }
            }
        }
    }

    /// Route a cancel to the appropriate worker.
    async fn handle_cancel(&mut self, transaction: String) {
        // We don't track which worker owns a transaction, so send to both.
        let req = IngestorRequest::Cancel {
            transaction: transaction.clone(),
        };
        if let Err(e) = self.ingestor.send(req).await {
            tracing::error!("failed to send cancel to ingestor: {e}");
        }
        let req = LedgerRequest::Cancel { transaction };
        if let Err(e) = self.ledger.send(req).await {
            tracing::error!("failed to send cancel to ledger: {e}");
        }
    }

    /// Handle a response from the ingestor.
    async fn handle_ingestor_response(&mut self, resp: IngestorResponse) {
        match resp {
            IngestorResponse::Result(result) => {
                if let Err(e) = self
                    .writer
                    .send(Message::FunctionResult(Box::new(result)))
                    .await
                {
                    tracing::error!("failed to emit result: {e}");
                }
            }
            IngestorResponse::Progress {
                transaction,
                done,
                total,
            } => {
                let msg = Message::FunctionProgressResponse(Box::new(FunctionProgressResponse {
                    transaction,
                    done,
                    all: total,
                }));
                if let Err(e) = self.writer.send(msg).await {
                    tracing::error!("failed to emit progress: {e}");
                }
            }
            IngestorResponse::ChartData { payload } => {
                if let Err(e) = self.writer.write_raw(&payload).await {
                    tracing::error!("failed to emit chart data: {e}");
                }
            }
            IngestorResponse::Ready { .. } => {
                tracing::warn!("unexpected late Ready from ingestor");
            }
        }
    }

    /// Handle a response from the ledger.
    async fn handle_ledger_response(&mut self, resp: LedgerResponse) {
        match resp {
            LedgerResponse::Result(result) => {
                if let Err(e) = self
                    .writer
                    .send(Message::FunctionResult(Box::new(result)))
                    .await
                {
                    tracing::error!("failed to emit result: {e}");
                }
            }
            LedgerResponse::Progress {
                transaction,
                done,
                total,
            } => {
                let msg = Message::FunctionProgressResponse(Box::new(FunctionProgressResponse {
                    transaction,
                    done,
                    all: total,
                }));
                if let Err(e) = self.writer.send(msg).await {
                    tracing::error!("failed to emit progress: {e}");
                }
            }
            LedgerResponse::Ready { .. } => {
                tracing::warn!("unexpected late Ready from ledger");
            }
        }
    }

    /// Handle a parsed message from stdin.
    async fn handle_agent_message(&mut self, msg: Message) {
        match msg {
            Message::FunctionCall(call) => {
                self.handle_function_call(*call).await;
            }
            Message::FunctionCancel(cancel) => {
                self.handle_cancel(cancel.transaction).await;
            }
            other => {
                tracing::trace!("unhandled agent message: {other:?}");
            }
        }
    }

    /// Main event loop: read from stdin + ingestor + ledger.
    ///
    /// If any worker disconnects, the supervisor exits with an error. We
    /// intentionally do not restart workers — the Netdata agent is responsible
    /// for restarting the entire plugin. This avoids the complexity of managing
    /// stale state, partial restarts, and re-indexing races.
    async fn run(&mut self) -> anyhow::Result<()> {
        let mut keepalive = tokio::time::interval(tokio::time::Duration::from_secs(60));

        loop {
            tokio::select! {
                msg = self.reader.recv() => {
                    match msg {
                        Some(Ok(msg)) => self.handle_agent_message(msg).await,
                        Some(Err(e)) => {
                            tracing::error!("stdin parse error: {e}");
                        }
                        None => {
                            tracing::info!("stdin closed, shutting down");
                            return Ok(());
                        }
                    }
                }
                resp = self.ingestor.recv() => {
                    let r = resp.context("ingestor disconnected")?;
                    self.handle_ingestor_response(r).await;
                }
                resp = self.ledger.recv() => {
                    let r = resp.context("ledger disconnected")?;
                    self.handle_ledger_response(r).await;
                }
                _ = keepalive.tick() => {
                    self.writer
                        .write_raw(b"PLUGIN_KEEPALIVE\n")
                        .await
                        .context("keepalive write failed")?;
                }
            }
        }
    }
}

/// Guard that removes a socket file on drop.
struct SocketGuard(std::path::PathBuf);

impl SocketGuard {
    fn new(dir: &std::path::Path, name: &str) -> Self {
        let path = dir.join(format!("{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        Self(path)
    }

    fn path(&self) -> &str {
        self.0.to_str().expect("socket path is not valid UTF-8")
    }
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Resolve the directory for IPC socket files.
///
/// Uses `$NETDATA_RUN_DIR/otel-plugin/` when running under the Netdata
/// agent, falling back to `/tmp` for standalone execution.
fn socket_dir() -> anyhow::Result<std::path::PathBuf> {
    let env = rt::NetdataEnv::from_environment();
    let dir = match env.run_dir {
        Some(run_dir) => run_dir.join("otel-plugin"),
        None => std::path::PathBuf::from("/tmp"),
    };
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create socket directory {}", dir.display()))?;
    tracing::info!("socket directory: {}", dir.display());
    Ok(dir)
}

async fn spawn_worker<S, R>(
    self_exe: &std::path::Path,
    sock: &SocketGuard,
    name: &'static str,
) -> anyhow::Result<(Connection<S, R>, ChildGuard)>
where
    S: serde::Serialize + Send + 'static,
    R: serde::de::DeserializeOwned + Send + 'static,
{
    let mut listener = Listener::<S, R>::bind(Endpoint::ipc(sock.path()))
        .open()
        .with_context(|| format!("failed to bind {name} socket at {}", sock.path()))?;

    tracing::info!("spawning {name} socket={}", sock.path());

    let child = Command::new(self_exe)
        .args(["worker", name, "--socket", sock.path()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("failed to spawn {name} worker"))?;

    // Wrap immediately so the child is killed if accept() fails.
    let guard = ChildGuard::new(child, name);

    let conn = tokio::time::timeout(WORKER_CONNECT_TIMEOUT, listener.accept())
        .await
        .map_err(|_| anyhow::anyhow!("{name} failed to connect within {WORKER_CONNECT_TIMEOUT:?}"))
        .and_then(|r| r.map_err(Into::into))
        .with_context(|| format!("{name} worker connection failed"))?;
    tracing::info!("{name} worker connected to supervisor");

    Ok((conn, guard))
}

/// Entry point for the supervisor mode.
pub async fn run() -> anyhow::Result<()> {
    tracing::info!("starting otel-plugin");

    let self_exe = std::env::current_exe().context("failed to resolve current executable")?;

    let mut plugin_config = config::load_config().context("failed to load configuration")?;

    // Socket guards — cleaned up when the supervisor exits.
    let sock_dir = socket_dir()?;
    let writer_sock = SocketGuard::new(&sock_dir, "writer");
    let ledger_sock = SocketGuard::new(&sock_dir, "ledger");
    let ingestor_sock = SocketGuard::new(&sock_dir, "ingestor");

    plugin_config.writer_socket_path = writer_sock.path().to_string();

    // Spawn ledger first (it must be listening before the ingestor's WriterPublisher connects).
    let (ledger_conn, ledger_child) = spawn_worker(&self_exe, &ledger_sock, "ledger").await?;
    let (ingestor_conn, ingestor_child) =
        spawn_worker(&self_exe, &ingestor_sock, "ingestor").await?;

    let mut supervisor = Supervisor {
        ingestor: ingestor_conn,
        ingestor_child,
        ledger: ledger_conn,
        ledger_child,
        routing: HashMap::new(),
        reader: MessageReader::new(tokio::io::stdin()),
        writer: MessageWriter::new(tokio::io::stdout()),
        sockets: [writer_sock, ledger_sock, ingestor_sock],
    };

    supervisor
        .writer
        .write_raw(b"TRUST_DURATIONS 1\n")
        .await
        .context("failed to write TRUST_DURATIONS to agent")?;

    supervisor
        .configure_ledger(plugin_config.clone())
        .await
        .context("ledger configuration failed")?;

    supervisor
        .configure_ingestor(plugin_config)
        .await
        .context("ingestor configuration failed")?;

    tracing::info!("all workers ready, entering main loop");

    supervisor.run().await
}
