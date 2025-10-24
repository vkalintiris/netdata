use crate::collections::{HashMap, HashSet};
use crate::index::{FileIndex, FilterExpr};
use crate::index_state::request::{BucketRequest, RequestMetadata};
use crate::index_state::ui;
use crate::repository::File;
#[cfg(feature = "allocative")]
use allocative::Allocative;
use enum_dispatch::enum_dispatch;
use std::sync::Arc;

/// A partial bucket response.
///
/// Partial bucket responses reference files that should be used to fulfill
/// the request and progress towards a complete/full response.
///
/// Each bucket response contains a set of unindexed fields and a hash table
/// mapping indexed fields to a tuple of (unfiltered, filtered) counts.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct BucketPartialResponse {
    // Used to incrementally progress request
    pub request_metadata: RequestMetadata,
    // Maps key=value pairs to (unfiltered, filtered) counts
    pub indexed_fields: HashMap<String, (usize, usize)>,
    // Set of fields that are not indexed
    pub unindexed_fields: HashSet<String>,
}

impl BucketPartialResponse {
    pub fn duration(&self) -> u64 {
        self.request_metadata.request.duration()
    }

    pub fn files(&self) -> &HashSet<File> {
        &self.request_metadata.files
    }

    pub fn start_time(&self) -> u64 {
        self.request_metadata.request.start
    }

    pub fn end_time(&self) -> u64 {
        self.request_metadata.request.end
    }

    pub fn filter_expr(&self) -> &Arc<FilterExpr<String>> {
        &self.request_metadata.request.filter_expr
    }

    pub fn to_complete(&self) -> BucketCompleteResponse {
        BucketCompleteResponse {
            indexed_fields: self.indexed_fields.clone(),
            unindexed_fields: self.unindexed_fields.clone(),
        }
    }

    pub fn update(&mut self, file: &File, file_index: &FileIndex) {
        // Nothing to do if we the request does not contain this file
        if !self.request_metadata.files.contains(file) {
            return;
        }

        // Check if the cached index has sufficient granularity
        if file_index.bucket_duration() > self.duration() {
            // Cached index doesn't have sufficient granularity, skip it
            return;
        }

        // Remove the file from the queue
        self.request_metadata.files.remove(file);

        // Add any missing unindexed fields to the bucket
        for field in file_index.fields() {
            if file_index.is_indexed(field) {
                continue;
            }

            self.unindexed_fields.insert(field.clone());
        }

        let filter_expr = self.filter_expr().as_ref();
        let filter_bitmap = if *filter_expr != FilterExpr::<String>::None {
            Some(filter_expr.resolve(file_index).evaluate())
        } else {
            None
        };

        let start_time = self.start_time();
        let end_time = self.end_time();

        for (indexed_field, bitmap) in file_index.bitmaps() {
            // once for unfiltered count
            {
                let unfiltered_count = file_index
                    .count_bitmap_entries_in_range(bitmap, start_time, end_time)
                    .unwrap_or(0);

                if let Some((unfiltered_total, _)) = self.indexed_fields.get_mut(indexed_field) {
                    *unfiltered_total += unfiltered_count;
                } else {
                    self.indexed_fields
                        .insert(indexed_field.clone(), (unfiltered_count, 0));
                }
            }

            // once more for filtered count
            if let Some(filter_bitmap) = &filter_bitmap {
                let bitmap = bitmap & filter_bitmap;
                let filtered_count = file_index
                    .count_bitmap_entries_in_range(&bitmap, start_time, end_time)
                    .unwrap_or(0);

                if let Some((_, filtered_total)) = self.indexed_fields.get_mut(indexed_field) {
                    *filtered_total += filtered_count;
                } else {
                    self.indexed_fields
                        .insert(indexed_field.clone(), (0, filtered_count));
                }
            }
        }
    }
}

/// A complete bucket response.
///
/// Contains the same information as a partial bucket response. However, it
/// does not contain the request metadata, simply because they are not needed.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct BucketCompleteResponse {
    // Maps key=value pairs to (unfiltered, filtered) counts
    pub indexed_fields: HashMap<String, (usize, usize)>,
    // Set of fields that are not indexed
    pub unindexed_fields: HashSet<String>,
}

#[enum_dispatch]
pub trait BucketResponseOps {
    fn indexed_fields(&self) -> HashSet<String>;
    fn unindexed_fields(&self) -> &HashSet<String>;
}

impl BucketResponseOps for BucketPartialResponse {
    fn indexed_fields(&self) -> HashSet<String> {
        self.indexed_fields
            .keys()
            .filter_map(|key| key.split_once('=').map(|(field, _value)| field.to_string()))
            .collect()
    }

    fn unindexed_fields(&self) -> &HashSet<String> {
        &self.unindexed_fields
    }
}

impl BucketResponseOps for BucketCompleteResponse {
    fn indexed_fields(&self) -> HashSet<String> {
        self.indexed_fields
            .keys()
            .filter_map(|key| key.split_once('=').map(|(field, _value)| field.to_string()))
            .collect()
    }

    fn unindexed_fields(&self) -> &HashSet<String> {
        &self.unindexed_fields
    }
}

/// Type to discriminate partial vs. complete bucket responses.
#[derive(Debug, Clone)]
#[enum_dispatch(BucketResponseOps)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub enum BucketResponse {
    Partial(BucketPartialResponse),
    Complete(BucketCompleteResponse),
}

/// Represents the result of a histogram evaluation.
///
/// It simply holds a vector of bucket (request, response) tuples. The vector
/// can be sorted by using keys from the `BucketRequest`.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct HistogramResult {
    pub buckets: Vec<(BucketRequest, BucketResponse)>,
}

impl HistogramResult {
    pub fn available_histograms(&self) -> Vec<ui::AvailableHistogram> {
        let mut indexed_fields = HashSet::default();

        for (_, bucket) in &self.buckets {
            indexed_fields.extend(bucket.indexed_fields());
        }

        let mut available_histograms = Vec::with_capacity(indexed_fields.len());
        for id in indexed_fields {
            available_histograms.push(ui::AvailableHistogram {
                id: id.clone(),
                name: id,
                order: 0,
            });
        }

        available_histograms.sort_by(|a, b| a.id.cmp(&b.id));

        for (order, available_histogram) in available_histograms.iter_mut().enumerate() {
            available_histogram.order = order;
        }

        available_histograms
    }
}
