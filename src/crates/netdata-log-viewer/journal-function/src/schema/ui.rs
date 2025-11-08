//! UI response types for rendering journal data in the Netdata dashboard.
//!
//! This module provides types for converting histogram responses into UI-friendly formats,
//! including facets, charts, and data points formatted for the Netdata dashboard.

#[cfg(feature = "allocative")]
use allocative::Allocative;
use crate::histogram::HistogramResponse;  // HistogramResponse is from parent crate
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const PRIORITY_LABELS: &[(&str, &str)] = &[
    ("0", "emergency"),
    ("1", "alert"),
    ("2", "critical"),
    ("3", "error"),
    ("4", "warning"),
    ("5", "notice"),
    ("6", "info"),
    ("7", "debug"),
];

/// Helper function to remap strings using a mapping table.
fn remap_strings(vec: &mut [String], map: &HashMap<&str, &str>) {
    for s in vec.iter_mut() {
        if let Some(&new_value) = map.get(s.as_str()) {
            *s = new_value.to_string();
        }
    }
}

// ============================================================================
// UI Response Types (flat structure)
// ============================================================================

/// Top-level response containing facets, available histograms, and a histogram.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Response {
    pub facets: Vec<Facet>,
    pub available_histograms: Vec<AvailableHistogram>,
    pub histogram: Histogram,
}

/// Represents an available histogram option.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct AvailableHistogram {
    pub id: String,
    pub name: String,
    pub order: usize,
}

/// A facet represents a field with multiple value options.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Facet {
    pub id: String,
    pub name: String,
    pub order: usize,
    pub options: Vec<FacetOption>,
}

/// A single option within a facet.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FacetOption {
    pub id: String,
    pub name: String,
    pub order: usize,
    pub count: usize,
}

/// A histogram for a specific field.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Histogram {
    pub id: String,
    pub name: String,
    pub chart: Chart,
}

/// A chart containing view metadata and result data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Chart {
    pub view: ChartView,
    pub result: ChartResult,
}

impl Chart {
    /// Patches priority field labels from numeric to human-readable.
    pub fn patch_priority(&mut self) {
        let map: HashMap<_, _> = PRIORITY_LABELS.iter().copied().collect();
        remap_strings(&mut self.view.dimensions.ids, &map);
        remap_strings(&mut self.view.dimensions.names, &map);
        remap_strings(&mut self.result.labels, &map);
    }
}

/// Chart view metadata describing how to display the chart.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct ChartView {
    pub title: String,
    pub after: u32,
    pub before: u32,
    pub update_every: u32,
    pub units: String,
    pub chart_type: String,
    pub dimensions: ChartDimensions,
}

/// Dimensions for the chart view.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct ChartDimensions {
    pub ids: Vec<String>,
    pub names: Vec<String>,
    pub units: Vec<String>,
}

/// Chart result data containing labels, point metadata, and time series data.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct ChartResult {
    pub labels: Vec<String>,
    pub point: ChartPoint,
    pub data: Vec<DataPoint>,
}

/// Point metadata for chart result.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct ChartPoint {
    pub value: u64,
    pub arp: u64,
    pub pa: u64,
}

/// A single data point in a time series.
#[derive(Debug)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct DataPoint {
    pub timestamp: u64,
    pub items: Vec<[usize; 3]>,
}

/// Custom serialization for DataPoint to flatten the structure.
///
/// Serialize as: [timestamp, [val1, arp1, pa1], [val2, arp2, pa2], ...]
/// This format matches the expected Netdata chart data format where the first
/// element is the timestamp followed by dimension data arrays.
impl Serialize for DataPoint {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeSeq;

        // Create a sequence with length = 1 (timestamp) + number of items
        let mut seq = serializer.serialize_seq(Some(1 + self.items.len()))?;

        // First element: timestamp
        seq.serialize_element(&self.timestamp)?;

        // Remaining elements: each [usize; 3] array
        for item in &self.items {
            seq.serialize_element(item)?;
        }

        seq.end()
    }
}

impl<'de> Deserialize<'de> for DataPoint {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{SeqAccess, Visitor};

        struct DataPointVisitor;

        impl<'de> Visitor<'de> for DataPointVisitor {
            type Value = DataPoint;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("an array with timestamp followed by data items")
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                // First element: timestamp
                let timestamp = seq
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;

                // Remaining elements: collect all [usize; 3] arrays
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }

                Ok(DataPoint { timestamp, items })
            }
        }

        deserializer.deserialize_seq(DataPointVisitor)
    }
}

// ============================================================================
// Public API for constructing UI types from HistogramResponse
// ============================================================================

impl Response {
    /// Creates a complete Response from a HistogramResponse for the given field.
    ///
    /// # Arguments
    /// * `histogram_response` - The histogram response to convert
    /// * `field` - The field to generate the histogram for
    pub fn from_histogram(
        histogram_response: &HistogramResponse,
        field: &journal::FieldName,
    ) -> Self {
        Self {
            facets: facets(histogram_response),
            available_histograms: available_histograms(histogram_response),
            histogram: histogram(histogram_response, field),
        }
    }
}

