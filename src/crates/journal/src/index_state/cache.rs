//! File index cache with background indexing workers.
//!
//! This module implements a concurrent file indexing system for UI dashboards that continuously
//! poll for histogram data. Requests are decomposed into individual file indexing tasks and
//! distributed to a pool of Y worker threads through a bounded channel of size X.
//!
//! # Request Flow and Prioritization
//!
//! When indexing requests arrive via `send_request`, they are pushed to the bounded channel using
//! `try_send`. If the channel is full, the request is silently dropped. This is by design: the
//! UI polls continuously (typically every second), so dropped requests will be resubmitted shortly.
//! This behavior creates implicit prioritization where newer requests naturally take precedence
//! during periods of high load.
//!
//! Each request carries a timestamp. Workers check this timestamp before processing and drop
//! requests older than Z seconds. This age-based filtering works together with the bounded channel
//! to ensure workers focus on recent requests. When a user switches from a long time range to a
//! short one mid-indexing, old requests either fail to queue or age out, allowing the new requests
//! to be processed quickly.
//!
//! Before indexing a file, workers check if the cache already contains an index with equal or
//! finer granularity. Only cache misses or requests for finer granularity proceed to actual
//! indexing. Workers use thread-local `FileIndexer` instances to avoid contention, and completed
//! indexes are stored in a shared cache protected by an RwLock.
//!
//! # Statistics
//!
//! When built with the `indexing-stats` feature, the cache tracks queue pressure (successful vs
//! failed sends), age-based drops, cache hit rates, and latency percentiles for both queue wait
//! time and indexing duration. These metrics help tune the channel size and timeout parameters.

use crate::collections::HashSet;
use crate::index::{FileIndex, FileIndexer};
use crate::index_state::request::BucketRequest;
use crate::index_state::response::BucketPartialResponse;
use crate::repository::File;
use crate::{JournalFile, file::Mmap};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use lru::LruCache;
use std::cell::RefCell;
use std::sync::{
    Arc,
    mpsc::{Receiver, SyncSender, sync_channel},
};
use std::time::{Duration, Instant};

#[cfg(feature = "indexing-stats")]
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "indexing-stats")]
use hdrhistogram::Histogram;

thread_local! {
    static FILE_INDEXER: RefCell<FileIndexer> = RefCell::new(FileIndexer::default());
}

/// User-facing statistics snapshot (requires `indexing-stats` feature).
#[cfg(feature = "indexing-stats")]
#[derive(Debug, Clone)]
pub struct IndexingStats {
    pub try_send_succeeded: u64,
    pub try_send_failed: u64,
    pub dropped_age_timeout: u64,
    pub skipped_already_cached: u64,
    pub indexed_successfully: u64,
    pub indexing_failed: u64,

    /// Indexing time percentiles in milliseconds (None if no samples)
    pub indexing_time_p50_ms: Option<f64>,
    pub indexing_time_p99_ms: Option<f64>,
    pub indexing_time_max_ms: Option<f64>,

    /// Request latency percentiles in milliseconds (None if no samples)
    pub request_latency_p50_ms: Option<f64>,
    pub request_latency_p99_ms: Option<f64>,
    pub request_latency_max_ms: Option<f64>,

    /// File index size percentiles in bytes (None if no samples)
    pub file_index_size_mean_bytes: Option<f64>,
    pub file_index_size_p90_bytes: Option<f64>,
    pub file_index_size_p95_bytes: Option<f64>,
    pub file_index_size_p99_bytes: Option<f64>,
    pub file_index_size_max_bytes: Option<f64>,

    /// Foyer cache statistics
    pub foyer_memory_usage_bytes: usize,
    pub foyer_memory_capacity_bytes: usize,
    pub foyer_disk_write_bytes: usize,
    pub foyer_disk_read_bytes: usize,
    pub foyer_disk_write_ios: usize,
    pub foyer_disk_read_ios: usize,
}

/// Internal metrics implementation with atomics and histograms (requires `indexing-stats` feature).
#[cfg(feature = "indexing-stats")]
struct IndexingMetrics {
    // Histograms (in microseconds for time, bytes for size)
    indexing_time_us: parking_lot::Mutex<Histogram<u64>>,
    request_latency_us: parking_lot::Mutex<Histogram<u64>>,
    file_index_size_bytes: parking_lot::Mutex<Histogram<u64>>,

