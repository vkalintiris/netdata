//! Journal file indexing infrastructure.
//!
//! This module provides the complete infrastructure for indexing journal files:
//! - Background indexing service with worker pool for cache warming
//! - Stream that orchestrates cache checks and inline computation
//! - Request/response types for indexing operations

use super::{CatalogError, File, FileIndexCache, FileIndexKey, FileIndexingMetrics, Result};
use async_stream::stream;
use futures::stream::Stream;
use journal::index::{FileIndex, FileIndexer};
use journal::{FieldName, JournalFile, file::Mmap};
use rt::ChartHandle;
use std::pin::Pin;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{error, info, warn};

// ============================================================================
// Helper Functions
// ============================================================================

/// Checks if a cached FileIndex is still fresh.
///
/// For files that were online (actively being written) when indexed, the cache
/// is considered stale after 1 second. For archived/offline files, the cache
/// is always fresh since they never change.
fn is_fresh(index: &FileIndex) -> bool {
    if index.was_online {
        // Active file: check if indexed < 1 second ago
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now - index.indexed_at < 1
    } else {
        // Archived/offline file: always fresh
        true
    }
}

/// Updates registry metadata with information from a file index.
///
/// Extracts time range and online status from the index and updates the registry
/// with appropriate TimeRange metadata (Active for online files, Bounded for archived).
/// Errors are silently ignored as registry updates are best-effort.
fn update_registry_from_index(registry: &crate::Registry, file: &File, index: &FileIndex) {
    let (start, end) = index.histogram().time_range();
    let time_range = if index.was_online {
        crate::TimeRange::Active {
            start,
            end,
            indexed_at: index.indexed_at,
        }
    } else {
        crate::TimeRange::Bounded {
            start,
            end,
            indexed_at: index.indexed_at,
        }
    };

    // Ignore errors - registry update is best-effort
    let _ = registry.update_time_range(file, time_range);
}

// ============================================================================
// Request/Response Types
// ============================================================================

/// Request to index a journal file with specific parameters.
#[derive(Debug, Clone)]
pub struct FileIndexRequest {
    /// The file and facets to index
    pub key: FileIndexKey,
    /// Field name to use for timestamps when indexing
    pub source_timestamp_field: journal::FieldName,
    /// Duration of histogram buckets in seconds
    pub bucket_duration: u32,
    /// When this request was created (for age-based filtering)
    pub created_at: std::time::Instant,
}

impl FileIndexRequest {
    /// Creates a new file index request.
    pub fn new(
        key: FileIndexKey,
        source_timestamp_field: journal::FieldName,
        bucket_duration: u32,
    ) -> Self {
        Self {
            key,
            source_timestamp_field,
            bucket_duration,
            created_at: std::time::Instant::now(),
        }
    }
}

/// Response from indexing a journal file.
#[derive(Debug)]
pub struct FileIndexResponse {
    /// The file and facets that were indexed
    pub key: FileIndexKey,
    /// The result of the indexing operation
    pub result: Result<journal::index::FileIndex>,
}

impl FileIndexResponse {
    /// Creates a new file index response.
    pub fn new(key: FileIndexKey, result: Result<journal::index::FileIndex>) -> Self {
        Self { key, result }
    }

    /// Returns true if the indexing was successful.
    pub fn is_ok(&self) -> bool {
        self.result.is_ok()
    }

    /// Returns true if the indexing failed.
    pub fn is_err(&self) -> bool {
        self.result.is_err()
    }
}

// ============================================================================
// Background Indexing Service
// ============================================================================

/// Service for background file indexing with worker pool.
///
/// This service manages a pool of worker threads that index journal files in the background,
/// storing results in the cache. It uses a fire-and-forget API - callers queue requests
/// and the cache is populated asynchronously.
#[derive(Clone)]
pub struct IndexingService {
    request_tx: SyncSender<FileIndexRequest>,
    metrics: ChartHandle<FileIndexingMetrics>,
}

