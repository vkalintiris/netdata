use crate::collections::{HashMap, HashSet};
use crate::index_state::cache::{IndexCache, IndexRequest};
use crate::index_state::error::Result;
use crate::index_state::request::{BucketRequest, HistogramRequest, RequestMetadata};
use crate::index_state::response::{
    BucketCompleteResponse, BucketPartialResponse, BucketResponse, HistogramResult,
};
use crate::registry::Registry;
use crate::repository::File;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use std::collections::VecDeque;

#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct AppState {
    pub registry: Registry,

    pub indexed_fields: HashSet<String>,

    #[cfg_attr(feature = "allocative", allocative(skip))]
    pub cache: IndexCache,

    pub partial_responses: HashMap<BucketRequest, BucketPartialResponse>,
    pub complete_responses: HashMap<BucketRequest, BucketCompleteResponse>,
}

impl AppState {
    pub fn new(path: &str, indexed_fields: std::collections::HashSet<String>) -> Result<Self> {
        let mut registry = Registry::new()?;
        registry.watch_directory(path)?;

        Ok(Self {
            registry,
            indexed_fields: indexed_fields.into_iter().collect(),
            cache: IndexCache::default(),
            partial_responses: HashMap::default(),
            complete_responses: HashMap::default(),
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
            if self.complete_responses.contains_key(bucket_request) {
                continue;
            }

            let Some(partial_response) = self.partial_responses.get(bucket_request) else {
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

    fn collect_index_requests(
        &self,
        files: &HashSet<File>,
        bucket_duration: u64,
    ) -> VecDeque<IndexRequest> {
        let mut index_requests = VecDeque::new();
        for file in files.iter().cloned() {
            index_requests.push_back(IndexRequest {
                file,
                bucket_duration,
            });
        }

        index_requests
    }

    pub fn process_histogram_request(&mut self, request: &HistogramRequest) {
        // Create any partial responses we don't already have
        let bucket_requests = request.bucket_requests();
        assert!(!bucket_requests.is_empty());
        self.create_partial_responses(&bucket_requests);

        // Figure out the files we will need to lookup for partial requests
        let pending_files = self.collect_partial_requests_files(&bucket_requests);

        // Send indexing requests
        let bucket_duration = bucket_requests.first().unwrap().duration();
        let index_requests = self.collect_index_requests(&pending_files, bucket_duration);
        self.cache
            .request_indexing(index_requests, self.indexed_fields.clone());

        // Progress partial responses
        self.cache
            .resolve_partial_responses(&mut self.partial_responses, pending_files);

        // Promote those that have been completed from partial to complete
        // responses
        self.promote_partial_responses();
    }

    pub fn get_histogram(&mut self, request: HistogramRequest) -> HistogramResult {
        // Process the histogram request to ensure buckets are computed/in-progress
        self.process_histogram_request(&request);

        // Generate the bucket requests for this histogram
        let bucket_requests = request.bucket_requests();

        // Collect the responses for each bucket
        let mut buckets = Vec::with_capacity(bucket_requests.len());
        for bucket_request in bucket_requests {
            let response = if let Some(complete) = self.complete_responses.get(&bucket_request) {
                BucketResponse::Complete(complete.clone())
            } else if let Some(partial) = self.partial_responses.get(&bucket_request) {
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
        for bucket_request in bucket_requests {
            // Ignore if we have the request in the cache of complete responses
            if self.complete_responses.contains_key(bucket_request) {
                continue;
            }

            // Ignore if we have the request in the cache of partial responses
            if self.partial_responses.contains_key(bucket_request) {
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
                .insert(bucket_request.clone(), partial_response);
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
        self.partial_responses
            .retain(|bucket_request, partial_response| {
                if !partial_response.files().is_empty() {
                    return true;
                } else {
                    self.complete_responses
                        .insert(bucket_request.clone(), partial_response.to_complete());
                    return false;
                }
            });
    }

    #[cfg(feature = "allocative")]
    pub fn build_fg(&self) {
        use allocative::FlameGraphBuilder;
        use std::fmt::Write as _;

        let mut output = String::new();

        for (file, file_index) in self.cache.file_indexes.read().iter() {
            let mut flamegraph = FlameGraphBuilder::default();
            flamegraph.visit_root(file_index);

            let fg_output = flamegraph.finish();

            let fg_str = fg_output.flamegraph().write();
            for line in fg_str.lines() {
                if !line.is_empty() {
                    // Prepend the file path to each stack trace
                    writeln!(output, "{};{}", file.path(), line).unwrap();
                }
            }
        }

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
