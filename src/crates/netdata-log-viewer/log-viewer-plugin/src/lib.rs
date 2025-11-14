//! log-viewer-plugin library - can be called from multi-call binaries or standalone

#![allow(dead_code)]

use journal_function::{FileIndexCache, JournalMetrics, Monitor};

mod catalog;
use catalog::CatalogFunction;

mod charts;
use charts::Metrics;
mod tracing_config;

use rt::PluginRuntime;
use tracing::{error, info};

/// Create a Foyer hybrid cache for file indexes
async fn create_file_index_cache() -> std::result::Result<FileIndexCache, Box<dyn std::error::Error>>
{
    use foyer::{
        BlockEngineBuilder, DeviceBuilder, FsDeviceBuilder, HybridCacheBuilder, IoEngineBuilder,
        PsyncIoEngineBuilder,
    };

    let memory_capacity = 1000; // 1000 items in memory
    let disk_capacity = 32 * 1024 * 1024; // 32 MiB disk storage
    let cache_dir = "/mnt/ramfs/foyer-cache";

    info!(
        "Building Foyer hybrid cache: memory={} items, disk={} bytes, dir={}",
        memory_capacity, disk_capacity, cache_dir
    );

    // Ensure cache directory exists
    std::fs::create_dir_all(cache_dir)?;

    let cache = HybridCacheBuilder::new()
        .with_name("file-index-cache")
        .memory(memory_capacity)
        .with_shards(4)
        .storage()
        .with_io_engine(PsyncIoEngineBuilder::new().build().await?)
        .with_engine_config(
            BlockEngineBuilder::new(
                FsDeviceBuilder::new(cache_dir)
                    .with_capacity(disk_capacity)
                    .build()?,
            )
            .with_block_size(4 * 1024 * 1024), // 4 MiB blocks
        )
        .build()
        .await?;

    info!("Foyer hybrid cache built successfully");

    Ok(FileIndexCache::with_foyer(cache))
}

/// Entry point for log-viewer-plugin - can be called from multi-call binary
///
/// # Arguments
/// * `args` - Command-line arguments (should include argv[0] as "log-viewer-plugin")
///
/// # Returns
/// Exit code (0 for success, non-zero for errors)
pub fn run(args: Vec<String>) -> i32 {
    // log-viewer-plugin is async, so we need a tokio runtime
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async_run(args))
}

async fn async_run(_args: Vec<String>) -> i32 {
    println!("TRUST_DURATIONS 1");

    tracing_config::initialize_tracing(tracing_config::TracingConfig::default());

    let result = run_internal().await;

    match result {
        Ok(()) => {
            info!("Plugin runtime stopped");
            0
        }
        Err(e) => {
            error!("Plugin error: {:#}", e);
            1
        }
    }
}

async fn run_internal() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut runtime = PluginRuntime::new("log-viewer");
    info!("Plugin runtime created");

    // Initialize plugin-level metrics
    let _plugin_metrics = Metrics::new(&mut runtime);
    info!("Plugin metrics initialized");

    // Initialize journal-function metrics
    let journal_metrics = JournalMetrics::new(&mut runtime);
    info!("Journal metrics initialized");

    // Create file index cache with Foyer hybrid cache
    info!("Initializing file index cache");
    let file_index_cache = create_file_index_cache().await?;
    info!("File index cache initialized");

    let (monitor, notify_rx) = match Monitor::new() {
        Ok(t) => t,
        Err(e) => {
            error!("Failed to setup notify monitoring: {}", e);
            return Ok(());
        }
    };

    let catalog_function = CatalogFunction::new(
        monitor,
        file_index_cache,
        journal_metrics.file_indexing.clone(),
        journal_metrics.bucket_cache.clone(),
        journal_metrics.bucket_operations.clone(),
    );

    match catalog_function.watch_directory("/var/log/journal") {
        Ok(()) => {}
        Err(e) => {
            error!("Failed to watch directory: {:#?}", e);
        }
    };

    runtime.register_handler(catalog_function.clone());
    info!("Catalog function handler registered");

    // Spawn task to process notify events
    let catalog_function_clone = catalog_function.clone();
    tokio::spawn(async move {
        let mut notify_rx = notify_rx;
        while let Some(event) = notify_rx.recv().await {
            catalog_function_clone.process_notify_event(event);
        }
        info!("Notify event processing task terminated");
    });

    info!("Starting plugin runtime - ready to process function calls");
    runtime.run().await?;

    Ok(())
}
