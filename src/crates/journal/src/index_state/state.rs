use crate::collections::{HashMap, HashSet};
use crate::index_state::cache::{IndexCache, IndexingRequest};
use crate::index_state::error::Result;
use crate::index_state::request::{BucketRequest, HistogramRequest, RequestMetadata};
use crate::index_state::response::{
    BucketCompleteResponse, BucketPartialResponse, BucketResponse, HistogramResult,
};
use crate::registry::Registry;
use crate::repository::File;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Instant;

#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct AppState {
    pub registry: Registry,

    pub indexed_fields: HashSet<String>,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub cache: IndexCache,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub partial_responses: LruCache<BucketRequest, BucketPartialResponse>,
    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub complete_responses: LruCache<BucketRequest, BucketCompleteResponse>,
}

impl AppState {
    /// Creates a new AppState with default cache configuration.
    ///
    /// Uses sensible defaults:
    /// - Cache directory: `/tmp/journal-index-cache`
    /// - Memory size: 1GB
    /// - Disk capacity: 10GB
    pub async fn new(
        path: &str,
        indexed_fields: std::collections::HashSet<String>,
        runtime_handle: tokio::runtime::Handle,
    ) -> Result<Self> {
        Self::new_with_cache_config(
            path,
            indexed_fields,
            runtime_handle,
            "/home/vk/repos/tmp/foyer-storage",
            4 * 1024 * 1024, // 1GB memory
            16 * 1024 * 1024,
        )
        .await
    }

    /// Creates a new AppState with custom cache configuration.
    ///
    /// # Arguments
    /// * `path` - Journal directory path to watch
    /// * `indexed_fields` - Fields to index
    /// * `runtime_handle` - Tokio runtime handle for async operations
    /// * `cache_dir` - Directory for disk cache storage
    /// * `memory_size` - Memory cache size in bytes
    /// * `disk_capacity` - Disk cache capacity in bytes
    pub async fn new_with_cache_config(
        path: &str,
        indexed_fields: std::collections::HashSet<String>,
        runtime_handle: tokio::runtime::Handle,
        cache_dir: impl AsRef<std::path::Path>,
        memory_size: usize,
        disk_capacity: u64,
    ) -> Result<Self> {
        let mut registry = Registry::new()?;
        registry.watch_directory(path)?;

        let cache_capacity = NonZeroUsize::new(1000).unwrap();

        Ok(Self {
            registry,
            indexed_fields: indexed_fields.into_iter().collect(),
            cache: IndexCache::new(runtime_handle, cache_dir, memory_size, disk_capacity).await?,
            partial_responses: LruCache::new(cache_capacity),
            complete_responses: LruCache::new(cache_capacity),
        })
    }

    // Creates indexing requests after deduplicating the files referenced
    // by the partial responses of our bucket requests.
    fn collect_partial_requests_files(&self, bucket_requests: &[BucketRequest]) -> HashSet<File> {
        if bucket_requests.is_empty() {
            return HashSet::default();
        }

        let mut files = HashSet::default();

        for bucket_request in bucket_requests.iter() {
            if self.complete_responses.peek(bucket_request).is_some() {
                continue;
            }

            let Some(partial_response) = self.partial_responses.peek(bucket_request) else {
                eprintln!(
                    "Missing partial response for bucket request: {:?}",
                    bucket_request
                );

                continue;
            };

            for file in partial_response.files() {
                files.insert(file.clone());
            }
        }

        files
    }

    pub fn send_indexing_requests(&self, pending_files: &HashSet<File>, bucket_duration: u64) {
        for file in pending_files {
            let indexing_request = IndexingRequest {
                fields: self.indexed_fields.clone(),
                bucket_duration,
                file: file.clone(),
                instant: Instant::now(),
            };

            // Use try_send to avoid blocking if queue is full
            // Ignore full queue errors - newer requests will arrive soon
            let _ = self.cache.try_send_request(indexing_request);
        }
    }

    pub async fn process_histogram_request(&mut self, request: &HistogramRequest) {
        // Create any partial responses we don't already have
        let bucket_requests = request.bucket_requests();
        assert!(!bucket_requests.is_empty());
        self.create_partial_responses(&bucket_requests);

        // Figure out the files we will need to lookup for partial requests
        let pending_files = self.collect_partial_requests_files(&bucket_requests);

        // Send indexing requests
        let bucket_duration = bucket_requests.first().unwrap().duration();
        self.send_indexing_requests(&pending_files, bucket_duration);

        // Progress partial responses
        self.cache
            .resolve_partial_responses(&mut self.partial_responses, pending_files)
            .await;

        // Promote those that have been completed from partial to complete
        // responses
        self.promote_partial_responses();
    }

