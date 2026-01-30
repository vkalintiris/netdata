mod aggregation;
mod chart;
mod config;
mod iter;
mod otel;
mod output;
mod service;
mod slot;

use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::MetricsServiceServer;
use tokio::sync::RwLock;
use tonic::transport::Server;

use chart::ChartConfig;
use config::ChartConfigManager;
use service::{ChartManager, NetdataMetricsService, create_shared_output, spawn_tick_task};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "0.0.0.0:4317".parse()?;

    println!("Listening for OTLP metrics on {}", addr);

    // Create shared state
    let chart_config = ChartConfig::default();
    let tick_interval = Duration::from_secs(chart_config.interval_secs);

    let ccm = Arc::new(RwLock::new(ChartConfigManager::with_default_configs()));
    let chart_manager = Arc::new(RwLock::new(ChartManager::new(chart_config)));
    let output = create_shared_output();

    // Create the service
    let svc = NetdataMetricsService::new(ccm, Arc::clone(&chart_manager), Arc::clone(&output));

    // Spawn the background tick task
    let tick_handle = spawn_tick_task(chart_manager, output, tick_interval);

    println!(
        "Started background tick task (interval: {}s)",
        tick_interval.as_secs()
    );

    // Run the gRPC server
    let result = Server::builder()
        .add_service(
            MetricsServiceServer::new(svc)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await;

    // Clean up the tick task on shutdown
    tick_handle.abort();

    result?;

    Ok(())
}
