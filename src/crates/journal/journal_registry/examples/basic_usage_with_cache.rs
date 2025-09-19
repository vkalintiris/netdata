use allocative::FlameGraphBuilder;
use journal_file::JournalFile;
use journal_file::Mmap;
use journal_file::index::FileIndex;
use journal_registry::JournalRegistry;
use journal_registry::cache::JournalIndexCache;
use std::time::Duration;
use std::time::Instant;
use tracing::{info, warn};

async fn sequential_with_cache(
    files: &[journal_registry::RegistryFile],
    cache: &JournalIndexCache,
) -> Vec<FileIndex> {
    let start_time = Instant::now();

    let systemd_keys: Vec<&[u8]> = vec![
        // --- USER JOURNAL FIELDS ---
        b"MESSAGE_ID",
        b"PRIORITY",
        b"CODE_FILE",
        b"CODE_FUNC",
        b"ERRNO",
        b"SYSLOG_FACILITY",
        b"SYSLOG_IDENTIFIER",
        b"UNIT",
        b"USER_UNIT",
        b"UNIT_RESULT",
        // --- TRUSTED JOURNAL FIELDS ---
        b"_UID",
        b"_GID",
        b"_COMM",
        b"_EXE",
        b"_CAP_EFFECTIVE",
        b"_AUDIT_LOGINUID",
        b"_SYSTEMD_CGROUP",
        b"_SYSTEMD_SLICE",
        b"_SYSTEMD_UNIT",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        b"_SYSTEMD_SESSION",
        b"_SYSTEMD_OWNER_UID",
        b"_SELINUX_CONTEXT",
        b"_BOOT_ID",
        b"_MACHINE_ID",
        b"_HOSTNAME",
        b"_TRANSPORT",
        b"_STREAM_ID",
        b"_NAMESPACE",
        b"_RUNTIME_SCOPE",
        // --- KERNEL JOURNAL FIELDS ---
        b"_KERNEL_SUBSYSTEM",
        b"_UDEV_DEVNODE",
        // --- LOGGING ON BEHALF ---
        b"OBJECT_UID",
        b"OBJECT_GID",
        b"OBJECT_COMM",
        b"OBJECT_EXE",
        b"OBJECT_AUDIT_LOGINUID",
        b"OBJECT_SYSTEMD_CGROUP",
        b"OBJECT_SYSTEMD_SESSION",
        b"OBJECT_SYSTEMD_OWNER_UID",
        b"OBJECT_SYSTEMD_UNIT",
        b"OBJECT_SYSTEMD_USER_UNIT",
        // --- CORE DUMPS ---
        b"COREDUMP_COMM",
        b"COREDUMP_UNIT",
        b"COREDUMP_USER_UNIT",
        b"COREDUMP_SIGNAL_NAME",
        b"COREDUMP_CGROUP",
        // --- DOCKER ---
        b"CONTAINER_ID",
        b"CONTAINER_NAME",
        b"CONTAINER_TAG",
        b"IMAGE_NAME",
        // --- NETDATA ---
        b"ND_NIDL_NODE",
        b"ND_NIDL_CONTEXT",
        b"ND_LOG_SOURCE",
        b"ND_ALERT_NAME",
        b"ND_ALERT_CLASS",
        b"ND_ALERT_COMPONENT",
        b"ND_ALERT_TYPE",
        b"ND_ALERT_STATUS",
    ];

    let mut total_index_size = 0;
    let mut file_indexes = Vec::new();
    let mut cache_hits = 0;
    let mut cache_misses = 0;

    for file in files {
        // Try to get from cache first
        match cache.get(&file.path).await {
            Ok(Some(cached_index)) => {
                // Cache hit - use cached index
                cache_hits += 1;

                let mut index_size = 0;
                for (data_payload, entry_indices) in cached_index.entry_indices.iter() {
                    index_size += data_payload.len() + entry_indices.len() as usize;
                }

                // info!(
                //     file = file.path.to_string_lossy().into_owned(),
                //     index_size,
                //     source = "cache"
                // );

                total_index_size += index_size;
                file_indexes.push(cached_index);
            }
            Ok(None) => {
                // Cache miss - compute and store in cache
                cache_misses += 1;

                let window_size = 8 * 1024 * 1024;
                let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

                let Ok(jfi) = FileIndex::from(&journal_file, systemd_keys.as_slice()) else {
                    continue;
                };

                let mut index_size = 0;
                for (data_payload, entry_indices) in jfi.entry_indices.iter() {
                    index_size += data_payload.len() + entry_indices.len() as usize;
                }

                // info!(
                //     file = file.path.to_string_lossy().into_owned(),
                //     index_size,
                //     source = "computed"
                // );

                total_index_size += index_size;

                // Store in cache for next time
                cache.insert(&file.path, jfi.clone());
                file_indexes.push(jfi);
            }
            Err(e) => {
                warn!("Cache error for {}: {}", file.path.display(), e);
                cache_misses += 1;

                // Fallback to compute without caching
                let window_size = 8 * 1024 * 1024;
                let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

                let Ok(jfi) = FileIndex::from(&journal_file, systemd_keys.as_slice()) else {
                    continue;
                };

                let mut index_size = 0;
                for (data_payload, entry_indices) in jfi.entry_indices.iter() {
                    index_size += data_payload.len() + entry_indices.len() as usize;
                }

                info!(
                    file = file.path.to_string_lossy().into_owned(),
                    index_size,
                    source = "fallback"
                );

                total_index_size += index_size;
                file_indexes.push(jfi);
            }
        }
    }

    // Count histogram buckets after processing
    let midx_count: usize = file_indexes.iter().map(|fi| fi.file_histogram.len()).sum();

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} histogram index buckets in {:#?} msec (cache hits: {}, misses: {})",
        midx_count,
        elapsed.as_millis(),
        cache_hits,
        cache_misses,
    );
    info!(
        "total index size: {:#?} MiB",
        total_index_size / (1024 * 1024)
    );

    file_indexes
}

