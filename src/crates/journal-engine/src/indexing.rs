//! Journal file indexing infrastructure.
//!
//! This module provides infrastructure for indexing journal files:
//! - Batch parallel indexing with time budget enforcement
//! - Cache builder for file indexes

use crate::{
    cache::{FileIndexCache, FileIndexKey},
    error::{EngineError, Result},
    query_time_range::QueryTimeRange,
};
use foyer::{Event, EventListener};
use journal_index::{FileIndex, FileIndexer, IndexingLimits};
use journal_registry::Registry;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{error, trace};

// ============================================================================
// Cache Event Counters
// ============================================================================

/// Snapshot of cache event counts at a point in time.
#[derive(Debug, Clone, Copy, Default)]
pub struct CacheEventSnapshot {
    pub evictions: u64,
    pub replacements: u64,
    pub removals: u64,
    pub clears: u64,
}

/// Thread-safe counters for foyer cache eviction events.
///
/// Attach an `Arc<CacheEventCounters>` to the cache via
/// `FileIndexCacheBuilder::with_event_listener` and read the accumulated
/// values with [`get()`](Self::get).
#[derive(Debug, Default)]
pub struct CacheEventCounters {
    evictions: AtomicU64,
    replacements: AtomicU64,
    removals: AtomicU64,
    clears: AtomicU64,
}

impl CacheEventCounters {
    /// Returns a plain snapshot of all counters.
    pub fn get(&self) -> CacheEventSnapshot {
        CacheEventSnapshot {
            evictions: self.evictions.load(Ordering::Relaxed),
            replacements: self.replacements.load(Ordering::Relaxed),
            removals: self.removals.load(Ordering::Relaxed),
            clears: self.clears.load(Ordering::Relaxed),
        }
    }
}

impl EventListener for CacheEventCounters {
    type Key = FileIndexKey;
    type Value = FileIndex;

    fn on_leave(&self, reason: Event, _key: &Self::Key, _value: &Self::Value)
    where
        Self::Key: foyer::Key,
        Self::Value: foyer::Value,
    {
        match reason {
            Event::Evict => self.evictions.fetch_add(1, Ordering::Relaxed),
            Event::Replace => self.replacements.fetch_add(1, Ordering::Relaxed),
            Event::Remove => self.removals.fetch_add(1, Ordering::Relaxed),
            Event::Clear => self.clears.fetch_add(1, Ordering::Relaxed),
        };
    }
}

// ============================================================================
// Indexing Counters
// ============================================================================

/// Snapshot of indexing statistics at a point in time.
#[derive(Debug, Clone, Copy, Default)]
pub struct IndexingSnapshot {
    /// Files served directly from cache (fresh + compatible bucket).
    pub cache_hits: u64,
    /// Files not found in cache.
    pub cache_misses: u64,
    /// Cached entries that were stale and needed recomputation.
    pub stale: u64,
    /// Cached entries with incompatible bucket duration.
    pub incompatible_bucket: u64,
    /// Files successfully indexed (new computation).
    pub computed: u64,
    /// File indexing attempts that failed.
    pub errors: u64,
}

/// Thread-safe counters for file indexing operations.
///
/// Pass an `Arc<IndexingCounters>` to [`batch_compute_file_indexes`] and read
/// the accumulated values with [`get()`](Self::get).
#[derive(Debug, Default)]
pub struct IndexingCounters {
    cache_hits: AtomicU64,
    cache_misses: AtomicU64,
    stale: AtomicU64,
    incompatible_bucket: AtomicU64,
    computed: AtomicU64,
    errors: AtomicU64,
}

impl IndexingCounters {
    /// Returns a plain snapshot of all counters.
    pub fn get(&self) -> IndexingSnapshot {
        IndexingSnapshot {
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
            stale: self.stale.load(Ordering::Relaxed),
            incompatible_bucket: self.incompatible_bucket.load(Ordering::Relaxed),
            computed: self.computed.load(Ordering::Relaxed),
            errors: self.errors.load(Ordering::Relaxed),
        }
    }
}

// ============================================================================
// File Index Cache Builder
// ============================================================================

/// Builder for constructing a FileIndexCache with custom configuration.
pub struct FileIndexCacheBuilder {
    cache_path: Option<std::path::PathBuf>,
    memory_capacity: Option<usize>,
    disk_capacity: Option<usize>,
    block_size: Option<usize>,
    event_listener: Option<Arc<CacheEventCounters>>,
}

impl FileIndexCacheBuilder {
    /// Creates a new builder with no configuration.
    ///
    /// All options use defaults if not explicitly set:
    /// - Cache path: temp directory + "journal-engine-cache"
    /// - Memory capacity: 128 entries
    /// - Disk capacity: 16 MB
    /// - Block size: 4 MB
    pub fn new() -> Self {
        Self {
            cache_path: None,
            memory_capacity: None,
            disk_capacity: None,
            block_size: None,
            event_listener: None,
        }
    }

