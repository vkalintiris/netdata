use crate::request::{BucketRequest, RequestMetadata};
use crate::ui;
use journal::collections::{HashMap, HashSet};
use journal::index::{FileIndex, FilterExpr, FilterTarget};
use journal::repository::File;
use journal::{FieldName, FieldValuePair};
use std::sync::Arc;

#[cfg(feature = "allocative")]
use allocative::Allocative;

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

    // Maps field=value pairs to (unfiltered, filtered) counts
    pub fv_counts: HashMap<FieldValuePair, (usize, usize)>,

    // Set of fields that are not indexed
    pub unindexed_fields: HashSet<FieldName>,
}

impl BucketPartialResponse {
    pub fn duration(&self) -> u32 {
        self.request_metadata.request.duration()
    }

    pub fn files(&self) -> &HashSet<File> {
        &self.request_metadata.files
    }

    pub fn start_time(&self) -> u32 {
        self.request_metadata.request.start
    }

    pub fn end_time(&self) -> u32 {
        self.request_metadata.request.end
    }

    pub fn filter_expr(&self) -> &Arc<FilterExpr<FilterTarget>> {
        &self.request_metadata.request.filter_expr
    }

    pub fn to_complete(&self) -> BucketCompleteResponse {
        BucketCompleteResponse {
            fv_counts: self.fv_counts.clone(),
            unindexed_fields: self.unindexed_fields.clone(),
        }
    }