use std::path::PathBuf;
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    info!("Creating journal index cache with 32 MiB disk capacity");

    // Create a cache with 32 MiB disk capacity and 8 MiB memory capacity
    let cache = JournalIndexCache::new(
        PathBuf::from("/tmp/fcache"),
        32 * 1024 * 1024,      // 32 MiB disk
        Some(8 * 1024 * 1024), // 8 MiB memory
    )
    .await?;

    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    for dir in ["/var/log/journal", "/run/log/journal"] {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    let mut files = registry.query().execute();
    files.sort_by_key(|x| x.path.clone());
    files.sort_by_key(|x| x.size);
    files.reverse();

    // First run - should be all cache misses
    info!("=== FIRST RUN (should be all cache misses when running the example _first_ time) ===");
    let v1 = sequential_with_cache(&files, &cache).await;

    // Second run - should have many cache hits
    info!("=== SECOND RUN (should have cache hits) ===");
    let v2 = sequential_with_cache(&files, &cache).await;

    // Generate flamegraph for the first run
    let mut flamegraph = FlameGraphBuilder::default();
    flamegraph.visit_root(&v1);
    let flamegraph_src = flamegraph.finish().flamegraph().write();
    std::fs::write("/tmp/flamegraph_cached.txt", flamegraph_src).unwrap();

    // Calculate and report compression ratios
    println!("\n=== Compression Ratios ===");
    let mut total_lz4_roaring_size = 0usize;

    for file_index in &v2 {
        // Use second run results
        for roaring_data in file_index.entry_indices.values() {
            total_lz4_roaring_size += roaring_data.len() as usize;
        }
    }

    println!("\nRoaring bitmap data:");
    println!("  LZ4 compressed: {} bytes", total_lz4_roaring_size);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    // Cache statistics
    let stats = cache.stats().await;
    println!("\n=== Cache Statistics ===");
    println!("Memory entries: {}", stats.memory_entries);
    println!("Disk entries: {}", stats.disk_entries);
    println!("Memory usage: {} bytes", stats.memory_bytes);
    println!("Disk usage: {} bytes", stats.disk_bytes);

    // Properly close the cache
    cache.close().await?;

    tokio::time::sleep(Duration::from_secs(3600)).await;

    Ok(())
}
