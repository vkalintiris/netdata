mod config;
mod supervisor;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Subcommand)]
enum WorkerKind {
    /// Run the ingestor worker.
    Ingestor {
        /// IPC socket path for communication with the supervisor.
        #[arg(long)]
        socket: String,
    },
    /// Run the ledger worker.
    Ledger {
        /// IPC socket path for communication with the supervisor.
        #[arg(long)]
        socket: String,
    },
}

#[derive(Subcommand)]
enum CliCommand {
    /// Run as a worker subprocess (used internally by the supervisor).
    Worker {
        #[command(subcommand)]
        kind: WorkerKind,
    },
}

#[derive(Parser)]
#[command(name = "otel-plugin")]
struct Cli {
    /// Collection frequency passed by the Netdata agent (ignored).
    #[arg(hide = true)]
    _update_every: Option<u64>,

    #[command(subcommand)]
    command: Option<CliCommand>,
}

async fn run_worker(kind: WorkerKind) -> anyhow::Result<()> {
    match kind {
        WorkerKind::Ingestor { socket } => otel_ingestor::run_worker(&socket)
            .await
            .context("ingestor worker failed"),
        WorkerKind::Ledger { socket } => journal_ledger::run_worker(&socket)
            .await
            .context("ledger worker failed"),
    }
}

#[tokio::main]
async fn main() {
    rt::init_tracing();

    let cli = Cli::parse();

    let result = match cli.command {
        Some(CliCommand::Worker { kind }) => run_worker(kind).await,
        None => supervisor::run().await,
    };

    if let Err(e) = result {
        tracing::error!("{e:#}");
        std::process::exit(1);
    }
}
