//! Histogram functionality for generating time-series data from journal files.
//!
//! This module provides types and services for computing histograms of journal log entries
//! over time ranges, with support for filtering and faceted field indexing.

use super::Result;
use super::{
    BucketCacheMetrics, BucketOperationsMetrics, Facets, File, FileIndexCache, FileIndexKey,
    FileIndexStream, IndexingService, Registry,
};
use futures::StreamExt;
use journal::FieldName;
use journal::collections::{HashMap, HashSet};
use journal::index::Filter;
use parking_lot::RwLock;
use rt::ChartHandle;
use std::time::Duration;
use tracing::{debug, error, instrument};

/// A bucket request contains a [start, end) time range along with the
/// filter that should be applied.
#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct BucketRequest {
    /// Start time of the bucket request
    pub start: u32,
    /// End time of the bucket request
    pub end: u32,
    /// Facets to use for file index
    pub facets: Facets,
    /// Applied filter expression
    pub filter_expr: Filter,
}

impl BucketRequest {
    /// The duration of the bucket request in seconds
    pub fn duration(&self) -> u32 {
        self.end - self.start
    }

    /// Returns the next bucket request with the same duration, facets, and filter.
    /// The next bucket starts where this bucket ends.
    pub fn next(&self) -> Self {
        let duration = self.duration();
        Self {
            start: self.end,
            end: self.end + duration,
            facets: self.facets.clone(),
            filter_expr: self.filter_expr.clone(),
        }
    }

    /// Returns the previous bucket request with the same duration, facets, and filter.
    /// The previous bucket ends where this bucket starts.
    pub fn prev(&self) -> Self {
        let duration = self.duration();
        Self {
            start: self.start.saturating_sub(duration),
            end: self.start,
            facets: self.facets.clone(),
            filter_expr: self.filter_expr.clone(),
        }
    }
}

/// A histogram request for a given [start, end) time range with a specific
/// filter expression that should be matched.
#[derive(Debug, Clone)]
pub struct HistogramRequest {
    /// Start time
    pub after: u32,
    /// End time
    pub before: u32,
    /// Facets to use for file indexes
    pub(crate) facets: Facets,
    /// Filter expression to apply
    pub filter_expr: Filter,
}

impl HistogramRequest {
    pub fn new(after: u32, before: u32, facets: &[String], filter_expr: &Filter) -> Self {
        Self {
            after,
            before,
            facets: Facets::new(facets),
            filter_expr: filter_expr.clone(),
        }
    }

    /// Returns the bucket requests that should be used in order to
    /// generate data for this histogram. The bucket duration is automatically
    /// determined by time range of the histogram request, and it's large
    /// enough to return at least 100 bucket requests.
    pub(crate) fn bucket_requests(&self) -> Vec<BucketRequest> {
        let bucket_duration = self.calculate_bucket_duration();

        // Buckets are aligned to their duration
        let aligned_start = (self.after / bucket_duration) * bucket_duration;
        let aligned_end = self.before.div_ceil(bucket_duration) * bucket_duration;

        // Allocate our buckets
        let num_buckets = ((aligned_end - aligned_start) / bucket_duration) as usize;
        let mut buckets = Vec::with_capacity(num_buckets);
        assert!(
            num_buckets > 0,
            "histogram requests should always have at least one bucket"
        );

        // Create our buckets
        for bucket_index in 0..num_buckets {
            let start = aligned_start + (bucket_index as u32 * bucket_duration);

            buckets.push(BucketRequest {
                start,
                end: start + bucket_duration,
                facets: self.facets.clone(),
                filter_expr: self.filter_expr.clone(),
            });
        }

        buckets
    }