    /// Sets the cache directory path.
    pub fn with_cache_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.cache_path = Some(path.into());
        self
    }

    /// Sets the memory capacity (number of items to keep in memory).
    pub fn with_memory_capacity(mut self, capacity: usize) -> Self {
        self.memory_capacity = Some(capacity);
        self
    }

    /// Sets the disk capacity in bytes.
    pub fn with_disk_capacity(mut self, capacity: usize) -> Self {
        self.disk_capacity = Some(capacity);
        self
    }

    /// Sets the block size in bytes.
    pub fn with_block_size(mut self, size: usize) -> Self {
        self.block_size = Some(size);
        self
    }

    /// Attaches a cache event listener that counts evictions, replacements,
    /// removals and clears.
    pub fn with_event_listener(mut self, listener: Arc<CacheEventCounters>) -> Self {
        self.event_listener = Some(listener);
        self
    }

    /// Builds the FileIndexCache with the configured settings.
    pub async fn build(self) -> Result<FileIndexCache> {
        use foyer::{
            BlockEngineBuilder, DeviceBuilder, FsDeviceBuilder, HybridCacheBuilder,
            IoEngineBuilder, PsyncIoEngineBuilder,
        };

        // Compute defaults
        let cache_path = self
            .cache_path
            .unwrap_or_else(|| std::env::temp_dir().join("journal-engine-cache"));
        let memory_capacity = self.memory_capacity.unwrap_or(128);
        let disk_capacity = self.disk_capacity.unwrap_or(16 * 1024 * 1024);
        let block_size = self.block_size.unwrap_or(4 * 1024 * 1024);

        // Ensure cache directory exists
        std::fs::create_dir_all(&cache_path).map_err(|e| {
            EngineError::Io(std::io::Error::other(format!(
                "Failed to create cache directory: {}",
                e
            )))
        })?;

        // Build Foyer hybrid cache
        let mut builder = HybridCacheBuilder::new()
            .with_name("file-index-cache")
            .with_policy(foyer::HybridCachePolicy::WriteOnInsertion);

        if let Some(listener) = self.event_listener {
            builder = builder.with_event_listener(listener);
        }

        let cache = builder
            .memory(memory_capacity)
            .with_shards(4)
            .storage()
            .with_io_engine(PsyncIoEngineBuilder::new().build().await?)
            .with_engine_config(
                BlockEngineBuilder::new(
                    FsDeviceBuilder::new(&cache_path)
                        .with_capacity(disk_capacity)
                        .build()?,
                )
                .with_block_size(block_size),
            )
            .build()
            .await?;

        Ok(cache)
    }
}

impl Default for FileIndexCacheBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Batch Processing
// ============================================================================