    pub fn update(&mut self, file: &File, file_index: &FileIndex) {
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
        let filter_expr = self.filter_expr().as_ref();
        let filter_bitmap = if *filter_expr != FilterExpr::<FilterTarget>::None {
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
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct BucketCompleteResponse {
    // Maps key=value pairs to (unfiltered, filtered) counts
    pub fv_counts: HashMap<FieldValuePair, (usize, usize)>,
    // Set of fields that are not indexed
    pub unindexed_fields: HashSet<FieldName>,
}

/// Type to discriminate partial vs. complete bucket responses.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub enum BucketResponse {
    Partial(BucketPartialResponse),
    Complete(BucketCompleteResponse),
}

impl BucketResponse {
    /// Get all indexed field names from this bucket response.
    pub fn indexed_fields(&self) -> HashSet<FieldName> {
        self.fv_counts()
            .keys()
            .map(|pair| pair.extract_field())
            .collect()
    }

    /// Get the set of unindexed field names.
    pub fn unindexed_fields(&self) -> &HashSet<FieldName> {
        match self {
            BucketResponse::Partial(partial) => &partial.unindexed_fields,
            BucketResponse::Complete(complete) => &complete.unindexed_fields,
        }
    }

    /// Get a reference to the fv_counts HashMap regardless of variant.
    pub fn fv_counts(&self) -> &HashMap<FieldValuePair, (usize, usize)> {
        match self {
            BucketResponse::Partial(partial) => &partial.fv_counts,
            BucketResponse::Complete(complete) => &complete.fv_counts,
        }
    }
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
    pub fn ui_available_histograms(&self) -> Vec<ui::available_histogram::AvailableHistogram> {
        let mut indexed_fields = HashSet::default();

        for (_, bucket) in &self.buckets {
            indexed_fields.extend(bucket.indexed_fields());
        }

        let mut available_histograms = Vec::with_capacity(indexed_fields.len());
        for field_name in indexed_fields {
            let id = field_name.as_str().to_string();
            available_histograms.push(ui::available_histogram::AvailableHistogram {
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

    pub fn ui_chart_result(&self, field: &str) -> ui::histogram::chart::result::Result {
        // Collect all unique values for the field across all buckets
        let mut values = HashSet::default();

        for (_, bucket_response) in &self.buckets {
            for pair in bucket_response.fv_counts().keys() {
                if pair.field() == field {
                    values.insert(pair.value().to_string());
                }
            }
        }

        // Sort values for consistent ordering
        let mut labels: Vec<String> = values.into_iter().collect();
        labels.sort();

        // Build data array
        let mut data = Vec::new();

        for (request, bucket_response) in &self.buckets {
            let timestamp = request.start;
            let mut counts = Vec::with_capacity(labels.len());

            for field_value in &labels {
                // Create FieldValuePair for lookup
                let field_name = FieldName::new_unchecked(field);
                let pair = field_name.with_value(field_value);

                let count = bucket_response
                    .fv_counts()
                    .get(&pair)
                    .map(|(_, filtered)| *filtered)
                    .unwrap_or(0);

                counts.push([count, 0, 0]);
            }

            data.push(ui::histogram::chart::result::DataItem {
                timestamp: timestamp as u64 * std::time::Duration::from_secs(1).as_millis() as u64,
                items: counts,
            });
        }

        let point = ui::histogram::chart::result::Point {
            value: 0,
            arp: 1,
            pa: 2,
        };

        labels.insert(0, String::from("time"));

        ui::histogram::chart::result::Result {
            labels,
            point,
            data,
        }
    }

    pub fn ui_chart_view(
        &self,
        field: &str,
        labels: &[String],
    ) -> ui::histogram::chart::view::View {
        let ids: Vec<String> = labels.iter().skip(1).cloned().collect();
        let names = ids.clone();
        let units = std::iter::repeat_n("events".to_string(), ids.len()).collect();

        let dimensions = ui::histogram::chart::view::Dimensions { ids, names, units };

        ui::histogram::chart::view::View {
            title: format!("Events distribution by {}", field),
            after: self.start_time(),
            before: self.end_time(),
            update_every: self.bucket_duration(),
            units: String::from("units"),
            chart_type: String::from("stackedBar"),
            dimensions,
        }
    }

    pub fn start_time(&self) -> u32 {
        let bucket_request = &self
            .buckets
            .first()
            .expect("histogram with at least one bucket")
            .0;
        bucket_request.start
    }

    pub fn end_time(&self) -> u32 {
        let bucket_request = &self
            .buckets
            .last()
            .expect("histogram with at least one bucket")
            .0;
        bucket_request.end
    }

    pub fn bucket_duration(&self) -> u32 {
        self.buckets
            .first()
            .expect("histogram with at least one bucket")
            .0
            .duration()
    }

    pub fn ui_chart(&self, field: &str) -> ui::histogram::chart::Chart {
        let result = self.ui_chart_result(field);
        let view = self.ui_chart_view(field, &result.labels);

        let mut chart = ui::histogram::chart::Chart { view, result };

        if field == "PRIORITY" {
            chart.patch_priority();
        }

        chart
    }

    pub fn ui_histogram(&self, field: &str) -> ui::histogram::Histogram {
        ui::histogram::Histogram {
            id: String::from(field),
            name: String::from(field),
            chart: self.ui_chart(field),
        }
    }

    pub fn ui_facets(&self) -> Vec<ui::facet::Facet> {
        // Aggregate filtered counts for each field=value pair across all buckets
        let mut field_value_counts: HashMap<FieldValuePair, usize> = HashMap::default();

        for (_, bucket_response) in &self.buckets {
            for (pair, (_unfiltered, filtered)) in bucket_response.fv_counts() {
                *field_value_counts.entry(pair.clone()).or_insert(0) += filtered;
            }
        }

        // Group values by field
        let mut field_to_values: HashMap<String, Vec<(String, usize)>> = HashMap::default();

        for (pair, count) in field_value_counts {
            field_to_values
                .entry(pair.field().to_string())
                .or_default()
                .push((pair.value().to_string(), count));
        }

        // Create facets with sorted fields and options
        let mut facets = Vec::new();
        let mut field_names: Vec<String> = field_to_values.keys().cloned().collect();
        field_names.sort();

        for (order, field_name) in field_names.into_iter().enumerate() {
            let Some(values) = field_to_values.get(&field_name) else {
                eprintln!("Could not find values for field '{}'", field_name);
                continue;
            };
            let mut values = values.clone();
            values.sort_by(|a, b| a.0.cmp(&b.0));

            let options: Vec<ui::facet::Option> = values
                .into_iter()
                .enumerate()
                .map(|(opt_order, (value, count))| ui::facet::Option {
                    id: value.clone(),
                    name: value,
                    order: opt_order,
                    count,
                })
                .collect();

            facets.push(ui::facet::Facet {
                id: field_name.clone(),
                name: field_name.clone(),
                order,
                options,
            });
        }

        facets
    }

    pub fn ui_response(&self, field: &str) -> ui::Response {
        ui::Response {
            facets: self.ui_facets(),
            available_histograms: self.ui_available_histograms(),
            histogram: self.ui_histogram(field),
        }
    }
}