impl IndexingService {
    /// Creates a new IndexingService with the specified configuration.
    ///
    /// # Arguments
    /// * `cache` - The file index cache to store indexes
    /// * `registry` - Registry to update with file metadata after indexing
    /// * `num_workers` - Number of worker threads (typically 24)
    /// * `queue_capacity` - Bounded channel capacity (typically 100)
    /// * `metrics` - Chart handle for tracking indexing metrics
    ///
    /// # Returns
    /// The initialized IndexingService
    pub fn new(
        cache: FileIndexCache,
        registry: crate::Registry,
        num_workers: usize,
        queue_capacity: usize,
        metrics: ChartHandle<FileIndexingMetrics>,
    ) -> Self {
        let (request_tx, request_rx) = sync_channel(queue_capacity);
        let request_rx = Arc::new(Mutex::new(request_rx));

        // Spawn worker threads
        for _ in 0..num_workers {
            let cache = cache.clone();
            let registry = registry.clone();
            let request_rx = Arc::clone(&request_rx);
            let metrics = metrics.clone();

            std::thread::spawn(move || {
                Self::worker_loop(cache, registry, request_rx, metrics);
            });
        }

        Self {
            request_tx,
            metrics,
        }
    }

    /// Queues a file for background indexing (fire-and-forget).
    ///
    /// If the queue is full, the request is silently dropped.
    ///
    /// # Arguments
    /// * `request` - The file index request containing file, facets, timestamp field, and bucket duration
    pub fn queue_indexing(&self, request: FileIndexRequest) {
        let _ = self.request_tx.try_send(request);
    }

    /// Gets the metrics handle.
    pub fn metrics(&self) -> ChartHandle<FileIndexingMetrics> {
        self.metrics.clone()
    }

    /// Worker loop that processes indexing requests.
    fn worker_loop(
        cache: FileIndexCache,
        registry: crate::Registry,
        request_rx: Arc<Mutex<Receiver<FileIndexRequest>>>,
        metrics: ChartHandle<FileIndexingMetrics>,
    ) {
        loop {
            let request = {
                let rx = request_rx.lock().unwrap();
                rx.recv()
            };

            let Ok(request) = request else {
                // Channel closed, exit worker
                break;
            };

            // Age-based filtering: drop requests older than 2 seconds
            if request.created_at.elapsed() > std::time::Duration::from_secs(2) {
                continue;
            }

            // Compute the index
            let result = Self::compute_file_index(
                &request.key.file,
                request.key.facets.as_slice(),
                &request.source_timestamp_field,
                request.bucket_duration,
            );

            // Store in cache and update registry if successful
            if let Ok(index) = result {
                // Track metric: one index computed
                metrics.update(|m| {
                    m.computed += 1;
                });

                // Update registry metadata
                update_registry_from_index(&registry, &request.key.file, &index);

                // Store in cache
                let _ = cache.insert(request.key, index);
            }
        }
    }

    /// Computes a file index by reading and indexing a journal file.
    pub fn compute_file_index(
        file: &File,
        facets: &[FieldName],
        source_timestamp_field: &FieldName,
        bucket_duration: u32,
    ) -> Result<FileIndex> {
        info!("Computing file index for {}", file.path());

        let mut file_indexer = FileIndexer::default();

        // Open the journal file with 32MB window size (matching journal-query)
        let window_size = 32 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(file, window_size).map_err(|e| {
            CatalogError::Io(std::io::Error::other(format!(
                "Failed to open journal file: {}",
                e
            )))
        })?;

        // Index the file
        let index = file_indexer
            .index(
                &journal_file,
                Some(source_timestamp_field),
                facets,
                bucket_duration,
            )
            .map_err(|e| {
                CatalogError::Io(std::io::Error::other(format!(
                    "Failed to index journal file: {}",
                    e
                )))
            })?;

        Ok(index)
    }
}

// ============================================================================
// File Index Iterator
// ============================================================================

/// Errors that can occur during iteration.
#[derive(Debug, Error)]
pub enum IteratorError {
    /// Time budget exceeded
    #[error("Iterator time budget exceeded")]
    TimeBudgetExceeded,
}

/// Stream that returns file indexes by checking cache first, then computing inline.
///
/// On creation, checks cache for each key and queues background indexing for cache misses.
/// On each poll, tries cache first (async), then computes inline if still missing.
///
/// Returns `Result<FileIndexResponse, IteratorError>` items. The outer Result handles
/// stream-level errors (time budget), while FileIndexResponse.result
/// contains indexing errors for individual files.
pub struct FileIndexStream {
    inner:
        Pin<Box<dyn Stream<Item = std::result::Result<FileIndexResponse, IteratorError>> + Send>>,
    failed_keys: Arc<Mutex<Vec<FileIndexKey>>>,
}

