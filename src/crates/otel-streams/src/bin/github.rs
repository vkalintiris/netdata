use std::time::Duration;

use clap::Parser;
use tokio::sync::mpsc;

use otel_streams::Source;
use otel_streams::args;
use otel_streams::github::{self, Github};
use otel_streams::sender::{OtelConfig, Sender};

#[derive(Parser)]
#[command(name = "github")]
#[command(about = "Replays GitHub events from GH Archive as OTel logs")]
struct Args {
    #[command(flatten)]
    common: args::CommonArgs,

    /// Starting hour in YYYY-MM-DD-H format (default: previous UTC hour)
    #[arg(long)]
    start: Option<String>,

    /// Target events per second (0 = unlimited)
    #[arg(long, default_value_t = 100)]
    rate: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    args::init_tls_and_logging(&args.common.log_level);

    let (record_tx, record_rx) = mpsc::channel(1000);

    let flush_interval = Duration::from_millis(args.common.flush_interval_ms);
    let config = OtelConfig {
        endpoint: args.common.otel_endpoint,
        batch_size: args.common.batch_size,
        flush_interval,
        tenant_id: args.common.tenant_id,
        service_name: Github::SERVICE_NAME,
        scope_name: Github::SCOPE_NAME,
        scope_version: Github::SCOPE_VERSION,
    };
    let mut sender = Sender::new(config, record_rx).await?;
    let _sender_handle = tokio::spawn(async move { sender.run().await });

    let (event_tx, mut event_rx) = mpsc::channel(1000);

    let _mapper_handle = tokio::spawn(async move {
        while let Some((event, raw_json)) = event_rx.recv().await {
            let record = Github::event_to_log_record(&event, &raw_json);
            if record_tx.send(record).await.is_err() {
                break;
            }
        }
    });

    github::replay_loop(args.start, args.rate, event_tx).await;

    Ok(())
}
