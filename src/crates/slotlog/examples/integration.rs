//! Integration example demonstrating the slot log protocol.
//!
//! This example runs a consumer and producer in the same process,
//! demonstrating the full registration and update flow.

use std::sync::Arc;

use slotlog::ChartType;
use slotlog::metrics_processor_server::MetricsProcessorServer;
use slotlog_consumer::{ConsumerConfig, SharedSlotLogConsumer};
use slotlog_producer::{GrpcSender, Producer};
use tokio::sync::Notify;
use tonic::transport::Server;

const SERVER_ADDR: &str = "127.0.0.1:50099";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create the consumer
    let consumer = SharedSlotLogConsumer::new(ConsumerConfig {
        compact_on_delete: false,
        max_late_slots: Some(60),
    });

    // Start the server in the background
    let server_ready = Arc::new(Notify::new());
    let server_ready_clone = server_ready.clone();

    let server_handle = tokio::spawn(async move {
        println!("Starting consumer server on {SERVER_ADDR}...");

        Server::builder()
            .add_service(MetricsProcessorServer::new(consumer))
            .serve_with_shutdown(SERVER_ADDR.parse().unwrap(), async move {
                server_ready_clone.notify_one();
                // Keep running until interrupted
                tokio::signal::ctrl_c()
                    .await
                    .expect("Failed to listen for ctrl-c");
            })
            .await
            .expect("Server failed");
    });

    // Give the server a moment to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    server_ready.notify_one();

    // Connect the producer
    println!("Connecting producer to http://{SERVER_ADDR}...");
    let sender = GrpcSender::connect(&format!("http://{SERVER_ADDR}")).await?;
    let mut producer = Producer::new(sender);

    // Define charts and dimensions upfront
    producer.define_chart("cpu.usage", ChartType::Gauge)?;
    producer.define_dimension("cpu.usage", "user")?;
    producer.define_dimension("cpu.usage", "system")?;

    // Slot 1: Register the chart with initial values
    println!("\n=== Slot 1000: Registering chart ===");
    producer.begin_slot(1000);
    producer.update("cpu.usage", "user", Some(25.5))?;
    producer.update("cpu.usage", "system", Some(10.2))?;
    producer.send().await?;

    println!("Chart registered: cpu.usage");
    println!("  user -> 25.5");
    println!("  system -> 10.2");

    // Slot 2: Update with new values and a new dimension
    println!("\n=== Slot 1001: Update with new dimension ===");
    producer.define_dimension("cpu.usage", "iowait")?;

    producer.begin_slot(1001);
    producer.update("cpu.usage", "user", Some(30.0))?;
    producer.update("cpu.usage", "system", Some(12.5))?;
    producer.update("cpu.usage", "iowait", Some(5.0))?;
    producer.send().await?;

    println!("Values updated, new dimension added:");
    println!("  user -> 30.0");
    println!("  system -> 12.5");
    println!("  iowait -> 5.0");

    // Slot 3: Add another chart
    println!("\n=== Slot 1002: Register second chart ===");
    producer.define_chart("memory.usage", ChartType::Gauge)?;
    producer.define_dimension("memory.usage", "used")?;
    producer.define_dimension("memory.usage", "cached")?;

    producer.begin_slot(1002);
    producer.update("cpu.usage", "user", Some(28.0))?;
    producer.update("cpu.usage", "system", Some(11.0))?;
    producer.update("cpu.usage", "iowait", Some(3.5))?;
    producer.update("memory.usage", "used", Some(8_000_000_000.0))?;
    producer.update("memory.usage", "cached", Some(2_000_000_000.0))?;
    producer.send().await?;

    println!("Second chart registered: memory.usage");
    println!("  used -> 8000000000.0");
    println!("  cached -> 2000000000.0");

    println!("\n=== Summary ===");
    println!("Charts defined: cpu.usage, memory.usage");
    println!("Press Ctrl+C to stop the server...");

    // Wait for the server to finish
    server_handle.await?;

    Ok(())
}