/// Creates a list of facets from a HistogramResponse.
///
/// Aggregates field=value counts across all buckets and groups them by field.
pub fn facets(histogram_response: &HistogramResponse) -> Vec<Facet> {
    use journal::FieldValuePair;
    use journal::collections::HashMap;

    // Aggregate filtered counts for each field=value pair across all buckets
    let mut field_value_counts: HashMap<FieldValuePair, usize> = HashMap::default();

    for (_, bucket_response) in &histogram_response.buckets {
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
            continue;
        };
        let mut values = values.clone();
        values.sort_by(|a, b| a.0.cmp(&b.0));

        let options: Vec<FacetOption> = values
            .into_iter()
            .enumerate()
            .map(|(opt_order, (value, count))| FacetOption {
                id: value.clone(),
                name: value,
                order: opt_order,
                count,
            })
            .collect();

        facets.push(Facet {
            id: field_name.clone(),
            name: field_name.clone(),
            order,
            options,
        });
    }

    facets
}

/// Creates a list of available histograms from a HistogramResponse.
///
/// Returns one available histogram for each indexed field found in the buckets.
pub fn available_histograms(
    histogram_response: &HistogramResponse,
) -> Vec<AvailableHistogram> {
    use journal::collections::HashSet;

    let mut indexed_fields = HashSet::default();

    for (_, bucket) in &histogram_response.buckets {
        indexed_fields.extend(bucket.indexed_fields());
    }

    let mut available_histograms = Vec::with_capacity(indexed_fields.len());
    for field_name in indexed_fields {
        let id = field_name.as_str().to_string();
        available_histograms.push(AvailableHistogram {
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

/// Creates a Histogram for the given field from a HistogramResponse.
///
/// # Arguments
/// * `histogram_response` - The histogram response to convert
/// * `field` - The field to generate the histogram for
pub fn histogram(
    histogram_response: &HistogramResponse,
    field: &journal::FieldName,
) -> Histogram {
    let field_str = field.as_str();
    Histogram {
        id: String::from(field_str),
        name: String::from(field_str),
        chart: chart_from_histogram(histogram_response, field),
    }
}

/// Creates a Chart for the given field from a HistogramResponse.
fn chart_from_histogram(
    histogram_response: &HistogramResponse,
    field: &journal::FieldName,
) -> Chart {
    let result = chart_result_from_histogram(histogram_response, field);
    let view = chart_view_from_histogram(histogram_response, field, &result.labels);

    let mut chart = Chart { view, result };

    if field.as_str() == "PRIORITY" {
        chart.patch_priority();
    }

    chart
}

/// Creates chart result data for the given field from a HistogramResponse.
fn chart_result_from_histogram(
    histogram_response: &HistogramResponse,
    field: &journal::FieldName,
) -> ChartResult {
    use journal::collections::HashSet;

    let field_str = field.as_str();

    // Collect all unique values for the field across all buckets
    let mut values = HashSet::default();

    for (_, bucket_response) in &histogram_response.buckets {
        for pair in bucket_response.fv_counts().keys() {
            if pair.field() == field_str {
                values.insert(pair.value().to_string());
            }
        }
    }

    // Sort values for consistent ordering
    let mut labels: Vec<String> = values.into_iter().collect();
    labels.sort();

    // Build data array
    let mut data = Vec::new();

    for (request, bucket_response) in &histogram_response.buckets {
        let timestamp = request.start;
        let mut counts = Vec::with_capacity(labels.len());

        for field_value in &labels {
            // Create FieldValuePair for lookup
            let pair = field.with_value(field_value);

            let count = bucket_response
                .fv_counts()
                .get(&pair)
                .map(|(_, filtered)| *filtered)
                .unwrap_or(0);

            counts.push([count, 0, 0]);
        }

        data.push(DataPoint {
            timestamp: timestamp as u64 * std::time::Duration::from_secs(1).as_millis() as u64,
            items: counts,
        });
    }

    let point = ChartPoint {
        value: 0,
        arp: 1,
        pa: 2,
    };

    labels.insert(0, String::from("time"));

    ChartResult {
        labels,
        point,
        data,
    }
}

/// Creates chart view metadata for the given field from a HistogramResponse.
fn chart_view_from_histogram(
    histogram_response: &HistogramResponse,
    field: &journal::FieldName,
    labels: &[String],
) -> ChartView {
    let ids: Vec<String> = labels.iter().skip(1).cloned().collect();
    let names = ids.clone();
    let units = std::iter::repeat_n("events".to_string(), ids.len()).collect();

    let dimensions = ChartDimensions { ids, names, units };

    ChartView {
        title: format!("Events distribution by {}", field.as_str()),
        after: histogram_response.start_time(),
        before: histogram_response.end_time(),
        update_every: histogram_response.bucket_duration(),
        units: String::from("units"),
        chart_type: String::from("stackedBar"),
        dimensions,
    }
}