    fn calculate_bucket_duration(&self) -> u32 {
        const MINUTE: Duration = Duration::from_secs(60);
        const HOUR: Duration = Duration::from_secs(60 * MINUTE.as_secs());
        const DAY: Duration = Duration::from_secs(24 * HOUR.as_secs());

        const VALID_DURATIONS: &[Duration] = &[
            // Seconds
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(15),
            Duration::from_secs(30),
            // Minutes
            MINUTE,
            Duration::from_secs(2 * MINUTE.as_secs()),
            Duration::from_secs(3 * MINUTE.as_secs()),
            Duration::from_secs(5 * MINUTE.as_secs()),
            Duration::from_secs(10 * MINUTE.as_secs()),
            Duration::from_secs(15 * MINUTE.as_secs()),
            Duration::from_secs(30 * MINUTE.as_secs()),
            // Hours
            HOUR,
            Duration::from_secs(2 * HOUR.as_secs()),
            Duration::from_secs(6 * HOUR.as_secs()),
            Duration::from_secs(8 * HOUR.as_secs()),
            Duration::from_secs(12 * HOUR.as_secs()),
            // Days
            DAY,
            Duration::from_secs(2 * DAY.as_secs()),
            Duration::from_secs(3 * DAY.as_secs()),
            Duration::from_secs(5 * DAY.as_secs()),
            Duration::from_secs(7 * DAY.as_secs()),
            Duration::from_secs(14 * DAY.as_secs()),
            Duration::from_secs(30 * DAY.as_secs()),
        ];

        let duration = self.before - self.after;

        VALID_DURATIONS
            .iter()
            .rev()
            .find(|&&bucket_width| duration as u64 / bucket_width.as_secs() >= 50)
            .map(|d| d.as_secs())
            .unwrap_or(1) as u32
    }
}

/// Contains metadata for tracking which files are needed for a bucket request.
#[derive(Debug, Clone)]
pub struct RequestMetadata {
    /// Files we need to use to generate a full response
    pub files: HashSet<File>,
}

/// A partial bucket response.
///
/// Partial bucket responses reference files that still need to be indexed
/// to complete the response.
#[derive(Debug, Clone)]
pub(crate) struct BucketPartialResponse {
    /// Used to incrementally progress request
    pub(crate) request_metadata: RequestMetadata,

    /// Maps field=value pairs to (unfiltered, filtered) counts
    pub(crate) fv_counts: HashMap<journal::FieldValuePair, (usize, usize)>,

    /// Set of fields that are not indexed
    pub(crate) unindexed_fields: HashSet<FieldName>,
}

impl BucketPartialResponse {
    pub(crate) fn new(request_metadata: RequestMetadata) -> Self {
        Self {
            request_metadata,
            fv_counts: Default::default(),
            unindexed_fields: Default::default(),
        }
    }

    pub(crate) fn files(&self) -> &HashSet<File> {
        &self.request_metadata.files
    }

    pub(crate) fn to_complete(&self) -> BucketCompleteResponse {
        BucketCompleteResponse {
            fv_counts: self.fv_counts.clone(),
            unindexed_fields: self.unindexed_fields.clone(),
        }
    }
}

/// A complete bucket response.
///
/// Contains the same information as a partial bucket response, but without
/// the request metadata since all files have been indexed.
#[derive(Debug, Clone)]
pub struct BucketCompleteResponse {
    /// Maps field=value pairs to (unfiltered, filtered) counts
    pub fv_counts: HashMap<journal::FieldValuePair, (usize, usize)>,
    /// Set of fields that are not indexed
    pub unindexed_fields: HashSet<FieldName>,
}

/// Internal enum for bucket response variants.
#[derive(Debug, Clone)]
enum BucketResponseInner {
    Partial(BucketPartialResponse),
    Complete(BucketCompleteResponse),
}

/// A bucket response that can be either partial (still indexing) or complete (fully indexed).
#[derive(Debug, Clone)]
pub struct BucketResponse(BucketResponseInner);

impl BucketResponse {
    /// Creates a partial bucket response (internal API).
    pub(crate) fn partial(response: BucketPartialResponse) -> Self {
        Self(BucketResponseInner::Partial(response))
    }

    /// Creates a complete bucket response (public API).
    pub fn complete(response: BucketCompleteResponse) -> Self {
        Self(BucketResponseInner::Complete(response))
    }

    /// Returns true if this is a partial response (still indexing files).
    pub fn is_partial(&self) -> bool {
        matches!(self.0, BucketResponseInner::Partial(_))
    }

    /// Returns true if this is a complete response (all files indexed).
    pub fn is_complete(&self) -> bool {
        matches!(self.0, BucketResponseInner::Complete(_))
    }

