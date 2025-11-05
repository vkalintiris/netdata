use crate::indexing::IndexingService;
use crate::error::Result;
use crate::request::{BucketRequest, HistogramFacets, HistogramRequest, RequestMetadata};
use crate::response::{
    BucketCompleteResponse, BucketPartialResponse, BucketResponse, HistogramResult,
};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::collections::{HashMap, HashSet};
use journal::registry::Registry;
use journal::repository::File;
use lru::LruCache;
use std::num::NonZeroUsize;
use std::time::Instant;
use tracing::{debug, instrument, warn};
#[cfg(feature = "allocative")]
use tracing::{error, info};

/// Time range information for a journal file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileTimeRange {
    /// File has not been indexed yet, explicit time range unknown.
    /// These files will be queued for indexing and reported in subsequent poll cycles.
    Unknown,
    /// Active file currently being written to. The end time represents the latest
    /// entry seen when the file was indexed, but new entries may have been written since.
    /// The indexed_at timestamp allows consumers to decide whether to re-index for fresher data.
    /// This is an explicit time range from the FileIndex.
    Active {
        start: u32,
        end: u32,
        indexed_at: u64, // Unix timestamp in seconds when the index was created
    },
    /// Archived file with known start and end times.
    /// This is an explicit time range from the FileIndex.
    Bounded {
        start: u32,
        end: u32,
        indexed_at: u64, // Unix timestamp in seconds when the index was created
    },
}

/// Extension of File that includes cached time range from the FileIndex.
///
/// This type augments a File with optional time range metadata obtained from
/// the IndexingService. The time range represents the actual temporal span of log
/// entries in the file (in seconds since epoch).
#[derive(Debug, Clone)]
pub struct FileWithRange {
    pub file: File,
    /// Cached time range from the FileIndex.
    pub time_range: FileTimeRange,
}

#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramService {
    pub registry: Registry,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub indexing_service: IndexingService,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub partial_responses: LruCache<BucketRequest, BucketPartialResponse>,
    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub complete_responses: LruCache<BucketRequest, BucketCompleteResponse>,
}

impl HistogramService {
    /// Creates a new HistogramService from an IndexingService and journal directory path.
    ///
    /// # Arguments
    /// * `indexing` - The IndexingService to use for file indexing
    /// * `path` - Journal directory path to watch
    pub fn new(indexing_service: IndexingService, path: &str) -> Result<Self> {
        let mut registry = Registry::new()?;
        registry.watch_directory(path)?;

        let cache_capacity = NonZeroUsize::new(1000).unwrap();

        Ok(Self {
            registry,
            indexing_service,
            partial_responses: LruCache::new(cache_capacity),
            complete_responses: LruCache::new(cache_capacity),
        })
    }

    /// Discovers files in the specified time range, augmented with explicit time range metadata.
    ///
    /// This method first uses the registry to discover candidate files based on rotation times,
    /// then augments each file with explicit time range data from the IndexingService if available.
    ///
    /// Files with Unknown time ranges (not yet indexed) are included conservatively and will be
    /// queued for indexing. The progressive response system handles reporting results as files
    /// become indexed in subsequent poll cycles.
    ///
    /// # Arguments
    /// * `start` - Start time in seconds since epoch
    /// * `end` - End time in seconds since epoch
    ///
    /// # Returns
    /// Vector of FileWithRange containing files and their explicit time ranges (or Unknown)
    pub fn find_files_in_range(&self, start: u32, end: u32) -> Vec<FileWithRange> {
        const USEC_PER_SEC: u64 = 1_000_000;
        let start_usec = start as u64 * USEC_PER_SEC;
        let end_usec = end as u64 * USEC_PER_SEC;

        // Get candidates from registry (rotation-based discovery)
        let candidates = self.registry.find_files_in_range(start, end);

        // Augment with explicit time ranges from cache and filter
        candidates
            .into_iter()
            .filter_map(|file| {
                let time_range = self.indexing_service.get_time_range(&file);

                // Filter based on explicit time range if available
                let include = match time_range {
                    FileTimeRange::Unknown => true, // Not indexed yet, keep conservatively
                    FileTimeRange::Active {
                        start: s, end: e, ..
                    } => {
                        let indexed_start = s as u64 * USEC_PER_SEC;
                        let indexed_end = e as u64 * USEC_PER_SEC;
                        // Use the cached end time for filtering, but note that the file
                        // may have grown since indexed_at
                        indexed_start < end_usec && indexed_end > start_usec
                    }
                    FileTimeRange::Bounded {
                        start: s, end: e, ..
                    } => {
                        let indexed_start = s as u64 * USEC_PER_SEC;
                        let indexed_end = e as u64 * USEC_PER_SEC;
                        indexed_start < end_usec && indexed_end > start_usec
                    }
                };

                if include {
                    Some(FileWithRange { file, time_range })
                } else {
                    None
                }
            })
            .collect()
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

    pub fn send_indexing_requests(
        &self,
        facets: &HistogramFacets,
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
    pub async fn get_histogram(&mut self, request: HistogramRequest) -> HistogramResult {
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
                BucketResponse::Complete(complete.clone())
            } else if let Some(partial) = self.partial_responses.get_mut(&bucket_request) {
                partial_count += 1;
                BucketResponse::Partial(partial.clone())
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
                fv_counts: HashMap::default(),
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
