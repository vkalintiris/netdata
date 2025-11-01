//! High-level file index cache with background indexing workers.
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

use super::FileIndexCache;
use super::hybrid_cache::CacheGetResult;
use crate::request::BucketRequest;
use crate::response::BucketPartialResponse;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::HashSet;
use journal::index::{FileIndex, FileIndexer};
use journal::repository::File;
use journal::{JournalFile, file::Mmap};
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

use crate::request::HistogramFacets;

/// A request to index a file with a specific bucket duration. The `instant` field
/// is used for age-based filtering by workers.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct IndexingRequest {
    pub facets: HistogramFacets,
    pub bucket_duration: u32,
    pub file: File,
    pub instant: Instant,
}

/// Cache for file indexes with a background worker pool for concurrent indexing.
pub struct IndexCache {
    file_indexes: FileIndexCache,
    indexing_tx: SyncSender<IndexingRequest>,
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
        memory_capacity: usize,
        disk_capacity: u64,
    ) -> crate::error::Result<Self> {
        use foyer::{
            BlockEngineBuilder, Compression, DeviceBuilder, FsDeviceBuilder, HybridCacheBuilder,
        };

        let cache_dir = cache_dir.as_ref();

        std::fs::create_dir_all(cache_dir)?;

        let device = FsDeviceBuilder::new(cache_dir)
            .with_capacity(disk_capacity.try_into().unwrap())
            .build()?;

        let foyer_cache = HybridCacheBuilder::new()
            .with_name("journal-index-cache")
            .with_policy(foyer::HybridCachePolicy::WriteOnEviction)
            .memory(memory_capacity)
            .with_shards(16)
            .storage()
            .with_compression(Compression::Zstd)
            .with_engine_config(BlockEngineBuilder::new(device).with_block_size(1024 * 1024))
            .build()
            .await?;

        let file_indexes = FileIndexCache::new(foyer_cache);

        let (tx, rx) = sync_channel(100);

        // Spawn 24 background indexing threads
        let rx = Arc::new(std::sync::Mutex::new(rx));
        for _ in 0..24 {
            let cache_clone = file_indexes.inner().clone();
            let rx_clone = Arc::clone(&rx);
            let handle_clone = runtime_handle.clone();

            std::thread::spawn(move || {
                Self::indexing_worker(rx_clone, cache_clone, handle_clone);
            });
        }

        Ok(Self {
            file_indexes,
            indexing_tx: tx,
        })
    }
}

impl IndexCache {
    /// Attempts to queue an indexing request. Returns `false` if the channel is full,
    /// in which case the request is dropped.
    pub fn try_send_request(&self, request: IndexingRequest) -> bool {
        self.indexing_tx.try_send(request).is_ok()
    }

    /// Gracefully closes the cache, ensuring all pending writes are flushed to disk.
    /// Should be called during application shutdown.
    pub async fn close(&self) -> crate::error::Result<()> {
        self.file_indexes.close().await?;
        Ok(())
    }

    /// Updates partial responses with data from cached file indexes.
    ///
    /// Attempts to fetch file indexes from cache with timeouts, updating all
    /// partial responses with the retrieved data. Uses per-file and total timeouts
    /// to ensure responsiveness.
    pub async fn resolve_partial_responses(
        &self,
        facets: &HistogramFacets,
        partial_responses: &mut LruCache<BucketRequest, BucketPartialResponse>,
        pending_files: HashSet<File>,
    ) {
        use std::time::Duration;
        use tokio::time::Instant;

        const TOTAL_TIMEOUT: Duration = Duration::from_millis(500);
        const PER_FILE_TIMEOUT: Duration = Duration::from_millis(100);

        let start = Instant::now();

        for file in pending_files {
            // Check if we've exceeded total timeout
            if start.elapsed() >= TOTAL_TIMEOUT {
                break;
            }

            // Calculate remaining time
            let remaining = TOTAL_TIMEOUT.saturating_sub(start.elapsed());
            let file_timeout = remaining.min(PER_FILE_TIMEOUT);

            // Create cache key with file and facets
            use super::FileIndexKey;
            let cache_key = FileIndexKey::new(file.clone(), facets.clone());

            // Apply timeout to the `obtain` operation
            match self
                .file_indexes
                .get_with_timeout(&cache_key, file_timeout)
                .await
            {
                Ok(CacheGetResult::Hit(file_index)) => {
                    use rayon::prelude::*;
                    let _count: usize = partial_responses
                        .iter_mut()
                        .par_bridge()
                        .map(|(_bucket_request, partial_response)| {
                            partial_response.update(&file, &file_index);
                            1
                        })
                        .sum();
                }
                Ok(CacheGetResult::Miss) => {
                    // File not in cache, will be indexed by workers
                }
                Err(_e) => {
                    // Error fetching from cache, skip this file
                }
            }
        }
    }

    fn indexing_worker(
        rx: Arc<std::sync::Mutex<Receiver<IndexingRequest>>>,
        cache: foyer::HybridCache<super::FileIndexKey, FileIndex>,
        runtime_handle: tokio::runtime::Handle,
    ) {
        loop {
            let task = {
                let rx = rx.lock().unwrap();
                rx.recv()
            };

            let Ok(task) = task else {
                break;
            };

            // Drop requests older than 2 seconds
            if task.instant.elapsed() > Duration::from_secs(2) {
                continue;
            }

            // Create cache key with file and facets
            use super::FileIndexKey;
            let cache_key = FileIndexKey::new(task.file.clone(), task.facets.clone());

            // Skip indexing if cache already contains this file with sufficient granularity
            // (cached duration <= requested duration means cached is more granular or equal)
            if let Ok(Some(cached_entry)) = runtime_handle.block_on(cache.get(&cache_key)) {
                if cached_entry.value().bucket_duration() <= task.bucket_duration {
                    continue;
                }
                // Otherwise, fall through and re-index with finer granularity
            }

            // Extract field names from facets
            let field_names: Vec<journal::FieldName> = task.facets.iter().cloned().collect();

            // Create the file index

            let file_index = FILE_INDEXER.with(|indexer| {
                let mut file_indexer = indexer.borrow_mut();
                let window_size = 32 * 1024 * 1024;
                let journal_file = JournalFile::<Mmap>::open(task.file.path(), window_size).ok()?;

                file_indexer
                    .index(&journal_file, None, &field_names, task.bucket_duration)
                    .ok()
            });

            // Update cache if indexing succeeded
            if let Some(file_index) = file_index {
                cache.insert(cache_key, file_index);
            }
        }
    }
}
