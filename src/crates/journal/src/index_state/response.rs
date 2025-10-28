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

    // Maps field=value pairs to (unfiltered, filtered) counts
    pub fv_counts: HashMap<String, (usize, usize)>,

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

        // Add any missing unindexed fields to the bucket
        for field in file_index.fields() {
            if file_index.is_indexed(field) {
                continue;
            }

            // FIXME: rethink this
            self.unindexed_fields.insert(field.clone());
        }

        // TODO: should `resolve`/`evaluate` return an `Option`?
        let filter_expr = self.filter_expr().as_ref();
        let filter_bitmap = if *filter_expr != FilterExpr::<String>::None {
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
            if let Some(counts) = self.fv_counts.get_mut(indexed_field) {
                counts.0 += unfiltered_count;
                counts.1 += filtered_count;
            } else {
                self.fv_counts
                    .insert(indexed_field.clone(), (unfiltered_count, filtered_count));
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
    pub fv_counts: HashMap<String, (usize, usize)>,
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
        self.fv_counts
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
        self.fv_counts
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
    pub fn ui_available_histograms(&self) -> Vec<ui::available_histogram::AvailableHistogram> {
        let mut indexed_fields = HashSet::default();

        for (_, bucket) in &self.buckets {
            indexed_fields.extend(bucket.indexed_fields());
        }

        let mut available_histograms = Vec::with_capacity(indexed_fields.len());
        for id in indexed_fields {
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
            match bucket_response {
                BucketResponse::Partial(partial) => {
                    for fv in partial.fv_counts.keys() {
                        if let Some((f, v)) = fv.split_once('=') {
                            if f == field {
                                values.insert(v.to_string());
                            }
                        }
                    }
                }
                BucketResponse::Complete(complete) => {
                    for fv in complete.fv_counts.keys() {
                        if let Some((f, v)) = fv.split_once('=') {
                            if f == field {
                                values.insert(v.to_string());
                            }
                        }
                    }
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
                let key = format!("{}={}", field, field_value);

                let count = match bucket_response {
                    BucketResponse::Partial(partial) => partial
                        .fv_counts
                        .get(&key)
                        .map(|(_, filtered)| *filtered)
                        .unwrap_or(0),
                    BucketResponse::Complete(complete) => complete
                        .fv_counts
                        .get(&key)
                        .map(|(_, filtered)| *filtered)
                        .unwrap_or(0),
                };

                counts.push([count, 0, 0]);
            }

            data.push(ui::histogram::chart::result::DataItem {
                timestamp: timestamp * std::time::Duration::from_secs(1).as_millis() as u64,
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
        let units = std::iter::repeat("events".to_string())
            .take(ids.len())
            .collect();

        let dimensions = ui::histogram::chart::view::Dimensions { ids, names, units };

        ui::histogram::chart::view::View {
            title: format!("Events distribution by {}", field),
            after: self.buckets.first().unwrap().0.start as u32,
            before: self.buckets.last().unwrap().0.end as u32,
            update_every: self.buckets.first().unwrap().0.duration() as u32,
            units: String::from("units"),
            chart_type: String::from("stackedBar"),
            dimensions,
        }
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
        let mut field_value_counts: HashMap<String, usize> = HashMap::default();

        for (_, bucket_response) in &self.buckets {
            let fv_counts = match bucket_response {
                BucketResponse::Partial(partial) => &partial.fv_counts,
                BucketResponse::Complete(complete) => &complete.fv_counts,
            };

            for (fv, (_unfiltered, filtered)) in fv_counts {
                *field_value_counts.entry(fv.clone()).or_insert(0) += filtered;
            }
        }

        // Group values by field
        let mut field_to_values: HashMap<String, Vec<(String, usize)>> = HashMap::default();

        for (fv, count) in field_value_counts {
            if let Some((f, v)) = fv.split_once('=') {
                field_to_values
                    .entry(f.to_string())
                    .or_insert_with(Vec::new)
                    .push((v.to_string(), count));
            }
        }

        // Create facets with sorted fields and options
        let mut facets = Vec::new();
        let mut field_names: Vec<String> = field_to_values.keys().cloned().collect();
        field_names.sort();

        for (order, field_name) in field_names.iter().enumerate() {
            let mut values = field_to_values.get(field_name).unwrap().clone();
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
