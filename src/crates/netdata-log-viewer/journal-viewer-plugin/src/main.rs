//! journal-viewer-plugin standalone binary

use journal_function::{IndexingLimits, JournalMetrics};
use journal_registry::Monitor;

mod catalog;
use catalog::CatalogFunction;

mod plugin_config;
use plugin_config::PluginConfig;

use rt::PluginRuntime;
use tracing::{error, info};

#[tokio::main]
async fn main() {
    // Install SIGBUS handler first, before any memory-mapped file operations,
    // because systemd can rotate journal files at any time.
    if let Err(e) = journal_core::install_sigbus_handler() {
        eprintln!("failed to install SIGBUS handler: {}", e);
        std::process::exit(1);
    }

    println!("TRUST_DURATIONS 1");

    rt::init_tracing();

    let result = run_plugin().await;

    match result {
        Ok(()) => {
            info!("plugin runtime stopped");
        }
        Err(e) => {
            error!("plugin error: {:#}", e);
            std::process::exit(1);
        }
    }
}

async fn run_plugin() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Load configuration
    let plugin_config = PluginConfig::new()?;
    let config = &plugin_config.config;

    info!(
        "configuration loaded: journal_paths={:?}, cache_dir={}, memory_capacity={}, disk_capacity={}, workers={}, max_unique_values_per_field={}, max_field_payload_size={}",
        config.journal.paths,
        config.cache.directory,
        config.cache.memory_capacity,
        config.cache.disk_capacity,
        config.cache.workers,
        config.indexing.max_unique_values_per_field,
        config.indexing.max_field_payload_size
    );

    let mut runtime = PluginRuntime::new("journal-viewer");
    info!("plugin runtime created");

    let (monitor, notify_rx) = match Monitor::new() {
        Ok(t) => t,
        Err(e) => {
            error!("failed to setup notify monitoring: {}", e);
            return Ok(());
        }
    };

    // Create catalog function with disk-backed cache
    info!("creating catalog function with Foyer hybrid cache");
    let indexing_limits = IndexingLimits {
        max_unique_values_per_field: config.indexing.max_unique_values_per_field,
        max_field_payload_size: config.indexing.max_field_payload_size,
    };
    let catalog_function = CatalogFunction::new(
        monitor,
        &config.cache.directory,
        config.cache.memory_capacity,
        config.cache.disk_capacity.as_u64() as usize,
        indexing_limits,
    )
    .await?;
    info!("catalog function initialized");

    // Watch configured journal directories
    for path in &config.journal.paths {
        match catalog_function.watch_directory(path) {
            Ok(()) => {
                info!("watching journal directory: {}", path);
            }
            Err(e) => {
                error!("failed to watch directory {}: {:#?}", path, e);
            }
        }
    }

    runtime.register_handler(catalog_function.clone());
    info!("catalog function handler registered");

    // Register cache metrics charts
    let metrics = JournalMetrics::new(&mut runtime);

    // Spawn task to poll foyer cache metrics every second
    let metrics_catalog = catalog_function.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
        loop {
            interval.tick().await;

            let cache = metrics_catalog.cache();

            // Memory cache state
            let usage = cache.memory().usage() as u64;
            let capacity = cache.memory().capacity() as u64;
            metrics.foyer_memory_state.update(|m| {
                m.usage = usage;
                m.capacity = capacity;
            });

            // Event counters
            let snap = metrics_catalog.event_counters().get();
            metrics.foyer_memory_events.update(|m| {
                m.evictions = snap.evictions;
                m.replacements = snap.replacements;
                m.removals = snap.removals;
            });

            // File indexing stats
            let idx = metrics_catalog.indexing_counters().get();
            metrics.file_indexing.update(|m| {
                m.computed = idx.computed;
                m.cached = idx.cache_hits;
            });

            // Disk I/O stats
            let stats = cache.statistics();
            metrics.foyer_disk_io.update(|m| {
                m.write_bytes = stats.disk_write_bytes() as u64;
                m.read_bytes = stats.disk_read_bytes() as u64;
            });
            metrics.foyer_disk_ops.update(|m| {
                m.write_ops = stats.disk_write_ios() as u64;
                m.read_ops = stats.disk_read_ios() as u64;
            });
        }
    });

    // Spawn task to process notify events
    let catalog_function_clone = catalog_function.clone();
    tokio::spawn(async move {
        let mut notify_rx = notify_rx;
        while let Some(event) = notify_rx.recv().await {
            catalog_function_clone.process_notify_event(event);
        }
        info!("notify event processing task terminated");
    });

    // Keepalive future to prevent Netdata from killing the plugin
    let writer = runtime.writer();
    let keepalive = async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            if let Ok(mut w) = writer.try_lock() {
                let _ = w.write_raw(b"PLUGIN_KEEPALIVE\n").await;
            }
        }
    };

    info!("starting plugin runtime");

    // Run plugin runtime and keepalive concurrently
    tokio::select! {
        result = runtime.run() => {
            result?;
        }
        _ = keepalive => {
            // Keepalive loop never completes normally
        }
    }

    Ok(())
}