    // Counters
    try_send_succeeded: AtomicU64,
    try_send_failed: AtomicU64,
    dropped_age_timeout: AtomicU64,
    skipped_already_cached: AtomicU64,
    indexed_successfully: AtomicU64,
    indexing_failed: AtomicU64,
}

#[cfg(feature = "indexing-stats")]
impl IndexingMetrics {
    fn new() -> Self {
        // Track up to 60 seconds (60,000,000 microseconds) with 3 significant digits
        let indexing_time_us =
            parking_lot::Mutex::new(Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap());
        let request_latency_us =
            parking_lot::Mutex::new(Histogram::<u64>::new_with_bounds(1, 60_000_000, 3).unwrap());
        // Track up to 128 MiB file index sizes (134,217,728 bytes) with 3 significant digits
        let file_index_size_bytes =
            parking_lot::Mutex::new(Histogram::<u64>::new_with_bounds(1, 134_217_728, 3).unwrap());

        Self {
            indexing_time_us,
            request_latency_us,
            file_index_size_bytes,
            try_send_succeeded: AtomicU64::new(0),
            try_send_failed: AtomicU64::new(0),
            dropped_age_timeout: AtomicU64::new(0),
            skipped_already_cached: AtomicU64::new(0),
            indexed_successfully: AtomicU64::new(0),
            indexing_failed: AtomicU64::new(0),
        }
    }

    /// Records the time spent indexing a single file.
    pub fn record_indexing_time(&self, duration: Duration) {
        let micros = duration.as_micros() as u64;
        if let Some(mut hist) = self.indexing_time_us.try_lock() {
            let _ = hist.record(micros);
        }
    }

    /// Records the time a request spent waiting in the queue before processing.
    fn record_request_latency(&self, duration: Duration) {
        let micros = duration.as_micros() as u64;
        if let Some(mut hist) = self.request_latency_us.try_lock() {
            let _ = hist.record(micros);
        }
    }

    /// Records the size of a file index in bytes.
    pub fn record_file_index_size(&self, size_bytes: usize) {
        if let Some(mut hist) = self.file_index_size_bytes.try_lock() {
            let _ = hist.record(size_bytes as u64);
        }
    }

