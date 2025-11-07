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

use super::hybrid_cache::CacheGetResult;
use super::{Facets, FileIndexCache, FileIndexKey, TimeRange};
use crate::histogram::request::BucketRequest;
use crate::histogram::response::BucketPartialResponse;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::HashSet;
use journal::index::FileIndexer;
use journal::repository::File;
use journal::{FieldName, JournalFile, file::Mmap};
use lru::LruCache;
use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::sync::{
    Arc, RwLock,
    mpsc::{Receiver, SyncSender, sync_channel},
};
use std::time::{Duration, Instant};

thread_local! {
    static FILE_INDEXER: RefCell<FileIndexer> = RefCell::new(FileIndexer::default());
}

/// A request to index a file with a specific bucket duration. The `instant` field
/// is used for age-based filtering by workers.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(crate) struct IndexingRequest {
    pub(crate) facets: Facets,
    pub(crate) bucket_duration: u32,
    pub(crate) file: File,
    pub(crate) instant: Instant,
}

/// Service for file indexing with a background worker pool for concurrent indexing.
pub struct IndexingService {
    file_index_cache: FileIndexCache,
    worker_queue: SyncSender<IndexingRequest>,
    /// LRU cache mapping File to its time range metadata.
    time_range_cache: std::sync::RwLock<LruCache<File, TimeRange>>,
}

impl IndexingService {
    /// Creates a new IndexingService with hybrid memory + disk storage.
    ///
    /// # Arguments
    /// * `runtime_handle` - Tokio runtime handle for async operations
    /// * `cache_dir` - Directory path for disk cache storage
    /// * `memory_capacity` - Memory cache capacity in items
    /// * `disk_capacity` - Disk cache capacity in bytes
    ///
    /// # Returns
    /// Result containing the initialized IndexingService or an error
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
            file_index_cache: file_indexes,
            worker_queue: tx,
            time_range_cache: RwLock::new(LruCache::new(NonZeroUsize::new(10000).unwrap())),
        })
    }
}

impl IndexingService {
    /// Attempts to queue an indexing request. Returns `false` if the channel is full,
    /// in which case the request is dropped.
    pub(crate) fn try_send_request(&self, request: IndexingRequest) -> bool {
        self.worker_queue.try_send(request).is_ok()
    }

    /// Retrieves the cached time range for a file.
    ///
    /// Returns Unknown if the file has not been indexed yet.
    #[allow(dead_code)] // Used by query pipeline
    pub(crate) fn get_time_range(&self, file: &File) -> TimeRange {
        self.time_range_cache
            .read()
            .ok()
            .and_then(|cache| cache.peek(file).copied())
            .unwrap_or(TimeRange::Unknown)
    }

    /// Stores the time range for a file in the cache.
    ///
    /// This should be called when a FileIndex is created or loaded.
    /// This is crate-internal.
    pub(crate) fn set_time_range(&self, file: &File, time_range: TimeRange) {
        if let Ok(mut cache) = self.time_range_cache.write() {
            cache.put(file.clone(), time_range);
        }
    }

    /// Gracefully closes the cache, ensuring all pending writes are flushed to disk.
    /// Should be called during application shutdown.
    pub async fn close(&self) -> crate::error::Result<()> {
        self.file_index_cache.close().await?;
        Ok(())
    }

