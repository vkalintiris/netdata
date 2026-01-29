//! journal-viewer-plugin standalone binary

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
        "configuration loaded: journal_paths={:?}, cache_dir={}, memory_capacity={}, disk_capacity={}, workers={}",
        config.journal.paths,
        config.cache.directory,
        config.cache.memory_capacity,
        config.cache.disk_capacity,
        config.cache.workers
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
    let catalog_function = CatalogFunction::new(
        monitor,
        &config.cache.directory,
        config.cache.memory_capacity,
        config.cache.disk_capacity.as_u64() as usize,
    )
    .await?;
    info!("catalog function initialized");

    // Watch configured journal directories (defers missing directories for retry)
    for path in &config.journal.paths {
        catalog_function.watch_directory_or_defer(path);
    }

    // Spawn task to retry deferred directories with exponential backoff
    let catalog_for_retry = catalog_function.clone();
    tokio::spawn(async move {
        // Initial delay before first retry check
        tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;

        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;
            let succeeded = catalog_for_retry.retry_pending_directories();
            for path in succeeded {
                info!("now watching previously deferred directory: {}", path);
            }
        }
    });

    runtime.register_handler(catalog_function.clone());
    info!("catalog function handler registered");

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