    /// Creates a snapshot of current statistics with plain values.
    pub fn snapshot(&self) -> IndexingStats {
        let (indexing_time_p50_ms, indexing_time_p99_ms, indexing_time_max_ms) =
            if let Some(hist) = self.indexing_time_us.try_lock() {
                if hist.len() > 0 {
                    (
                        Some(hist.value_at_quantile(0.50) as f64 / 1000.0),
                        Some(hist.value_at_quantile(0.99) as f64 / 1000.0),
                        Some(hist.max() as f64 / 1000.0),
                    )
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            };

        let (request_latency_p50_ms, request_latency_p99_ms, request_latency_max_ms) =
            if let Some(hist) = self.request_latency_us.try_lock() {
                if hist.len() > 0 {
                    (
                        Some(hist.value_at_quantile(0.50) as f64 / 1000.0),
                        Some(hist.value_at_quantile(0.99) as f64 / 1000.0),
                        Some(hist.max() as f64 / 1000.0),
                    )
                } else {
                    (None, None, None)
                }
            } else {
                (None, None, None)
            };

        let (
            file_index_size_mean_bytes,
            file_index_size_p90_bytes,
            file_index_size_p95_bytes,
            file_index_size_p99_bytes,
            file_index_size_max_bytes,
        ) = if let Some(hist) = self.file_index_size_bytes.try_lock() {
            if hist.len() > 0 {
                (
                    Some(hist.mean()),
                    Some(hist.value_at_quantile(0.90) as f64),
                    Some(hist.value_at_quantile(0.95) as f64),
                    Some(hist.value_at_quantile(0.99) as f64),
                    Some(hist.max() as f64),
                )
            } else {
                (None, None, None, None, None)
            }
        } else {
            (None, None, None, None, None)
        };

        IndexingStats {
            try_send_succeeded: self.try_send_succeeded.load(Ordering::Relaxed),
            try_send_failed: self.try_send_failed.load(Ordering::Relaxed),
            dropped_age_timeout: self.dropped_age_timeout.load(Ordering::Relaxed),
            skipped_already_cached: self.skipped_already_cached.load(Ordering::Relaxed),
            indexed_successfully: self.indexed_successfully.load(Ordering::Relaxed),
            indexing_failed: self.indexing_failed.load(Ordering::Relaxed),
            indexing_time_p50_ms,
            indexing_time_p99_ms,
            indexing_time_max_ms,
            request_latency_p50_ms,
            request_latency_p99_ms,
            request_latency_max_ms,
            file_index_size_mean_bytes,
            file_index_size_p90_bytes,
            file_index_size_p95_bytes,
            file_index_size_p99_bytes,
            file_index_size_max_bytes,
            // Foyer stats will be added at IndexCache level
            foyer_memory_usage_bytes: 0,
            foyer_memory_capacity_bytes: 0,
            foyer_disk_write_bytes: 0,
            foyer_disk_read_bytes: 0,
            foyer_disk_write_ios: 0,
            foyer_disk_read_ios: 0,
        }
    }

    /// Prints all collected statistics to stdout.
    fn print_stats(&self) {
        let stats = self.snapshot();

        println!("\n=== Indexing Statistics ===");
        println!("\nCounters:");
        println!("  try_send succeeded:    {}", stats.try_send_succeeded);
        println!("  try_send failed:       {}", stats.try_send_failed);
        println!("  dropped (age timeout): {}", stats.dropped_age_timeout);
        println!("  skipped (cached):      {}", stats.skipped_already_cached);
        println!("  indexed successfully:  {}", stats.indexed_successfully);
        println!("  indexing failed:       {}", stats.indexing_failed);

        if stats.indexing_time_p50_ms.is_some() {
            println!("\nIndexing Time:");
            println!("  P50: {:.2}ms", stats.indexing_time_p50_ms.unwrap());
            println!("  P99: {:.2}ms", stats.indexing_time_p99_ms.unwrap());
            println!("  Max: {:.2}ms", stats.indexing_time_max_ms.unwrap());
        }

        if stats.request_latency_p50_ms.is_some() {
            println!("\nRequest Latency (queue wait time):");
            println!("  P50: {:.2}ms", stats.request_latency_p50_ms.unwrap());
            println!("  P99: {:.2}ms", stats.request_latency_p99_ms.unwrap());
            println!("  Max: {:.2}ms", stats.request_latency_max_ms.unwrap());
        }

        if stats.file_index_size_mean_bytes.is_some() {
            println!("\nFile Index Size:");
            println!(
                "  Mean: {:.2} KB",
                stats.file_index_size_mean_bytes.unwrap() / 1024.0
            );
            println!(
                "  P90:  {:.2} KB",
                stats.file_index_size_p90_bytes.unwrap() / 1024.0
            );
            println!(
                "  P95:  {:.2} KB",
                stats.file_index_size_p95_bytes.unwrap() / 1024.0
            );
            println!(
                "  P99:  {:.2} KB",
                stats.file_index_size_p99_bytes.unwrap() / 1024.0
            );
            println!(
                "  Max:  {:.2} KB",
                stats.file_index_size_max_bytes.unwrap() / 1024.0
            );
        }

        println!("\nFoyer Cache:");
        println!(
            "  Memory usage:     {:.2} MB / {:.2} MB",
            stats.foyer_memory_usage_bytes as f64 / (1024.0 * 1024.0),
            stats.foyer_memory_capacity_bytes as f64 / (1024.0 * 1024.0)
        );
        if stats.foyer_disk_write_bytes > 0 || stats.foyer_disk_read_bytes > 0 {
            println!(
                "  Disk write:       {:.2} MB ({} IOs)",
                stats.foyer_disk_write_bytes as f64 / (1024.0 * 1024.0),
                stats.foyer_disk_write_ios
            );
            println!(
                "  Disk read:        {:.2} MB ({} IOs)",
                stats.foyer_disk_read_bytes as f64 / (1024.0 * 1024.0),
                stats.foyer_disk_read_ios
            );
        } else {
            println!("  Disk I/O:         Statistics not tracked yet");
        }

        println!();
    }
}

/// A request to index a file with a specific bucket duration. The `instant` field
/// is used for age-based filtering by workers.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct IndexingRequest {
    pub fields: HashSet<String>,
    pub bucket_duration: u64,
    pub file: File,
    pub instant: Instant,
}

/// Cache for file indexes with a background worker pool for concurrent indexing.
pub struct IndexCache {
    pub file_indexes: foyer::HybridCache<File, FileIndex>,
    pub indexing_tx: SyncSender<IndexingRequest>,

