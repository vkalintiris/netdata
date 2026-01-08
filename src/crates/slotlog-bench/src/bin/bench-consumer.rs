//! Benchmark consumer using the real slotlog-consumer implementation.

use clap::Parser;
use slotlog::metrics_processor_server::MetricsProcessorServer;
use slotlog_consumer::{ConsumerConfig, SharedSlotLogConsumer};
use tonic::transport::Server;

/// Max gRPC message size (64MB to match producer).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

#[derive(Parser)]
#[command(name = "bench-consumer")]
#[command(about = "Benchmark consumer using slotlog-consumer")]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value = "50051")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let addr = format!("[::1]:{}", args.port).parse()?;

    let consumer = SharedSlotLogConsumer::new(ConsumerConfig {
        compact_on_delete: false,
        max_late_slots: None,
    });

    let service = MetricsProcessorServer::new(consumer)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);

    println!("Starting consumer on {}...", addr);
    println!("Press Ctrl+C to stop.\n");

    Server::builder()
        .add_service(service)
        .serve_with_shutdown(addr, async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen for ctrl-c");
            println!("\nShutting down...");
        })
        .await?;

    Ok(())
}
