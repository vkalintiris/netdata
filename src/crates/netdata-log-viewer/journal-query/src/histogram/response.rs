use super::request::{BucketRequest, RequestMetadata};
use journal::collections::{HashMap, HashSet};
use journal::repository::File;
use journal::{FieldName, FieldValuePair};

#[cfg(feature = "allocative")]
use allocative::Allocative;

/// A partial bucket response.
///
/// Partial bucket responses reference files that should be used to fulfill
/// the request and progress towards a complete/full response.
///
/// Each bucket response contains a set of unindexed fields and a hash table
/// mapping indexed fields to a tuple of (unfiltered, filtered) counts.
///
/// This is an internal type - external users interact via BucketResponse wrapper.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(crate) struct BucketPartialResponse {
    // Used to incrementally progress request
    pub(crate) request_metadata: RequestMetadata,

    // Maps field=value pairs to (unfiltered, filtered) counts
    pub(crate) fv_counts: HashMap<FieldValuePair, (usize, usize)>,

    // Set of fields that are not indexed
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
/// Contains the same information as a partial bucket response. However, it
/// does not contain the request metadata, simply because they are not needed.
///
/// This is an internal type - external users interact via BucketResponse wrapper.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(crate) struct BucketCompleteResponse {
    // Maps key=value pairs to (unfiltered, filtered) counts
    pub(crate) fv_counts: HashMap<FieldValuePair, (usize, usize)>,
    // Set of fields that are not indexed
    pub(crate) unindexed_fields: HashSet<FieldName>,
}

/// Internal enum for bucket response variants.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
enum BucketResponseInner {
    Partial(BucketPartialResponse),
    Complete(BucketCompleteResponse),
}

/// A bucket response that can be either partial (still indexing) or complete (fully indexed).
///
/// This wrapper hides the internal response types and provides a stable public API.
/// Users can query the response state via methods without needing to pattern match
/// on internal types.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct BucketResponse(BucketResponseInner);

impl BucketResponse {
    /// Creates a partial bucket response (internal API).
    pub(crate) fn partial(response: BucketPartialResponse) -> Self {
        Self(BucketResponseInner::Partial(response))
    }

    /// Creates a complete bucket response (internal API).
    pub(crate) fn complete(response: BucketCompleteResponse) -> Self {
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
    pub fn fv_counts(&self) -> &HashMap<FieldValuePair, (usize, usize)> {
        match &self.0 {
            BucketResponseInner::Partial(partial) => &partial.fv_counts,
            BucketResponseInner::Complete(complete) => &complete.fv_counts,
        }
    }
}

/// Represents the result of a histogram evaluation.
///
/// It simply holds a vector of bucket (request, response) tuples. The vector
/// can be sorted by using keys from the `BucketRequest`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
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
}