    #[cfg(feature = "indexing-stats")]
    stats: Arc<IndexingMetrics>,
}

impl IndexCache {
    /// Creates a new IndexCache with hybrid memory + disk storage.
    ///
    /// # Arguments
    /// * `runtime_handle` - Tokio runtime handle for async operations
    /// * `cache_dir` - Directory path for disk cache storage
    /// * `memory_size` - Memory cache size in bytes
    /// * `disk_capacity` - Disk cache capacity in bytes
    ///
    /// # Returns
    /// Result containing the initialized IndexCache or an error
    pub async fn new(
        runtime_handle: tokio::runtime::Handle,
        cache_dir: impl AsRef<std::path::Path>,
        memory_size: usize,
        disk_capacity: u64,
    ) -> crate::index_state::error::Result<Self> {
        use foyer::{
            BlockEngineBuilder, Compression, DeviceBuilder, FsDeviceBuilder, HybridCacheBuilder,
        };

        let cache_dir = cache_dir.as_ref();

        // Create cache directory if it doesn't exist
        std::fs::create_dir_all(cache_dir)?;

        // Create filesystem device with specified capacity
        let device = FsDeviceBuilder::new(cache_dir)
            .with_capacity(disk_capacity.try_into().unwrap())
            .build()?;

        // Build hybrid cache with block-based storage

        // Custom weighter that returns the serialized size of FileIndex in bytes
        let weighter = |_key: &File, value: &FileIndex| -> usize {
            // Use bincode to estimate the serialized size
            bincode::serialize(value)
                .map(|serialized| serialized.len())
                .unwrap_or(1) // Fallback to weight=1 if serialization fails
        };

        let file_indexes = HybridCacheBuilder::new()
            .with_name("journal-index-cache")
            .with_policy(foyer::HybridCachePolicy::WriteOnInsertion)
            .memory(memory_size)
            .with_weighter(weighter)
            .with_shards(16)
            .storage()
            .with_compression(Compression::Zstd)
            .with_engine_config(BlockEngineBuilder::new(device).with_block_size(256 * 1024))
            .build()
            .await?;

        let (tx, rx) = sync_channel(100);

        #[cfg(feature = "indexing-stats")]
        let stats = Arc::new(IndexingMetrics::new());

        // Spawn 24 background indexing threads
        let rx = Arc::new(std::sync::Mutex::new(rx));
        for _ in 0..24 {
            let cache_clone = file_indexes.clone();
            let rx_clone = Arc::clone(&rx);
            let handle_clone = runtime_handle.clone();
            #[cfg(feature = "indexing-stats")]
            let stats_clone = Arc::clone(&stats);

            std::thread::spawn(move || {
                Self::indexing_worker(
                    rx_clone,
                    cache_clone,
                    handle_clone,
                    #[cfg(feature = "indexing-stats")]
                    stats_clone,
                );
            });
        }

        Ok(Self {
            file_indexes,
            indexing_tx: tx,
            #[cfg(feature = "indexing-stats")]
            stats,
        })
    }
}

impl IndexCache {
    /// Attempts to queue an indexing request. Returns `false` if the channel is full,
    /// in which case the request is dropped.
    pub fn try_send_request(&self, request: IndexingRequest) -> bool {
        match self.indexing_tx.try_send(request) {
            Ok(_) => {
                #[cfg(feature = "indexing-stats")]
                {
                    use std::sync::atomic::Ordering;
                    self.stats
                        .try_send_succeeded
                        .fetch_add(1, Ordering::Relaxed);
                }
                true
            }
            Err(_) => {
                #[cfg(feature = "indexing-stats")]
                {
                    use std::sync::atomic::Ordering;
                    self.stats.try_send_failed.fetch_add(1, Ordering::Relaxed);
                }
                false
            }
        }
    }

