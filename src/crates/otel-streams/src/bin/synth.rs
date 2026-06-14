use std::time::Duration;

use clap::Parser;
use tokio::sync::mpsc;
use tracing::info;

use otel_streams::args::{self, CommonArgs};
use otel_streams::otel::now_unix_nanos;
use otel_streams::sender::{OtelConfig, Sender};
use otel_streams::synth::{SynthParams, generate};

#[derive(Parser)]
#[command(name = "synth")]
#[command(about = "Send a deterministic synthetic batch of OTLP logs to an endpoint (for testing)")]
struct Args {
    #[command(flatten)]
    common: CommonArgs,

    /// Number of log records to generate and send.
    #[arg(long, default_value_t = 100)]
    count: usize,

    /// Distinct values per mid-cardinality attribute (host, code).
    #[arg(long, default_value_t = 100)]
    field_cardinality: usize,

    /// Nanoseconds between consecutive records.
    #[arg(long, default_value_t = 1_000_000_000)]
    spacing_nanos: u64,

    /// Timestamp of the first record (unix nanos). Default: now − count·spacing,
    /// so the batch lands in the recent past and a "last N hours" query sees it.
    #[arg(long)]
    start_time_nanos: Option<u64>,

    /// Value-selection offset added before the field-cardinality modulo. Seeds
    /// differing by less than --field-cardinality give distinct corpora; a
    /// multiple of it collides. Use seeds in [0, field-cardinality).
    #[arg(long, default_value_t = 0)]
    seed: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args::init_tls_and_logging(&args.common.log_level);

    let spread = (args.count as u64).saturating_mul(args.spacing_nanos);
    let start = args
        .start_time_nanos
        .unwrap_or_else(|| now_unix_nanos().saturating_sub(spread));
    let records = generate(&SynthParams {
        count: args.count,
        start_time_nanos: start,
        spacing_nanos: args.spacing_nanos,
        field_cardinality: args.field_cardinality,
        seed: args.seed,
    });
    let total = records.len();

    // Reuse the production sender (connect-retry, batching, tenant header).
    let (tx, rx) = mpsc::channel(1000);
    let config = OtelConfig {
        endpoint: args.common.otel_endpoint.clone(),
        batch_size: args.common.batch_size,
        flush_interval: Duration::from_millis(args.common.flush_interval_ms),
        tenant_id: args.common.tenant_id.clone(),
        service_name: "otel-streams-synth",
        scope_name: "synth",
        scope_version: "1.0",
    };
    let mut sender = Sender::new(config, rx).await?;
    let handle = tokio::spawn(async move { sender.run().await });

    for record in records {
        tx.send(record)
            .await
            .map_err(|_| anyhow::anyhow!("sender stopped before all records were queued"))?;
    }
    drop(tx); // closes the channel → sender flushes the remainder and returns
    handle.await?;

    info!(count = total, endpoint = %args.common.otel_endpoint, "synthetic logs sent");
    Ok(())
}