    /// Resolves an index request by fetching file indexes from cache.
    ///
    /// Returns IndexProgress containing the indexed data and any files still pending.
    /// This is a hermetic method that knows nothing about buckets or histograms.
    pub(crate) async fn resolve_index_request(&self, request: &super::IndexRequest) -> super::IndexProgress {
        use std::time::Duration;
        use tokio::time::Instant;

        const TOTAL_TIMEOUT: Duration = Duration::from_millis(500);
        const PER_FILE_TIMEOUT: Duration = Duration::from_millis(100);

        let start = Instant::now();

        let mut progress = super::IndexProgress::new();
        progress.pending_files = request.files.clone();

        for file in &request.files {
            // Check if we've exceeded total timeout
            if start.elapsed() >= TOTAL_TIMEOUT {
                break;
            }

            // Calculate remaining time
            let remaining = TOTAL_TIMEOUT.saturating_sub(start.elapsed());
            let file_timeout = remaining.min(PER_FILE_TIMEOUT);

            // Create cache key with file and facets
            let cache_key = FileIndexKey::new(file.clone(), request.facets.clone());

            // Apply timeout to the `get` operation
            match self.file_index_cache.get(&cache_key, file_timeout).await {
                Ok(CacheGetResult::Hit(file_index)) => {
                    // Cache the time range from the FileIndex
                    let time_range = TimeRange::from_file_index(&file_index);
                    self.set_time_range(file, time_range);

                    // This file is no longer pending
                    progress.pending_files.remove(file);

                    // Track unindexed fields
                    for field in file_index.fields() {
                        if !file_index.is_indexed(field) {
                            if let Some(field_name) = journal::FieldName::new(field) {
                                progress.unindexed_fields.insert(field_name);
                            }
                        }
                    }

                    // Evaluate filter if needed
                    let filter_bitmap = if !request.filter.is_none() {
                        Some(request.filter.resolve(&file_index).evaluate())
                    } else {
                        None
                    };

                    // Count field=value pairs in this file
                    for (indexed_field, field_bitmap) in file_index.bitmaps() {
                        let unfiltered_count = file_index
                            .count_bitmap_entries_in_range(field_bitmap, request.start, request.end)
                            .unwrap_or(0);

                        let filtered_count = if let Some(filter_bitmap) = &filter_bitmap {
                            let filtered_bitmap = field_bitmap & filter_bitmap;
                            file_index
                                .count_bitmap_entries_in_range(&filtered_bitmap, request.start, request.end)
                                .unwrap_or(0)
                        } else {
                            unfiltered_count
                        };

                        // Update counts
                        if let Some(pair) = journal::FieldValuePair::parse(indexed_field) {
                            let counts = progress.fv_counts.entry(pair).or_insert((0, 0));
                            counts.0 += unfiltered_count;
                            counts.1 += filtered_count;
                        }
                    }
                }
                Ok(CacheGetResult::Miss) => {
                    // File not in cache, remains pending
                }
                Err(_e) => {
                    // Error fetching from cache, remains pending
                }
            }
        }

        progress
    }

    /// Updates partial responses with data from cached file indexes.
    ///
    /// Attempts to fetch file indexes from cache with timeouts, updating all
    /// partial responses with the retrieved data. Uses per-file and total timeouts
    /// to ensure responsiveness.
    ///
    /// NOTE: This method now uses resolve_index_request internally, which provides
    /// a hermetic indexing API. The bucket-specific logic remains here.
    pub(crate) async fn resolve_partial_responses(
        &self,
        facets: &Facets,
        partial_responses: &mut LruCache<BucketRequest, BucketPartialResponse>,
        pending_files: HashSet<File>,
    ) {
        // Process each partial response using the new hermetic API
        // Collect bucket requests AND their files to avoid borrow checker issues
        let bucket_data: Vec<_> = partial_responses
            .iter()
            .map(|(bucket_request, partial_response)| {
                (
                    bucket_request.clone(),
                    partial_response.request_metadata.files.clone(),
                )
            })
            .collect();

        for (bucket_request, bucket_files) in bucket_data {
            // Only process files that:
            // 1. Are needed for this bucket (in bucket_files)
            // 2. Haven't been indexed yet (in pending_files)
            let files_to_process: HashSet<File> = bucket_files
                .intersection(&pending_files)
                .cloned()
                .collect();

            // Skip if no files to process
            if files_to_process.is_empty() {
                continue;
            }

            // Create an IndexRequest from the bucket request
            let index_request = super::IndexRequest::new(
                bucket_request.start,
                bucket_request.end,
                facets.clone(),
                bucket_request.filter_expr.clone(),
                files_to_process,
            );

            // Use the hermetic resolve_index_request method
            let progress = self.resolve_index_request(&index_request).await;

            // Update the partial response with the progress
            if let Some(partial_response) = partial_responses.get_mut(&bucket_request) {
                // Remove files that are no longer pending
                partial_response.request_metadata.files.retain(|f| progress.pending_files.contains(f));

                // Merge field-value counts
                for (pair, (unfiltered, filtered)) in progress.fv_counts {
                    let counts = partial_response.fv_counts.entry(pair).or_insert((0, 0));
                    counts.0 += unfiltered;
                    counts.1 += filtered;
                }

                // Merge unindexed fields
                partial_response.unindexed_fields.extend(progress.unindexed_fields);
            }
        }
    }

    fn indexing_worker(
        rx: Arc<std::sync::Mutex<Receiver<IndexingRequest>>>,
        cache: foyer::HybridCache<super::FileIndexKey, journal::index::FileIndex>,
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
            let field_names: Vec<FieldName> = task.facets.iter().cloned().collect();

            // Create the file index

            let file_index = FILE_INDEXER.with(|indexer| {
                let mut file_indexer = indexer.borrow_mut();
                let window_size = 32 * 1024 * 1024;
                let journal_file = JournalFile::<Mmap>::open(&task.file, window_size).ok()?;

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