    /// Returns a snapshot of current indexing statistics.
    #[cfg(feature = "indexing-stats")]
    pub fn indexing_stats(&self) -> IndexingStats {
        let mut stats = self.stats.snapshot();

        // Add foyer cache statistics
        // usage/capacity return weighted sizes (bytes in our case, since we use a byte-based weighter)
        stats.foyer_memory_usage_bytes = self.file_indexes.memory().usage();
        stats.foyer_memory_capacity_bytes = self.file_indexes.memory().capacity();

        // Only populate disk stats if running in hybrid mode
        if self.file_indexes.is_hybrid() {
            let foyer_stats = self.file_indexes.statistics();
            stats.foyer_disk_write_bytes = foyer_stats.disk_write_bytes();
            stats.foyer_disk_read_bytes = foyer_stats.disk_read_bytes();
            stats.foyer_disk_write_ios = foyer_stats.disk_write_ios();
            stats.foyer_disk_read_ios = foyer_stats.disk_read_ios();
        }

        stats
    }

    /// Prints indexing statistics to stdout.
    #[cfg(feature = "indexing-stats")]
    pub fn print_indexing_stats(&self) {
        println!(
            "\n[DEBUG] Cache mode: {}",
            if self.file_indexes.is_hybrid() {
                "Hybrid (memory + disk)"
            } else {
                "Memory only"
            }
        );
        self.stats.print_stats();
    }

    /// Gracefully closes the cache, ensuring all pending writes are flushed to disk.
    /// Should be called during application shutdown.
    pub async fn close(&self) -> crate::index_state::error::Result<()> {
        self.file_indexes.close().await?;
        Ok(())
    }

    /// Updates partial responses with data from cached file indexes.
    pub async fn resolve_partial_responses(
        &self,
        partial_responses: &mut LruCache<BucketRequest, BucketPartialResponse>,
        pending_files: HashSet<File>,
    ) {
        for file in pending_files {
            if let Ok(Some(entry)) = self.file_indexes.get(&file).await {
                let file_index = entry.value();
                for (_bucket_request, partial_response) in partial_responses.iter_mut() {
                    partial_response.update(&file, file_index);
                }
            }
        }
    }

    fn indexing_worker(
        rx: Arc<std::sync::Mutex<Receiver<IndexingRequest>>>,
        cache: foyer::HybridCache<File, FileIndex>,
        runtime_handle: tokio::runtime::Handle,
        #[cfg(feature = "indexing-stats")] stats: Arc<IndexingMetrics>,
    ) {
        loop {
            let task = {
                let rx = rx.lock().unwrap();
                rx.recv()
            };

            let Ok(task) = task else {
                break;
            };

            // Record request latency (time from creation to now)
            #[cfg(feature = "indexing-stats")]
            stats.record_request_latency(task.instant.elapsed());

            // Drop requests older than 2 seconds
            if task.instant.elapsed() > Duration::from_secs(2) {
                #[cfg(feature = "indexing-stats")]
                stats.dropped_age_timeout.fetch_add(1, Ordering::Relaxed);
                continue;
            }

            // Skip indexing if cache already contains this file with sufficient granularity
            // (cached duration <= requested duration means cached is more granular or equal)
            if let Ok(Some(cached_entry)) = runtime_handle.block_on(cache.get(&task.file)) {
                if cached_entry.value().bucket_duration() <= task.bucket_duration {
                    #[cfg(feature = "indexing-stats")]
                    stats.skipped_already_cached.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                // Otherwise, fall through and re-index with finer granularity
            }

            let field_names: Vec<&[u8]> = task.fields.iter().map(|x| x.as_bytes()).collect();

            // Create the file index and measure indexing time
            #[cfg(feature = "indexing-stats")]
            let indexing_start = Instant::now();

            let file_index = FILE_INDEXER.with(|indexer| {
                let mut file_indexer = indexer.borrow_mut();
                let window_size = 32 * 1024 * 1024;
                let journal_file = JournalFile::<Mmap>::open(task.file.path(), window_size).ok()?;

                file_indexer
                    .index(&journal_file, None, &field_names, task.bucket_duration)
                    .ok()
            });

            #[cfg(feature = "indexing-stats")]
            stats.record_indexing_time(indexing_start.elapsed());

            // Update cache if indexing succeeded
            if let Some(file_index) = file_index {
                #[cfg(feature = "indexing-stats")]
                {
                    // Record file index size (approximate using bincode serialization)
                    if let Ok(serialized) = bincode::serialized_size(&file_index) {
                        stats.record_file_index_size(serialized as usize);
                    }
                    stats.indexed_successfully.fetch_add(1, Ordering::Relaxed);
                }

                cache.insert(task.file, file_index);
            } else {
                #[cfg(feature = "indexing-stats")]
                stats.indexing_failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}