    /// Get all indexed field names from this bucket response.
    pub fn indexed_fields(&self) -> HashSet<FieldName> {
        self.fv_counts()
            .keys()
            .map(|pair| pair.extract_field())
            .collect()
    }

    /// Get the set of unindexed field names.
    pub fn unindexed_fields(&self) -> &HashSet<FieldName> {
        match &self.0 {
            BucketResponseInner::Partial(partial) => &partial.unindexed_fields,
            BucketResponseInner::Complete(complete) => &complete.unindexed_fields,
        }
    }

    /// Get a reference to the fv_counts HashMap regardless of variant.
    pub fn fv_counts(&self) -> &HashMap<journal::FieldValuePair, (usize, usize)> {
        match &self.0 {
            BucketResponseInner::Partial(partial) => &partial.fv_counts,
            BucketResponseInner::Complete(complete) => &complete.fv_counts,
        }
    }
}

/// Represents the result of a histogram evaluation.
#[derive(Debug, Clone)]
pub struct HistogramResponse {
    pub buckets: Vec<(BucketRequest, BucketResponse)>,
}

impl HistogramResponse {
    /// Returns the start time of the histogram (first bucket's start time).
    pub fn start_time(&self) -> u32 {
        let bucket_request = &self
            .buckets
            .first()
            .expect("histogram with at least one bucket")
            .0;
        bucket_request.start
    }

    /// Returns the end time of the histogram (last bucket's end time).
    pub fn end_time(&self) -> u32 {
        let bucket_request = &self
            .buckets
            .last()
            .expect("histogram with at least one bucket")
            .0;
        bucket_request.end
    }

    /// Returns the duration of each bucket in seconds.
    pub fn bucket_duration(&self) -> u32 {
        self.buckets
            .first()
            .expect("histogram with at least one bucket")
            .0
            .duration()
    }

    pub fn progress(&self) -> u32 {
        let mut partial_buckets = 0u32;
        let mut complete_buckets = 0u32;

        for (_, bucket_response) in self.buckets.iter() {
            if bucket_response.is_partial() {
                partial_buckets += 1;
            };

            if bucket_response.is_complete() {
                complete_buckets += 1;
            };
        }

        let total_buckets = partial_buckets + complete_buckets;

        (complete_buckets * 100)
            .checked_div(total_buckets)
            .unwrap_or(0)
    }
}

/// Service for computing histograms using the catalog's components.
pub struct HistogramService {
    registry: Registry,
    indexing_service: IndexingService,
    file_index_cache: FileIndexCache,
    partial_responses: RwLock<HashMap<BucketRequest, BucketPartialResponse>>,
    complete_responses: RwLock<HashMap<BucketRequest, BucketCompleteResponse>>,
    _bucket_cache_metrics: ChartHandle<BucketCacheMetrics>,
    bucket_operations_metrics: ChartHandle<BucketOperationsMetrics>,
}

impl HistogramService {
    /// Creates a new HistogramService.
    pub fn new(
        registry: Registry,
        indexing_service: IndexingService,
        file_index_cache: FileIndexCache,
        bucket_cache_metrics: ChartHandle<BucketCacheMetrics>,
        bucket_operations_metrics: ChartHandle<BucketOperationsMetrics>,
    ) -> Self {
        Self {
            registry,
            indexing_service,
            file_index_cache,
            partial_responses: RwLock::new(HashMap::default()),
            complete_responses: RwLock::new(HashMap::default()),
            _bucket_cache_metrics: bucket_cache_metrics,
            bucket_operations_metrics,
        }
    }

