//! Benchmark producer - generates synthetic load for the consumer.

use std::time::{Duration, Instant};

use clap::Parser;
use slotlog::ChartType;
use slotlog_producer::{GrpcSender, Producer};

#[derive(Parser)]
#[command(name = "bench-producer")]
#[command(about = "Benchmark producer - generates synthetic load")]
struct Args {
    /// Number of charts to create
    #[arg(short = 'c', long, default_value = "100")]
    charts: usize,

    /// Number of dimensions per chart
    #[arg(short = 'd', long, default_value = "10")]
    dimensions: usize,

    /// Number of slots to send (after initial registration)
    #[arg(short = 'n', long, default_value = "60")]
    iterations: u64,

    /// Consumer address
    #[arg(short = 'a', long, default_value = "http://[::1]:50051")]
    address: String,

    /// Simulate real-time by waiting ~1 second between slots
    #[arg(short = 'r', long)]
    realtime: bool,
}

const SLOT_INTERVAL_SECS: u64 = 1;
const CONSTANT_VALUE: f64 = 1.0;
/// Max gRPC message size (64MB should be enough for large registrations).
const MAX_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let total_dimensions = args.charts * args.dimensions;

    println!("Benchmark Producer Configuration:");
    println!("  Charts: {}", args.charts);
    println!("  Dimensions per chart: {}", args.dimensions);
    println!("  Total dimensions: {}", total_dimensions);
    println!("  Iterations: {}", args.iterations);
    println!("  Slot interval: {}s", SLOT_INTERVAL_SECS);
    println!(
        "  Mode: {}",
        if args.realtime { "realtime" } else { "burst" }
    );
    println!("  Consumer: {}", args.address);
    println!();

    // Connect to consumer
    println!("Connecting to consumer...");
    let sender = GrpcSender::connect_with_config(&args.address, MAX_MESSAGE_SIZE).await?;
    let mut producer = Producer::new(sender);

    // Define all charts and dimensions
    println!(
        "Defining {} charts with {} dimensions each...",
        args.charts, args.dimensions
    );
    let define_start = Instant::now();

    for chart_idx in 0..args.charts {
        let chart_name = format!("bench.chart{}", chart_idx);
        producer.define_chart(&chart_name, ChartType::Gauge)?;

        for dim_idx in 0..args.dimensions {
            let dim_name = format!("dim{}", dim_idx);
            producer.define_dimension(&chart_name, &dim_name)?;
        }
    }

    let define_elapsed = define_start.elapsed();
    println!("  Defined in {:?}", define_elapsed);

    // Slot 0: Register all charts with initial values
    println!("\nSlot 0: Registering all charts...");
    let reg_start = Instant::now();

    producer.begin_slot(0);
    for chart_idx in 0..args.charts {
        let chart_name = format!("bench.chart{}", chart_idx);
        for dim_idx in 0..args.dimensions {
            let dim_name = format!("dim{}", dim_idx);
            producer.update(&chart_name, &dim_name, Some(CONSTANT_VALUE))?;
        }
    }
    producer.send().await?;

    let reg_elapsed = reg_start.elapsed();
    println!("  Registered in {:?}", reg_elapsed);

    // Pre-allocate chart/dimension names for the update loop
    let chart_names: Vec<String> = (0..args.charts)
        .map(|i| format!("bench.chart{}", i))
        .collect();
    let dim_names: Vec<String> = (0..args.dimensions).map(|i| format!("dim{}", i)).collect();

    // Run update iterations
    let mode = if args.realtime { "realtime" } else { "burst" };
    println!(
        "\nRunning {} update iterations ({} mode)...",
        args.iterations, mode
    );
    let update_start = Instant::now();

    let mut total_send_time = Duration::ZERO;
    let mut min_send_time = Duration::MAX;
    let mut max_send_time = Duration::ZERO;

    let slot_duration = Duration::from_secs(SLOT_INTERVAL_SECS);

    for slot in 1..=args.iterations {
        let slot_start = Instant::now();
        let slot_timestamp = slot * SLOT_INTERVAL_SECS;

        producer.begin_slot(slot_timestamp);

        // Update all dimensions with constant value
        for chart_name in &chart_names {
            for dim_name in &dim_names {
                producer.update(chart_name, dim_name, Some(CONSTANT_VALUE))?;
            }
        }

        let send_start = Instant::now();
        producer.send().await?;
        let send_time = send_start.elapsed();

        total_send_time += send_time;
        min_send_time = min_send_time.min(send_time);
        max_send_time = max_send_time.max(send_time);

        // Progress indicator every 10 slots
        if slot % 10 == 0 {
            println!("  Slot {}/{}", slot, args.iterations);
        }

        // In realtime mode, wait for the remainder of the slot interval
        if args.realtime {
            let elapsed = slot_start.elapsed();
            if let Some(remaining) = slot_duration.checked_sub(elapsed) {
                tokio::time::sleep(remaining).await;
            }
        }
    }

    let update_elapsed = update_start.elapsed();

    // Print results
    println!("\n=== Benchmark Results ===");
    println!("Total time: {:?}", update_elapsed);
    println!("Slots sent: {}", args.iterations);
    println!(
        "Total dimensions updated: {}",
        args.iterations as usize * total_dimensions
    );
    println!();

    let avg_send_time = total_send_time / args.iterations as u32;
    println!("Send latency:");
    println!("  Min: {:?}", min_send_time);
    println!("  Max: {:?}", max_send_time);
    println!("  Avg: {:?}", avg_send_time);
    println!();

    let secs = update_elapsed.as_secs_f64();
    if secs > 0.0 {
        let slots_per_sec = args.iterations as f64 / secs;
        let dims_per_sec = (args.iterations as usize * total_dimensions) as f64 / secs;
        println!("Throughput:");
        println!("  Slots/sec: {:.2}", slots_per_sec);
        println!("  Dimensions/sec: {:.2}", dims_per_sec);
    }

    Ok(())
}
