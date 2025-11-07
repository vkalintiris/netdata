use super::request::{BucketRequest, HistogramRequest, RequestMetadata};
use super::response::{
    BucketCompleteResponse, BucketPartialResponse, BucketResponse, HistogramResponse,
};
use crate::error::Result;
use crate::indexing::{Facets, IndexingService};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::HashSet;
use journal::registry::{Monitor, Registry};
use journal::repository::File;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Instant;
use tracing::{debug, instrument, warn};
#[cfg(feature = "allocative")]
use tracing::{error, info};

#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramService {
    pub registry: Registry,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub indexing_service: IndexingService,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub(crate) partial_responses: LruCache<BucketRequest, BucketPartialResponse>,
    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub(crate) complete_responses: LruCache<BucketRequest, BucketCompleteResponse>,
}

impl HistogramService {
    /// Creates a new HistogramService from journal directory path and an IndexingService.
    ///
    /// # Arguments
    /// * `path` - Journal directory path to watch
    /// * `indexing` - The IndexingService to use for file indexing
    pub fn new(path: &str, indexing_service: IndexingService) -> Result<Self> {
        let monitor = Monitor::new()?;
        let mut registry = Registry::new(monitor);
        registry.watch_directory(path)?;

        let cache_capacity = NonZeroUsize::new(1000).unwrap();

        Ok(Self {
            registry,
            indexing_service,
            partial_responses: LruCache::new(cache_capacity),
            complete_responses: LruCache::new(cache_capacity),
        })
    }

    #[instrument(skip(self), fields(
        after = request.after,
        before = request.before,
        time_range = request.before - request.after,
        num_facets = request.facets.len(),
    ))]
    pub async fn process_histogram_request(&mut self, request: &HistogramRequest) {
        // Create any partial responses we don't already have
        let bucket_requests = request.bucket_requests();
        let num_buckets = bucket_requests.len();
        assert!(!bucket_requests.is_empty());
        debug!(num_buckets, "Creating partial responses");
        self.create_partial_responses(&bucket_requests);

        // Figure out the files we will need to lookup for partial requests
        let pending_files = self.collect_partial_requests_files(&bucket_requests);
        debug!(
            pending_files = pending_files.len(),
            "Collected pending files"
        );

        // Send indexing requests
        let facets = &request.facets;
        let bucket_duration = bucket_requests.first().unwrap().duration();
        self.send_indexing_requests(facets, &pending_files, bucket_duration);

        // Progress partial responses
        self.indexing_service
            .resolve_partial_responses(facets, &mut self.partial_responses, pending_files)
            .await;

        // Promote those that have been completed from partial to complete
        // responses
        self.promote_partial_responses();
    }

    #[instrument(skip(self), fields(
        after = request.after,
        before = request.before,
    ))]
    pub async fn get_histogram(&mut self, request: HistogramRequest) -> HistogramResponse {
        // Process the histogram request to ensure buckets are computed/in-progress
        self.process_histogram_request(&request).await;

        // Generate the bucket requests for this histogram
        let bucket_requests = request.bucket_requests();

        // Collect the responses for each bucket
        let mut buckets = Vec::with_capacity(bucket_requests.len());
        let mut complete_count = 0;
        let mut partial_count = 0;

        for bucket_request in bucket_requests {
            let response = if let Some(complete) = self.complete_responses.get_mut(&bucket_request)
            {
                complete_count += 1;
                BucketResponse::complete(complete.clone())
            } else if let Some(partial) = self.partial_responses.get_mut(&bucket_request) {
                partial_count += 1;
                BucketResponse::partial(partial.clone())
            } else {
                // This shouldn't happen after process_histogram_request, but handle it gracefully
                warn!("Missing bucket response for request: {:?}", bucket_request);
                continue;
            };

            buckets.push((bucket_request, response));
        }

        debug!(
            complete = complete_count,
            partial = partial_count,
            total = buckets.len(),
            "Histogram result collected"
        );

        HistogramResponse { buckets }
    }

    /// Creates the request metadata, ie. the bucket request itself, along
    /// with the files it needs.
    fn create_request_metadata(&self, bucket_request: BucketRequest) -> RequestMetadata {
        let files = self
            .registry
            .find_files_in_range(bucket_request.start, bucket_request.end);

        RequestMetadata { files }
    }

    /// Creates responses for bucket requests that we don't have in our caches
    fn create_partial_responses(&mut self, bucket_requests: &[BucketRequest]) {
        // NOTE: Use `get()`, instead of `peek()`, to make the request the
        // most recently used in our caches.
        for bucket_request in bucket_requests {
            // Ignore if we have this request in our LRU caches
            if self.complete_responses.get(bucket_request).is_some()
                || self.partial_responses.get(bucket_request).is_some()
            {
                continue;
            }

            // We do not have any partial or complete response for this bucket
            // request. Build a new partial response with the journal files
            // that it needs to query.

            let request_metadata = self.create_request_metadata(bucket_request.clone());
            let partial_response = BucketPartialResponse::new(request_metadata);

            self.partial_responses
                .put(bucket_request.clone(), partial_response);
        }
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
                warn!(
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

    pub(crate) fn send_indexing_requests(
        &self,
        facets: &Facets,
        pending_files: &HashSet<File>,
        bucket_duration: u32,
    ) {
        use crate::indexing::IndexingRequest;

        debug!("Pending files: {}", pending_files.len());
        for file in pending_files {
            let indexing_request = IndexingRequest {
                facets: facets.clone(),
                bucket_duration,
                file: file.clone(),
                instant: Instant::now(),
            };

            // Use try_send to avoid blocking if queue is full
            // Ignore full queue errors - newer requests will arrive soon
            let _ = self.indexing_service.try_send_request(indexing_request);
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

    /// Gracefully closes the histogram cache, ensuring all pending cache writes are flushed to disk.
    /// Should be called during application shutdown.
    pub async fn close(&self) -> Result<()> {
        self.indexing_service.close().await?;
        Ok(())
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
            info!("Flamegraph SVG generated at ~/mo/flamegraph.svg");
        } else {
            error!("Failed to generate flamegraph SVG");
        }
    }
}
