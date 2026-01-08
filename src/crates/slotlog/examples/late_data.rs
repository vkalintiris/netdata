//! Late data integration example demonstrating the two-phase protocol.
//!
//! This example shows how late-arriving data is handled:
//! 1. Late data for unregistered chart -> registration in slot N, values in slot N+1
//! 2. Late data for registered chart -> sent immediately

use std::sync::Arc;

use slotlog::ChartType;
use slotlog::metrics_processor_server::MetricsProcessorServer;
use slotlog_consumer::{ConsumerConfig, SharedSlotLogConsumer};
use slotlog_producer::{GrpcSender, Producer};
use tokio::sync::Notify;
use tonic::transport::Server;

const SERVER_ADDR: &str = "127.0.0.1:50098";

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

    tokio::spawn(async move {
        println!("Starting consumer server on {SERVER_ADDR}...");

        Server::builder()
            .add_service(MetricsProcessorServer::new(consumer))
            .serve_with_shutdown(SERVER_ADDR.parse().unwrap(), async move {
                server_ready_clone.notify_one();
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
    println!("Connecting producer to http://{SERVER_ADDR}...\n");
    let sender = GrpcSender::connect(&format!("http://{SERVER_ADDR}")).await?;
    let mut producer = Producer::new(sender);

    // =========================================================================
    // Scenario 1: Late data for unregistered chart (two-phase)
    // =========================================================================
    println!("=== Scenario 1: Late data for UNREGISTERED chart ===\n");

    // Define the chart (but don't send any current data yet)
    producer.define_chart("late.metric", ChartType::Gauge)?;
    producer.define_dimension("late.metric", "value")?;

    println!("Slot 1000: Late data arrives for 'late.metric' (not registered with consumer)");
    producer.begin_slot(1000);
    producer.update_late(998, "late.metric", "value", Some(42.0))?;
    producer.send().await?;

    println!("  -> Chart registered with consumer");
    println!("  -> Values are deferred (waiting for ID assignment)\n");

    println!("Slot 1001: Deferred late data is sent with assigned IDs");
    producer.begin_slot(1001);
    producer.send().await?;

    println!("  -> Late values for slot 998 now sent to consumer!\n");

    // =========================================================================
    // Scenario 2: Late data for registered chart (immediate)
    // =========================================================================
    println!("=== Scenario 2: Late data for REGISTERED chart ===\n");

    // Define and register chart normally
    producer.define_chart("current.metric", ChartType::Gauge)?;
    producer.define_dimension("current.metric", "dim1")?;

    println!("Slot 1002: Register 'current.metric' normally");
    producer.begin_slot(1002);
    producer.update("current.metric", "dim1", Some(100.0))?;
    producer.send().await?;

    println!("  -> Chart registered: current.metric");

    // Now send late data for the registered chart with a new dimension
    producer.define_dimension("current.metric", "dim2")?;

    println!("\nSlot 1003: Late data arrives for registered 'current.metric'");
    producer.begin_slot(1003);
    producer.update_late(1001, "current.metric", "dim1", Some(50.0))?;
    producer.update_late(1001, "current.metric", "dim2", Some(75.0))?;
    producer.send().await?;

    println!("  -> Late values sent immediately (chart was registered)");
    println!("  -> New dimension 'dim2' registered via late update");

    // =========================================================================
    // Scenario 3: Mixed current + late data
    // =========================================================================
    println!("\n=== Scenario 3: Mixed current + late data ===\n");

    // Define new charts
    producer.define_chart("new.metric", ChartType::DeltaSum)?;
    producer.define_dimension("new.metric", "count")?;

    producer.define_chart("another.late", ChartType::CumulativeSum)?;
    producer.define_dimension("another.late", "total")?;

    println!("Slot 1004: Current data for new chart + late data for unregistered chart");
    producer.begin_slot(1004);
    producer.update("new.metric", "count", Some(1.0))?;
    producer.update_late(1002, "another.late", "total", Some(1000.0))?;
    producer.send().await?;

    println!("  -> new.metric registered");
    println!("  -> another.late registered (late values deferred)");

    println!("\nSlot 1005: Deferred data sent");
    producer.begin_slot(1005);
    producer.send().await?;
    println!("  -> Deferred late values now sent\n");

    // =========================================================================
    // Summary
    // =========================================================================
    println!("=== Summary ===");
    println!("Charts defined:");
    println!("  - late.metric (Gauge)");
    println!("  - current.metric (Gauge)");
    println!("  - new.metric (DeltaSum)");
    println!("  - another.late (CumulativeSum)");

    println!("\nLate data handling demonstrated successfully!");
    println!("Press Ctrl+C to stop the server...");

    // Wait briefly then exit
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    Ok(())
}