/// Batch computes file indexes in parallel using rayon, with cache checking and time budget enforcement.
///
/// This function:
/// 1. Checks cache for all keys upfront
/// 2. Identifies cache misses
/// 3. Uses tokio::task to compute missing indexes in parallel
/// 4. Inserts newly computed indexes into cache
/// 5. Returns all results (cached + newly computed)
///
/// # Arguments
/// * `cache` - The file index cache
/// * `registry` - Registry to update with file metadata
/// * `keys` - Vector of (file, facets, source_timestamp_field) to fetch/compute indexes for
/// * `time_range` - Query time range for bucket duration calculation
/// * `cancellation` - Token to signal cancellation from the caller
/// * `indexing_limits` - Configuration limits for indexing (cardinality, payload size)
/// * `progress_counter` - Optional atomic counter incremented after each file is indexed
/// * `indexing_counters` - Optional shared counters for cache hit/miss/computation statistics
///
/// # Returns
/// Vector of responses for each key. Successful responses contain the file index.
/// If cancelled, returns Cancelled error.
pub async fn batch_compute_file_indexes(
    cache: &FileIndexCache,
    registry: &Registry,
    keys: Vec<FileIndexKey>,
    time_range: &QueryTimeRange,
    cancellation: CancellationToken,
    indexing_limits: IndexingLimits,
    progress_counter: Option<Arc<AtomicUsize>>,
    indexing_counters: Option<Arc<IndexingCounters>>,
) -> Result<Vec<(FileIndexKey, FileIndex)>> {
    let bucket_duration = time_range.bucket_duration_seconds();
    // Phase 1: Batch check cache for all keys upfront
    let cache_lookup_futures = keys.iter().map(|key| {
        let key_clone = key.clone();
        async move {
            let cached = cache
                .get(&key_clone)
                .await
                .map(|entry| entry.map(|e| e.value().clone()))
                .map_err(|e| e.into());
            (key_clone, cached)
        }
    });

    let cache_lookup_results: Vec<(FileIndexKey, Result<Option<FileIndex>>)> = tokio::select! {
        results = futures::future::join_all(cache_lookup_futures) => results,
        _ = cancellation.cancelled() => return Err(EngineError::Cancelled),
    };

    // Phase 2: Separate cache hits from misses, check freshness and compatibility
    let mut responses = Vec::with_capacity(keys.len());
    let mut keys_to_compute = Vec::new();
    let mut cache_hits = 0;
    let mut cache_misses = 0;
    let mut stale_entries = 0;
    let mut incompatible_bucket = 0;

    for (key, cache_lookup_result) in cache_lookup_results {
        match cache_lookup_result {
            Ok(Some(file_index)) => {
                let fresh = file_index.is_fresh();
                let bucket_ok = file_index.bucket_duration() <= bucket_duration
                    && bucket_duration.is_multiple_of(file_index.bucket_duration());

                if fresh && bucket_ok {
                    // Cache hit with fresh data and compatible granularity
                    cache_hits += 1;
                    responses.push((key, file_index));
                } else {
                    if !fresh {
                        stale_entries += 1;
                    }
                    if !bucket_ok {
                        incompatible_bucket += 1;
                    }
                    keys_to_compute.push(key);
                }
            }
            Ok(None) => {
                // Cache miss - need to compute
                cache_misses += 1;
                keys_to_compute.push(key);
            }
            Err(e) => {
                error!("cached file index lookup error {}", e);
            }
        }
    }

    if cancellation.is_cancelled() {
        return Err(EngineError::Cancelled);
    }

    trace!(
        "phase 2 summary: hits={}, misses={}, stale={}, incompatible_bucket={}",
        cache_hits, cache_misses, stale_entries, incompatible_bucket
    );

    if let Some(ref counters) = indexing_counters {
        counters.cache_hits.fetch_add(cache_hits, Ordering::Relaxed);
        counters.cache_misses.fetch_add(cache_misses, Ordering::Relaxed);
        counters.stale.fetch_add(stale_entries, Ordering::Relaxed);
        counters.incompatible_bucket.fetch_add(incompatible_bucket, Ordering::Relaxed);
    }

    // Phase 3: Spawn single blocking task with rayon for parallel computation
    //
    // The cancellation token is cloned into the blocking task so that cancellation
    // is visible to the per-file check.
    let cancellation_for_blocking = cancellation.clone();

    let compute_task = tokio::task::spawn_blocking(move || {
        use rayon::prelude::*;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        let cancelled = Arc::new(AtomicBool::new(false));

        keys_to_compute
            .into_par_iter()
            .map(|key| {
                // Check cancellation before processing
                if cancellation_for_blocking.is_cancelled() || cancelled.load(Ordering::Relaxed) {
                    cancelled.store(true, Ordering::Relaxed);
                    return (key, Err(EngineError::Cancelled));
                }

                let mut file_indexer = FileIndexer::new(indexing_limits);
                let result = file_indexer
                    .index(
                        &key.file,
                        key.source_timestamp_field.as_ref(),
                        key.facets.as_slice(),
                        bucket_duration,
                    )
                    .map_err(|e| e.into());

                if result.is_ok() {
                    if let Some(ref counter) = progress_counter {
                        counter.fetch_add(1, Ordering::Relaxed);
                    }
                }

                (key, result)
            })
            .collect::<Vec<(FileIndexKey, Result<FileIndex>)>>()
    });

    let computed_results = tokio::select! {
        result = compute_task => {
            match result {
                Ok(results) => results,
                Err(e) => {
                    return Err(EngineError::Io(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        format!("Blocking task panicked: {}", e),
                    )));
                }
            }
        }
        _ = cancellation.cancelled() => {
            return Err(EngineError::Cancelled);
        }
    };

    // Phase 4: Update registry and cache, then collect responses
    let mut computed = 0u64;
    let mut errors = 0u64;
    for (key, response) in computed_results {
        match response {
            Ok(index) => {
                // Update registry and cache on success
                registry.update_time_range(
                    &key.file,
                    index.start_time(),
                    index.end_time(),
                    index.indexed_at(),
                    index.online(),
                );

                cache.insert(key.clone(), index.clone());
                responses.push((key, index));
                computed += 1;
            }
            Err(e) => {
                error!(
                    "file index computation failed for file={}: {}",
                    key.file.path(),
                    e
                );
                errors += 1;
            }
        }
    }

    if let Some(ref counters) = indexing_counters {
        counters.computed.fetch_add(computed, Ordering::Relaxed);
        counters.errors.fetch_add(errors, Ordering::Relaxed);
    }

    Ok(responses)
}
