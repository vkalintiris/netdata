use crate::request::{BucketRequest, RequestMetadata};
use journal::collections::{HashMap, HashSet};
use journal::index::{FileIndex, Filter};
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
    pub(crate) fn duration(&self) -> u32 {
        self.request_metadata.request.duration()
    }

    pub(crate) fn files(&self) -> &HashSet<File> {
        &self.request_metadata.files
    }

    pub(crate) fn start_time(&self) -> u32 {
        self.request_metadata.request.start
    }

    pub(crate) fn end_time(&self) -> u32 {
        self.request_metadata.request.end
    }

    pub(crate) fn filter_expr(&self) -> &Filter {
        &self.request_metadata.request.filter_expr
    }

    pub(crate) fn to_complete(&self) -> BucketCompleteResponse {
        BucketCompleteResponse {
            fv_counts: self.fv_counts.clone(),
            unindexed_fields: self.unindexed_fields.clone(),
        }
    }

    pub(crate) fn update(&mut self, file: &File, file_index: &FileIndex) {
        // Nothing to do if we the request does not contain this file
        if !self.request_metadata.files.contains(file) {
            return;
        }

        // Can not use file index, if it doesn't have sufficient granularity
        if self.duration() < file_index.bucket_duration() {
            return;
        }

        // Remove the file from the queue
        self.request_metadata.files.remove(file);

        // Track fields that exist in the file but were not indexed
        // This allows the UI to distinguish between indexed and unindexed fields
        for field in file_index.fields() {
            if file_index.is_indexed(field) {
                continue;
            }

            if let Some(field_name) = FieldName::new(field) {
                self.unindexed_fields.insert(field_name);
            }
        }

        // TODO: should `resolve`/`evaluate` return an `Option`?
        let filter_expr = self.filter_expr();
        let filter_bitmap = if !filter_expr.is_none() {
            Some(filter_expr.resolve(file_index).evaluate())
        } else {
            None
        };

        let start_time = self.start_time();
        let end_time = self.end_time();

        for (indexed_field, field_bitmap) in file_index.bitmaps() {
            // Calculate unfiltered count (all occurrences of this field=value)
            let unfiltered_count = file_index
                .count_bitmap_entries_in_range(field_bitmap, start_time, end_time)
                .unwrap_or(0);

            // Calculate filtered count (occurrences matching the filter expression)
            // When no filter is specified, filtered = unfiltered (shows all entries)
            let filtered_count = if let Some(filter_bitmap) = &filter_bitmap {
                let filtered_bitmap = field_bitmap & filter_bitmap;
                file_index
                    .count_bitmap_entries_in_range(&filtered_bitmap, start_time, end_time)
                    .unwrap_or(0)
            } else {
                unfiltered_count
            };

            // Update the counts for this field=value pair
            // Parse the indexed_field string into a FieldValuePair
            if let Some(pair) = FieldValuePair::parse(indexed_field) {
                if let Some(counts) = self.fv_counts.get_mut(&pair) {
                    counts.0 += unfiltered_count;
                    counts.1 += filtered_count;
                } else {
                    self.fv_counts
                        .insert(pair, (unfiltered_count, filtered_count));
                }
            }
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