    /// Process a histogram request and return the histogram response.
    #[instrument(skip(self), fields(
        after = request.after,
        before = request.before,
        time_range = request.before - request.after,
        num_facets = request.facets.len(),
    ))]
    pub async fn get_histogram(&self, request: HistogramRequest) -> Result<HistogramResponse> {
        let bucket_requests = request.bucket_requests();
        let num_buckets = bucket_requests.len();
        debug!(num_buckets, "Processing histogram request");

        // Create partial responses for buckets we don't have
        let files_to_index = self.create_partial_responses(&bucket_requests)?;
        debug!(num_files = files_to_index.len(), "Files to index");

        // Create iterator to fetch/compute file indexes
        let bucket_duration = bucket_requests.first().unwrap().duration();
        let source_timestamp_field = FieldName::new_unchecked("__REALTIME_TIMESTAMP");
        let time_budget = Duration::from_secs(10); // TODO: Make configurable

        // Build file index keys
        let file_index_keys: Vec<FileIndexKey> = files_to_index
            .iter()
            .map(|file| FileIndexKey::new(file, &request.facets))
            .collect();

        // Create stream and process indexes
        if !file_index_keys.is_empty() {
            let mut stream = FileIndexStream::new(
                self.indexing_service.clone(),
                self.file_index_cache.clone(),
                self.registry.clone(),
                file_index_keys,
                source_timestamp_field,
                bucket_duration,
                time_budget,
            );

            // Process file indexes and update partial responses
            while let Some(result) = stream.next().await {
                match result {
                    Ok(file_index_response) => {
                        if let Ok(file_index) = file_index_response.result {
                            let file = &file_index_response.key.file;

                            // Acquire write lock for each file update (released immediately after)
                            let mut partial_responses = self.partial_responses.write();

                            // Get file's time range from the index
                            let (file_start, file_end) = file_index.histogram().time_range();

                            // Find all bucket requests that need data from this file
                            for bucket_request in &bucket_requests {
                                let partial = match partial_responses.get_mut(bucket_request) {
                                    Some(p) => p,
                                    None => continue,
                                };

                                // Check if we should process this file
                                if partial.files().contains(file) {
                                    partial.request_metadata.files.remove(file);
                                } else {
                                    continue;
                                }

                                // Skip if file's time range doesn't overlap with bucket's time range
                                // File range [file_start, file_end) overlaps with [bucket_start, bucket_end) if:
                                // file_start < bucket_end && file_end > bucket_start
                                if file_start >= bucket_request.end
                                    || file_end <= bucket_request.start
                                {
                                    continue;
                                }

                                // Resolve filter to bitmap
                                let filter_bitmap = if !bucket_request.filter_expr.is_none() {
                                    Some(bucket_request.filter_expr.resolve(&file_index).evaluate())
                                } else {
                                    None
                                };

                                // Track unindexed fields
                                for field in file_index.fields() {
                                    if !file_index.is_indexed(field) {
                                        if let Some(field_name) = FieldName::new(field) {
                                            partial.unindexed_fields.insert(field_name);
                                        }
                                    }
                                }

                                // Count field=value pairs in this file for this bucket's time range
                                for (indexed_field, field_bitmap) in file_index.bitmaps() {
                                    let unfiltered_count = file_index
                                        .count_bitmap_entries_in_range(
                                            field_bitmap,
                                            bucket_request.start,
                                            bucket_request.end,
                                        )
                                        .unwrap_or(0);

                                    let filtered_count =
                                        if let Some(ref filter_bitmap) = filter_bitmap {
                                            let filtered_bitmap = field_bitmap & filter_bitmap;
                                            file_index
                                                .count_bitmap_entries_in_range(
                                                    &filtered_bitmap,
                                                    bucket_request.start,
                                                    bucket_request.end,
                                                )
                                                .unwrap_or(0)
                                        } else {
                                            unfiltered_count
                                        };

                                    // Update counts
                                    if let Some(pair) =
                                        journal::FieldValuePair::parse(indexed_field)
                                    {
                                        let counts =
                                            partial.fv_counts.entry(pair).or_insert((0, 0));
                                        counts.0 += unfiltered_count;
                                        counts.1 += filtered_count;
                                    }
                                }
                            }
                            // Lock is dropped here automatically
                        }
                    }
                    Err(e) => {
                        debug!("Iterator error: {}", e);
                        break;
                    }
                }
            }
        }

        // Promote completed partial responses
        self.promote_partial_responses();

        // Build the response
        let complete_responses = self.complete_responses.read();
        let partial_responses = self.partial_responses.read();

        let buckets = bucket_requests
            .into_iter()
            .filter_map(|bucket_request| {
                if let Some(complete) = complete_responses.get(&bucket_request) {
                    Some((bucket_request, BucketResponse::complete(complete.clone())))
                } else {
                    partial_responses
                        .get(&bucket_request)
                        .map(|partial| (bucket_request, BucketResponse::partial(partial.clone())))
                }
            })
            .collect();

        Ok(HistogramResponse { buckets })
    }

    /// Creates partial responses for bucket requests that don't exist in caches.
    ///
    /// Queries the registry once for the entire histogram time range for efficiency,
    /// then assigns files to all buckets. The indexing/counting logic filters entries
    /// by time range.
    fn create_partial_responses(&self, bucket_requests: &[BucketRequest]) -> Result<HashSet<File>> {
        let complete_responses = self.complete_responses.read();
        let mut partial_responses = self.partial_responses.write();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        // Collect buckets that need creation
        let mut buckets_to_create = Vec::new();
        let mut invalidated_count = 0u64;

        for bucket_request in bucket_requests {
            // If bucket covers current time and exists in partial cache,
            // remove it so it gets rebuilt with fresh data
            if bucket_request.end > now && partial_responses.contains_key(bucket_request) {
                partial_responses.remove(bucket_request);
                invalidated_count += 1;
            }

            // Skip if we already have a response
            if complete_responses.contains_key(bucket_request)
                || partial_responses.contains_key(bucket_request)
            {
                continue;
            }

            buckets_to_create.push(bucket_request.clone());
        }

        // Track invalidations
        if invalidated_count > 0 {
            self.bucket_operations_metrics.update(|m| {
                m.invalidated += invalidated_count;
            });
        }

        if buckets_to_create.is_empty() {
            return Ok(HashSet::default());
        }

        // Assert buckets are sorted by time (development check)
        assert!(
            buckets_to_create
                .windows(2)
                .all(|w| w[0].start <= w[1].start),
            "Buckets must be sorted by start time"
        );
        assert!(
            buckets_to_create.windows(2).all(|w| w[0].end <= w[1].end),
            "Buckets must be sorted by end time"
        );

        // Query registry once for entire histogram time range
        let histogram_start = bucket_requests.first().unwrap().start;
        let histogram_end = bucket_requests.last().unwrap().end;
        let histogram_file_infos = self
            .registry
            .find_files_in_range(histogram_start, histogram_end)?;

        debug!(
            "Found {} file infos in histograms time range [{},{})",
            histogram_file_infos.len(),
            histogram_start,
            histogram_end
        );

        for (idx, file_info) in histogram_file_infos.iter().enumerate() {
            debug!("[{}] {}", idx, file_info.file.path());
        }

        // Convert to a set - all buckets will share this file set
        let histogram_files: HashSet<File> =
            histogram_file_infos.into_iter().map(|f| f.file).collect();

        // Track bucket creations
        let created_count = buckets_to_create.len() as u64;

        // Create partial responses for each bucket using the same file set
        for bucket_request in buckets_to_create {
            let request_metadata = RequestMetadata {
                files: histogram_files.clone(),
            };
            let partial_response = BucketPartialResponse::new(request_metadata);
            partial_responses.insert(bucket_request, partial_response);
        }

        // Update creation metric
        self.bucket_operations_metrics.update(|m| {
            m.created += created_count;
        });

        Ok(histogram_files)
    }

    /// Promotes partial responses to complete responses when all files are indexed.
    fn promote_partial_responses(&self) {
        let mut partial_responses = self.partial_responses.write();
        let mut complete_responses = self.complete_responses.write();

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as u32;

        let to_promote: Vec<BucketRequest> = partial_responses
            .iter()
            .filter(|(bucket_request, partial)| {
                // Only promote if all files are indexed AND bucket doesn't cover current time
                partial.files().is_empty() && bucket_request.end < now
            })
            .map(|(req, _)| req.clone())
            .collect();

        let promoted_count = to_promote.len() as u64;

        for bucket_request in to_promote {
            if let Some(partial) = partial_responses.remove(&bucket_request) {
                complete_responses.insert(bucket_request, partial.to_complete());
            }
        }

        // Update promotion metric
        if promoted_count > 0 {
            self.bucket_operations_metrics.update(|m| {
                m.promoted += promoted_count;
            });
        }
    }
}