    pub async fn get_histogram(&mut self, request: HistogramRequest) -> HistogramResult {
        // Process the histogram request to ensure buckets are computed/in-progress
        self.process_histogram_request(&request).await;

        // Generate the bucket requests for this histogram
        let bucket_requests = request.bucket_requests();

        // Collect the responses for each bucket
        let mut buckets = Vec::with_capacity(bucket_requests.len());
        for bucket_request in bucket_requests {
            let response = if let Some(complete) = self.complete_responses.get_mut(&bucket_request)
            {
                BucketResponse::Complete(complete.clone())
            } else if let Some(partial) = self.partial_responses.get_mut(&bucket_request) {
                BucketResponse::Partial(partial.clone())
            } else {
                // This shouldn't happen after process_histogram_request, but handle it gracefully
                continue;
            };

            buckets.push((bucket_request, response));
        }

        HistogramResult { buckets }
    }

    /// Creates responses for bucket requests that we don't have in our caches
    fn create_partial_responses(&mut self, bucket_requests: &[BucketRequest]) {
        // NOTE: we use `get()`, instead of `peek()`, when looking up responses
        // in the LRU caches, to promote them to the head of the LRU list.
        // This ensures that any newly-created responses will not evict any
        // responses we've queried.
        for bucket_request in bucket_requests {
            // Ignore if we have the request in the cache of complete responses
            if self.complete_responses.get(bucket_request).is_some() {
                continue;
            }

            // Ignore if we have the request in the cache of partial responses
            if self.partial_responses.get(bucket_request).is_some() {
                continue;
            }

            // We do not have any partial or complete response for this bucket
            // request. Build a new partial response with the journal files
            // that it needs to query.

            let request_metadata = self.create_request_metadata(bucket_request.clone());

            let partial_response = BucketPartialResponse {
                request_metadata,
                indexed_fields: HashMap::default(),
                unindexed_fields: HashSet::default(),
            };

            self.partial_responses
                .put(bucket_request.clone(), partial_response);
        }
    }

    /// Creates the request metadata, ie. the bucket request itself, along
    /// with the files it needs.
    fn create_request_metadata(&self, bucket_request: BucketRequest) -> RequestMetadata {
        let files = self
            .registry
            .find_files_in_range(bucket_request.start, bucket_request.end);

        RequestMetadata {
            request: bucket_request,
            files,
        }
    }

    /// Promotes responses from the partial cache to the completed cache
    fn promote_partial_responses(&mut self) {
        // Collect bucket requests that are ready to be promoted (no pending files)
        let mut to_promote = Vec::new();
        for (bucket_request, partial_response) in self.partial_responses.iter() {
            if partial_response.files().is_empty() {
                to_promote.push(bucket_request.clone());
            }
        }

        // Promote completed partial responses to complete responses
        for bucket_request in to_promote {
            if let Some(partial_response) = self.partial_responses.pop(&bucket_request) {
                self.complete_responses
                    .put(bucket_request, partial_response.to_complete());
            }
        }
    }

    /// Returns a snapshot of current indexing statistics.
    #[cfg(feature = "indexing-stats")]
    pub fn indexing_stats(&self) -> crate::index_state::cache::IndexingStats {
        self.cache.indexing_stats()
    }

    /// Prints indexing statistics to stdout.
    #[cfg(feature = "indexing-stats")]
    pub fn print_indexing_stats(&self) {
        self.cache.print_indexing_stats();
    }

    #[cfg(feature = "allocative")]
    pub fn build_fg(&self) {
        use allocative::FlameGraphBuilder;
        use std::fmt::Write as _;

        let mut output = String::new();

        // NOTE: Iteration over foyer::HybridCache is not supported.
        // This functionality needs to be redesigned if needed.
        // The per-file flamegraph generation has been disabled.

        // TODO: Consider tracking files separately if per-file flamegraphs are needed

        let mut flamegraph = FlameGraphBuilder::default();
        flamegraph.visit_root(self);

        let fg_output = flamegraph.finish();
        let fg_str = fg_output.flamegraph().write();
        writeln!(output, "{}", fg_str).unwrap();

        std::fs::write("/home/vk/mo/flamegraph.fg", output).unwrap();

        // Generate SVG using inferno
        use std::process::Command;
        let status = Command::new("sh")
            .arg("-c")
            .arg(r#"cat ~/mo/flamegraph.fg | inferno-flamegraph --title "Journal Cache Memory by File" --colors mem --countname "bytes" > ~/mo/flamegraph.svg"#)
            .status()
            .expect("Failed to execute inferno-flamegraph");

        if status.success() {
            println!("Flamegraph SVG generated at ~/mo/flamegraph.svg");
        } else {
            eprintln!("Failed to generate flamegraph SVG");
        }
    }
}