impl FileIndexStream {
    /// Creates a new stream that fetches or computes file indexes.
    ///
    /// On creation, checks cache for each key and queues cache misses for background indexing.
    ///
    /// # Arguments
    /// * `indexing_service` - The indexing service to use for background cache warming
    /// * `cache` - The file index cache to check
    /// * `registry` - Registry to update with file metadata on cache miss
    /// * `keys` - Vector of (file, facets) pairs to fetch/compute indexes for
    /// * `source_timestamp_field` - Field name to use for timestamps when indexing
    /// * `bucket_duration` - Duration of histogram buckets in seconds
    /// * `time_budget` - Maximum total time the stream can spend processing
    pub fn new(
        indexing_service: IndexingService,
        cache: FileIndexCache,
        registry: crate::Registry,
        keys: Vec<FileIndexKey>,
        source_timestamp_field: FieldName,
        bucket_duration: u32,
        time_budget: Duration,
    ) -> Self {
        // Queue cache misses for background indexing
        for key in &keys {
            if let Ok(false) = cache.contains(key) {
                let request = FileIndexRequest::new(
                    key.clone(),
                    source_timestamp_field.clone(),
                    bucket_duration,
                );
                indexing_service.queue_indexing(request);
            }
        }

        let failed_keys = Arc::new(Mutex::new(Vec::new()));
        let failed_keys_clone = failed_keys.clone();
        let metrics = indexing_service.metrics();

        let inner = stream! {
            let mut total_time = Duration::ZERO;

            for (index, key) in keys.iter().enumerate() {
                // Check time budget before processing
                if total_time >= time_budget {
                    // Add all remaining unprocessed keys to failed_keys
                    let remaining = keys[index..].to_vec();
                    failed_keys_clone.lock().unwrap().extend(remaining);
                    yield Err(IteratorError::TimeBudgetExceeded);
                    break;
                }

                let start = Instant::now();

                // Try cache first
                let result = match cache.get(key).await {
                    Ok(Some(cached_index))
                        if is_fresh(&cached_index)
                        && cached_index.bucket_duration() <= bucket_duration
                        && bucket_duration % cached_index.bucket_duration() == 0 =>
                    {
                        // Cache hit with fresh data and compatible granularity (bucket boundaries align)
                        // Track metric: one index retrieved from cache
                        metrics.update(|m| {
                            m.cached += 1;
                        });
                        Ok(cached_index)
                    }
                    _ => {
                        // Cache miss or incompatible granularity - compute inline
                        tracing::info!("Computing file index for {}", key.file.path());
                        match IndexingService::compute_file_index(
                            &key.file,
                            key.facets.as_slice(),
                            &source_timestamp_field,
                            bucket_duration,
                        ) {
                            Ok(index) => {
                                // Track metric: one index computed
                                metrics.update(|m| {
                                    m.computed += 1;
                                });

                                // Update registry metadata on cache miss
                                update_registry_from_index(&registry, &key.file, &index);

                                // Insert into cache for future use
                                if let Err(e) = cache.insert(key.clone(), index.clone()) {
                                    warn!(
                                        "Failed to insert index into cache for {:?}: {}",
                                        key.file.path(),
                                        e
                                    );
                                }
                                Ok(index)
                            }
                            Err(e) => Err(e),
                        }
                    }
                };

                // Track failures
                if result.is_err() {
                    failed_keys_clone.lock().unwrap().push(key.clone());
                }

                // Update cumulative time spent
                total_time += start.elapsed();

                yield Ok(FileIndexResponse::new(key.clone(), result));
            }
        };

        Self {
            inner: Box::pin(inner),
            failed_keys,
        }
    }

    /// Returns the keys that failed to index.
    ///
    /// This can be called during or after streaming to retrieve the list
    /// of files that couldn't be indexed so far, enabling selective retries.
    pub fn remaining(&self) -> Vec<FileIndexKey> {
        self.failed_keys.lock().unwrap().clone()
    }

    /// Consumes the stream and collects all successfully indexed files.
    ///
    /// This is a convenience method for consuming the entire stream and
    /// collecting all files that were successfully indexed. Files that fail
    /// to index are silently skipped.
    pub async fn collect_indexes(mut self) -> Result<Vec<journal::index::FileIndex>> {
        use futures::stream::StreamExt;

        let mut results = Vec::new();

        while let Some(result) = self.next().await {
            match result {
                Ok(response) => {
                    if let Ok(index) = response.result {
                        results.push(index);
                    }
                }
                Err(e) => {
                    warn!("Streaming index collection timed out: {}", e);
                    break;
                }
            }
        }

        Ok(results)
    }
}

impl Stream for FileIndexStream {
    type Item = std::result::Result<FileIndexResponse, IteratorError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}
