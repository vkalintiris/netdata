use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::{MetricsService, MetricsServiceServer},
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue};
use opentelemetry_proto::tonic::metrics::v1::{
    metric::Data, AggregationTemporality, Gauge, Histogram, HistogramDataPoint, Metric,
    NumberDataPoint, ResourceMetrics, Sum,
};
use serde_json::{json, Map as JsonMap, Value as JsonValue};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::num::NonZeroU64;
use std::sync::RwLock;
use tonic::{transport::Server, Request, Response, Status};

#[derive(Debug, Clone)]
struct NetdataMetricConfig {
    instance_attributes: Vec<String>,
    dimension_attribute: String,
}

impl NetdataMetricConfig {
    fn new(instance_attributes: Vec<String>, dimension_attribute: String) -> Self {
        Self {
            instance_attributes,
            dimension_attribute,
        }
    }
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
struct SamplePoint {
    unix_time: u64,
    value: f64,
}

impl SamplePoint {
    fn new(unix_time: u64, value: f64) -> Self {
        Self { unix_time, value }
    }
}

#[derive(Copy, Clone, Debug)]
struct CollectionInterval {
    end_time: u64,
    update_every: NonZeroU64,
}

impl CollectionInterval {
    fn from_samples(sample_points: &[SamplePoint]) -> Option<Self> {
        if sample_points.len() < 2 {
            return None;
        }

        let collection_time = sample_points[0].unix_time;
        let mut update_every = u64::MAX;

        for w in sample_points.windows(2) {
            update_every = update_every.min(w[1].unix_time - w[0].unix_time);
        }

        NonZeroU64::new(update_every).map(|update_every| Self {
            end_time: collection_time,
            update_every,
        })
    }

    fn next_interval(&self) -> Self {
        Self {
            end_time: self.end_time + self.update_every.get(),
            update_every: self.update_every,
        }
    }

    fn collection_time(&self) -> u64 {
        self.end_time + self.update_every.get()
    }

    fn is_stale(&self, sp: &SamplePoint) -> bool {
        sp.unix_time < self.end_time
    }

    fn is_on_time(&self, sp: &SamplePoint) -> bool {
        let window = self.update_every.get() / 4;
        let window_start = self.end_time + self.update_every.get() - window;
        let window_end = self.end_time + self.update_every.get() + window;

        sp.unix_time >= window_start && sp.unix_time <= window_end
    }

    fn is_in_gap(&self, sp: &SamplePoint) -> bool {
        !self.is_stale(sp) && !self.is_on_time(sp)
    }

    fn aligned_interval(&self) -> Option<Self> {
        let dur = std::time::Duration::from_nanos(self.end_time);
        let end_time = dur.as_secs() + u64::from(dur.subsec_millis() >= 500);

        let dur = std::time::Duration::from_nanos(self.update_every.get());
        let update_every = dur.as_secs() + u64::from(dur.subsec_millis() >= 500);

        Self::from_secs(end_time, update_every)
    }

    fn from_secs(end_time: u64, update_every: u64) -> Option<Self> {
        let end_time = std::time::Duration::from_secs(end_time).as_nanos() as u64;
        let update_every = std::time::Duration::from_secs(update_every).as_nanos() as u64;

        NonZeroU64::new(update_every).map(|update_every| Self {
            end_time,
            update_every,
        })
    }
}

#[derive(Debug, Default, Clone)]
struct SamplesBuffer(Vec<SamplePoint>);

impl SamplesBuffer {
    fn push(&mut self, sp: SamplePoint) {
        match self.0.binary_search_by_key(&sp.unix_time, |p| p.unix_time) {
            Ok(idx) => self.0[idx] = sp,
            Err(idx) => self.0.insert(idx, sp),
        }
    }

    fn pop(&mut self) -> Option<SamplePoint> {
        if self.0.is_empty() {
            None
        } else {
            Some(self.0.remove(0))
        }
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn first(&self) -> Option<&SamplePoint> {
        self.0.first()
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn drop_stale_samples(&mut self, ci: &CollectionInterval) {
        let split_idx = self
            .0
            .iter()
            .position(|sp| !ci.is_stale(sp))
            .unwrap_or(self.0.len());

        self.0.drain(..split_idx);
    }

    fn collection_interval(&self) -> Option<CollectionInterval> {
        CollectionInterval::from_samples(&self.0)
    }
}

#[derive(Debug, Default)]
struct SamplesTable {
    dimensions: HashMap<String, SamplesBuffer>,
}

impl SamplesTable {
    fn insert(&mut self, dimension: String, sample_point: SamplePoint) {
        self.dimensions
            .entry(dimension)
            .or_default()
            .push(sample_point);
    }

    fn is_empty(&self) -> bool {
        self.dimensions.values().all(|sb| sb.is_empty())
    }

    fn total_samples(&self) -> usize {
        self.dimensions
            .values()
            .map(|sb| sb.len())
            .max()
            .unwrap_or(0)
    }

    fn drop_stale_samples(&mut self, ci: &CollectionInterval) {
        for buffer in self.dimensions.values_mut() {
            buffer.drop_stale_samples(ci);
        }
    }

    fn collection_interval(&self) -> Option<CollectionInterval> {
        self.dimensions
            .values()
            .filter_map(|sb| sb.collection_interval())
            .min_by_key(|ci| ci.collection_time())
    }
}

#[derive(Debug, Default, Clone)]
enum ChartState {
    #[default]
    Uninitialized,
    InGap,
    Initialized,
    Empty,
}

#[derive(Debug)]
struct NetdataChart {
    chart_id: String,
    metric_name: String,
    metric_unit: String,
    metric_type: String,
    samples_table: SamplesTable,
    last_samples_table_interval: Option<CollectionInterval>,
    last_collection_interval: Option<CollectionInterval>,
    chart_state: ChartState,
    samples_threshold: usize,
}

impl NetdataChart {
    fn new(
        chart_id: String,
        metric_name: String,
        metric_unit: String,
        metric_type: String,
    ) -> Self {
        Self {
            chart_id,
            metric_name,
            metric_unit,
            metric_type,
            samples_table: SamplesTable::default(),
            last_samples_table_interval: None,
            last_collection_interval: None,
            chart_state: ChartState::Uninitialized,
            samples_threshold: 3, // Wait for at least 3 samples to detect frequency
        }
    }

    fn ingest(&mut self, dimension_name: String, value: f64, timestamp: u64) {
        let sample_point = SamplePoint::new(timestamp, value);
        self.samples_table.insert(dimension_name, sample_point);
    }

    fn process(&mut self) {
        loop {
            match &self.chart_state {
                ChartState::Uninitialized | ChartState::InGap => {
                    if !self.initialize() {
                        return;
                    }

                    self.emit_chart_definition();
                    self.chart_state = ChartState::Initialized;
                }
                ChartState::Initialized => {
                    self.chart_state = self.process_next_interval();
                }
                ChartState::Empty => {
                    self.chart_state = ChartState::Initialized;
                    return;
                }
            }
        }
    }

    fn initialize(&mut self) -> bool {
        // Clean up stale samples if we have a previous interval
        if let Some(ci) = &self.last_samples_table_interval {
            self.samples_table.drop_stale_samples(ci);
        }

        // Check if we have enough samples to determine frequency
        if self.samples_table.total_samples() < self.samples_threshold {
            return false;
        }

        // Set up collection intervals
        self.last_samples_table_interval =
            self.samples_table
                .collection_interval()
                .map(|ci| CollectionInterval {
                    end_time: ci.end_time - ci.update_every.get(),
                    update_every: ci.update_every,
                });

        self.last_collection_interval = self
            .last_samples_table_interval
            .and_then(|ci| ci.aligned_interval());

        true
    }

    fn process_next_interval(&mut self) -> ChartState {
        let lsti = match &self.last_samples_table_interval {
            Some(interval) => interval,
            None => return ChartState::Empty,
        };

        let lci = match &self.last_collection_interval {
            Some(interval) => interval,
            None => return ChartState::Empty,
        };

        // Clean stale samples
        self.samples_table.drop_stale_samples(lsti);
        if self.samples_table.is_empty() {
            return ChartState::Empty;
        }

        // Check for gaps (all dimensions have samples that are not on time)
        let have_gap = self
            .samples_table
            .dimensions
            .values()
            .all(|buffer| buffer.first().map_or(true, |sp| lsti.is_in_gap(sp)));

        if have_gap {
            return ChartState::InGap;
        }

        // Collect samples to emit
        let mut samples_to_emit = Vec::new();

        for (dimension_name, buffer) in &mut self.samples_table.dimensions {
            if let Some(sp) = buffer.first() {
                if lsti.is_on_time(sp) {
                    if let Some(sample) = buffer.pop() {
                        samples_to_emit.push((dimension_name.clone(), sample.value));
                    }
                }
            }
        }

        // Emit data if we have samples
        if !samples_to_emit.is_empty() {
            self.emit_begin(lci.collection_time());
            for (dimension_name, value) in samples_to_emit {
                self.emit_set(&dimension_name, value);
            }
            self.emit_end();
        }

        // Move to next interval
        self.last_samples_table_interval = Some(lsti.next_interval());
        self.last_collection_interval = Some(lci.next_interval());

        ChartState::Initialized
    }

    fn emit_chart_definition(&self) {
        println!(
            "CHART {} '' '{}' '{}' 'otel' 'otel.{}' line 1 1",
            self.chart_id, self.metric_name, self.metric_unit, self.metric_type
        );

        // Emit dimensions for all known dimension names
        for dimension_name in self.samples_table.dimensions.keys() {
            println!(
                "DIMENSION {} {} absolute 1 1",
                dimension_name, dimension_name
            );
        }
    }

    fn emit_begin(&self, _collection_time: u64) {
        println!("BEGIN {}", self.chart_id);
    }

    fn emit_set(&self, dimension_name: &str, value: f64) {
        println!("SET {} {}", dimension_name, value);
    }

    fn emit_end(&self) {
        println!("END");
    }
}

struct MyMetricsService {
    metric_configs: HashMap<String, NetdataMetricConfig>,
    charts: RwLock<HashMap<String, NetdataChart>>,
}

impl MyMetricsService {
    fn new(metric_configs: HashMap<String, NetdataMetricConfig>) -> Self {
        Self {
            metric_configs,
            charts: RwLock::new(HashMap::new()),
        }
    }

    fn process_flattened_metric(&self, flattened_metric: &JsonValue) -> Result<(), String> {
        // Extract key information from flattened metric
        let metric_hash = flattened_metric["metric.hash"]
            .as_str()
            .ok_or("Missing metric.hash")?;
        let metric_name = flattened_metric["metric.name"]
            .as_str()
            .ok_or("Missing metric.name")?;
        let metric_unit = flattened_metric["metric.unit"].as_str().unwrap_or("count");
        let metric_type = flattened_metric["metric.type"].as_str().unwrap_or("gauge");
        let metric_value = flattened_metric["metric.value"]
            .as_f64()
            .ok_or("Missing or invalid metric.value")?;
        let timestamp = flattened_metric["metric.time_unix_nano"]
            .as_u64()
            .ok_or("Missing metric.time_unix_nano")?;

        // Extract dimension name from metadata or default
        let dimension_name = if let Some(dim_attr) = flattened_metric
            .get("metric.metadata._nd_dimension_attribute")
            .and_then(|v| v.as_str())
        {
            // Look for the dimension attribute value in metric attributes
            let attr_key = format!("metric.attributes.{}", dim_attr);
            flattened_metric
                .get(&attr_key)
                .and_then(|v| v.as_str())
                .unwrap_or("value")
                .to_string()
        } else {
            "value".to_string()
        };

        // Update or create chart
        {
            let mut charts = self
                .charts
                .write()
                .map_err(|_| "Failed to acquire charts lock")?;

            let chart = charts.entry(metric_hash.to_string()).or_insert_with(|| {
                NetdataChart::new(
                    metric_hash.to_string(),
                    metric_name.to_string(),
                    metric_unit.to_string(),
                    metric_type.to_string(),
                )
            });

            // Ingest sample into buffering system
            chart.ingest(dimension_name, metric_value, timestamp);
        }

        Ok(())
    }

    fn process_all_charts(&self) {
        if let Ok(mut charts) = self.charts.write() {
            for chart in charts.values_mut() {
                chart.process();
            }
        }
    }
}

#[tonic::async_trait]
impl MetricsService for MyMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let mut req = request.into_inner();

        // Preprocess metrics to add Netdata metadata
        preprocess_metrics_request(&mut req, &self.metric_configs);

        // Process flattened metrics through chart management
        for flattened_metric in flatten_metrics_request(&req) {
            if false {
                if let Err(e) = self.process_flattened_metric(&flattened_metric) {
                    eprintln!("Error processing metric: {}", e);
                }
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&flattened_metric).unwrap()
                );
            }
        }

        // // Process all charts to handle buffering and emission
        // self.process_all_charts();

        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

fn preprocess_metrics_request(
    req: &mut ExportMetricsServiceRequest,
    metric_configs: &HashMap<String, NetdataMetricConfig>,
) {
    for resource_metrics in &mut req.resource_metrics {
        for scope_metrics in &mut resource_metrics.scope_metrics {
            for metric in &mut scope_metrics.metrics {
                if let Some(config) = metric_configs.get(&metric.name) {
                    annotate_metric_with_netdata_metadata(metric, config);
                }
            }
        }
    }
}

fn annotate_metric_with_netdata_metadata(metric: &mut Metric, config: &NetdataMetricConfig) {
    // Create Netdata metadata as key-value pairs
    let instance_attrs_kv = KeyValue {
        key: "_nd_instance_attributes".to_string(),
        value: Some(AnyValue {
            value: Some(
                opentelemetry_proto::tonic::common::v1::any_value::Value::ArrayValue(
                    opentelemetry_proto::tonic::common::v1::ArrayValue {
                        values: config
                            .instance_attributes
                            .iter()
                            .map(|attr| AnyValue {
                                value: Some(
                                    opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                                        attr.clone(),
                                    ),
                                ),
                            })
                            .collect(),
                    },
                ),
            ),
        }),
    };

    let dimension_attr_kv = KeyValue {
        key: "_nd_dimension_attribute".to_string(),
        value: Some(AnyValue {
            value: Some(
                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(
                    config.dimension_attribute.clone(),
                ),
            ),
        }),
    };

    // Add metadata to the metric's metadata field
    metric.metadata.push(instance_attrs_kv);
    metric.metadata.push(dimension_attr_kv);
}

fn flatten_metrics_request(req: &ExportMetricsServiceRequest) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    for resource_metric in &req.resource_metrics {
        flattened_metrics.extend(flatten_resource_metrics(resource_metric));
    }

    flattened_metrics
}

fn flatten_resource_metrics(resource_metrics: &ResourceMetrics) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();
    let mut base_object = JsonMap::new();

    // Add resource attributes
    if let Some(resource) = &resource_metrics.resource {
        for (key, value) in flatten_key_value_list(&resource.attributes) {
            base_object.insert(format!("resource.attributes.{}", key), value);
        }
        if resource.dropped_attributes_count != 0 {
            base_object.insert(
                "resource.dropped_attributes_count".to_string(),
                JsonValue::Number(resource.dropped_attributes_count.into()),
            );
        }
    }

    // Add scope metrics
    for scope_metrics in &resource_metrics.scope_metrics {
        let mut scope_base = base_object.clone();

        // Add scope info
        if let Some(scope) = &scope_metrics.scope {
            scope_base.insert(
                "scope.name".to_string(),
                JsonValue::String(scope.name.clone()),
            );
            scope_base.insert(
                "scope.version".to_string(),
                JsonValue::String(scope.version.clone()),
            );
            for (key, value) in flatten_key_value_list(&scope.attributes) {
                scope_base.insert(format!("scope.attributes.{}", key), value);
            }
            if scope.dropped_attributes_count != 0 {
                scope_base.insert(
                    "scope.dropped_attributes_count".to_string(),
                    JsonValue::Number(scope.dropped_attributes_count.into()),
                );
            }
        }

        // Add individual metrics
        for metric in &scope_metrics.metrics {
            flattened_metrics.extend(flatten_metric(metric, &scope_base));
        }
    }

    flattened_metrics
}

fn flatten_metric(metric: &Metric, base_object: &JsonMap<String, JsonValue>) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();
    let mut metric_base = base_object.clone();

    // Add metric metadata
    metric_base.insert(
        "metric.name".to_string(),
        JsonValue::String(metric.name.clone()),
    );
    metric_base.insert(
        "metric.description".to_string(),
        JsonValue::String(metric.description.clone()),
    );
    metric_base.insert(
        "metric.unit".to_string(),
        JsonValue::String(metric.unit.clone()),
    );

    // Add metric metadata (including Netdata annotations)
    for (key, value) in flatten_key_value_list(&metric.metadata) {
        metric_base.insert(format!("metric.metadata.{}", key), value);
    }

    // Handle different metric data types
    if let Some(data) = &metric.data {
        match data {
            Data::Gauge(gauge) => {
                flattened_metrics.extend(flatten_gauge(gauge, &metric_base));
            }
            Data::Sum(sum) => {
                flattened_metrics.extend(flatten_sum(sum, &metric_base));
            }
            Data::Histogram(histogram) => {
                flattened_metrics.extend(flatten_histogram(histogram, &metric_base));
            }
            Data::ExponentialHistogram(_) => {
                // Skip exponential histograms for now
            }
            Data::Summary(_) => {
                // Skip summaries for now
            }
        }
    }

    flattened_metrics
}

fn flatten_gauge(gauge: &Gauge, base_object: &JsonMap<String, JsonValue>) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    for data_point in &gauge.data_points {
        let mut point_object = base_object.clone();
        point_object.insert(
            "metric.type".to_string(),
            JsonValue::String("gauge".to_string()),
        );
        flatten_number_data_point(data_point, &mut point_object);
        flattened_metrics.push(flatten_final_object(point_object));
    }

    flattened_metrics
}

fn flatten_sum(sum: &Sum, base_object: &JsonMap<String, JsonValue>) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    let aggregation_temporality = match sum.aggregation_temporality {
        x if x == AggregationTemporality::Unspecified as i32 => "unspecified",
        x if x == AggregationTemporality::Delta as i32 => "delta",
        x if x == AggregationTemporality::Cumulative as i32 => "cumulative",
        _ => "unknown",
    };

    for data_point in &sum.data_points {
        let mut point_object = base_object.clone();
        point_object.insert(
            "metric.type".to_string(),
            JsonValue::String("sum".to_string()),
        );
        point_object.insert(
            "metric.aggregation_temporality".to_string(),
            JsonValue::String(aggregation_temporality.to_string()),
        );
        point_object.insert(
            "metric.is_monotonic".to_string(),
            JsonValue::Bool(sum.is_monotonic),
        );
        flatten_number_data_point(data_point, &mut point_object);
        flattened_metrics.push(flatten_final_object(point_object));
    }

    flattened_metrics
}

fn flatten_histogram(
    histogram: &Histogram,
    base_object: &JsonMap<String, JsonValue>,
) -> Vec<JsonValue> {
    let mut flattened_metrics = Vec::new();

    let aggregation_temporality = match histogram.aggregation_temporality {
        x if x == AggregationTemporality::Unspecified as i32 => "unspecified",
        x if x == AggregationTemporality::Delta as i32 => "delta",
        x if x == AggregationTemporality::Cumulative as i32 => "cumulative",
        _ => "unknown",
    };

    for data_point in &histogram.data_points {
        let mut point_object = base_object.clone();
        point_object.insert(
            "metric.type".to_string(),
            JsonValue::String("histogram".to_string()),
        );
        point_object.insert(
            "metric.aggregation_temporality".to_string(),
            JsonValue::String(aggregation_temporality.to_string()),
        );
        flatten_histogram_data_point(data_point, &mut point_object);
        flattened_metrics.push(flatten_final_object(point_object));
    }

    flattened_metrics
}

fn flatten_number_data_point(
    data_point: &NumberDataPoint,
    point_object: &mut JsonMap<String, JsonValue>,
) {
    // Add attributes
    for (key, value) in flatten_key_value_list(&data_point.attributes) {
        point_object.insert(format!("metric.attributes.{}", key), value);
    }

    // Add timestamps
    point_object.insert(
        "metric.start_time_unix_nano".to_string(),
        JsonValue::Number(data_point.start_time_unix_nano.into()),
    );
    point_object.insert(
        "metric.time_unix_nano".to_string(),
        JsonValue::Number(data_point.time_unix_nano.into()),
    );

    // Add value
    if let Some(value) = &data_point.value {
        match value {
            opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(d) => {
                point_object.insert("metric.value".to_string(), json!(d));
            }
            opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i) => {
                point_object.insert("metric.value".to_string(), JsonValue::Number((*i).into()));
            }
        }
    }

    // Add exemplars count if present
    if !data_point.exemplars.is_empty() {
        point_object.insert(
            "metric.exemplars_count".to_string(),
            JsonValue::Number(data_point.exemplars.len().into()),
        );
    }

    // Add flags
    if data_point.flags != 0 {
        point_object.insert(
            "metric.flags".to_string(),
            JsonValue::Number(data_point.flags.into()),
        );
    }
}

fn flatten_histogram_data_point(
    data_point: &HistogramDataPoint,
    point_object: &mut JsonMap<String, JsonValue>,
) {
    // Add attributes
    for (key, value) in flatten_key_value_list(&data_point.attributes) {
        point_object.insert(format!("metric.attributes.{}", key), value);
    }

    // Add timestamps
    point_object.insert(
        "metric.start_time_unix_nano".to_string(),
        JsonValue::Number(data_point.start_time_unix_nano.into()),
    );
    point_object.insert(
        "metric.time_unix_nano".to_string(),
        JsonValue::Number(data_point.time_unix_nano.into()),
    );

    // Add histogram values
    point_object.insert(
        "metric.count".to_string(),
        JsonValue::Number(data_point.count.into()),
    );
    if let Some(sum) = data_point.sum {
        point_object.insert("metric.sum".to_string(), json!(sum));
    }
    if let Some(min) = data_point.min {
        point_object.insert("metric.min".to_string(), json!(min));
    }
    if let Some(max) = data_point.max {
        point_object.insert("metric.max".to_string(), json!(max));
    }

    // Add bucket counts
    for (i, bucket_count) in data_point.bucket_counts.iter().enumerate() {
        point_object.insert(
            format!("metric.bucket_counts.{}", i),
            JsonValue::Number((*bucket_count).into()),
        );
    }

    // Add explicit bounds
    for (i, bound) in data_point.explicit_bounds.iter().enumerate() {
        point_object.insert(format!("metric.explicit_bounds.{}", i), json!(bound));
    }

    // Add exemplars count if present
    if !data_point.exemplars.is_empty() {
        point_object.insert(
            "metric.exemplars_count".to_string(),
            JsonValue::Number(data_point.exemplars.len().into()),
        );
    }

    // Add flags
    if data_point.flags != 0 {
        point_object.insert(
            "metric.flags".to_string(),
            JsonValue::Number(data_point.flags.into()),
        );
    }
}

fn flatten_key_value_list(kvl: &Vec<KeyValue>) -> JsonMap<String, JsonValue> {
    let mut map = JsonMap::new();
    for kv in kvl {
        if let Some(any_value) = &kv.value {
            map.insert(kv.key.clone(), json_from_any_value(any_value));
        }
    }
    map
}

fn json_from_any_value(any_value: &AnyValue) -> JsonValue {
    if let Some(value) = &any_value.value {
        match value {
            opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(s) => {
                JsonValue::String(s.clone())
            }
            opentelemetry_proto::tonic::common::v1::any_value::Value::BoolValue(b) => {
                JsonValue::Bool(*b)
            }
            opentelemetry_proto::tonic::common::v1::any_value::Value::IntValue(i) => {
                JsonValue::Number((*i).into())
            }
            opentelemetry_proto::tonic::common::v1::any_value::Value::DoubleValue(d) => {
                json!(d)
            }
            opentelemetry_proto::tonic::common::v1::any_value::Value::ArrayValue(arr) => {
                JsonValue::Array(arr.values.iter().map(json_from_any_value).collect())
            }
            opentelemetry_proto::tonic::common::v1::any_value::Value::KvlistValue(kvl) => {
                let mut obj = JsonMap::new();
                for kv in &kvl.values {
                    if let Some(val) = &kv.value {
                        obj.insert(kv.key.clone(), json_from_any_value(val));
                    }
                }
                JsonValue::Object(obj)
            }
            opentelemetry_proto::tonic::common::v1::any_value::Value::BytesValue(b) => {
                use base64::Engine;
                JsonValue::String(base64::engine::general_purpose::STANDARD.encode(b))
            }
        }
    } else {
        JsonValue::Null
    }
}

fn compute_metric_hash(flattened_metric: &JsonMap<String, JsonValue>) -> u64 {
    let mut hasher = DefaultHasher::new();

    // Define fields to exclude from hashing (metadata and temporal/value fields)
    let excluded_prefixes = [
        "metric.metadata.",
        "metric.start_time_unix_nano",
        "metric.time_unix_nano",
        "metric.value",
        "metric.count",
        "metric.sum",
        "metric.min",
        "metric.max",
        "metric.bucket_counts.",
        "metric.explicit_bounds.",
        "metric.exemplars_count",
        "metric.flags",
    ];

    // Collect and sort keys to ensure consistent hashing
    let mut hash_fields: Vec<(&String, &JsonValue)> = flattened_metric
        .iter()
        .filter(|(key, _)| {
            // Include all fields except those with excluded prefixes
            !excluded_prefixes
                .iter()
                .any(|prefix| key.starts_with(prefix))
        })
        .collect();

    // Sort by key for deterministic hashing
    hash_fields.sort_by_key(|(key, _)| *key);

    // Hash each included field
    for (key, value) in hash_fields {
        key.hash(&mut hasher);
        hash_json_value(value, &mut hasher);
    }

    hasher.finish()
}

fn hash_json_value(value: &JsonValue, hasher: &mut DefaultHasher) {
    match value {
        JsonValue::Null => 0u8.hash(hasher),
        JsonValue::Bool(b) => b.hash(hasher),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.hash(hasher);
            } else if let Some(u) = n.as_u64() {
                u.hash(hasher);
            } else if let Some(f) = n.as_f64() {
                f.to_bits().hash(hasher);
            }
        }
        JsonValue::String(s) => s.hash(hasher),
        JsonValue::Array(arr) => {
            arr.len().hash(hasher);
            for item in arr {
                hash_json_value(item, hasher);
            }
        }
        JsonValue::Object(obj) => {
            obj.len().hash(hasher);
            let mut sorted_items: Vec<_> = obj.iter().collect();
            sorted_items.sort_by_key(|(k, _)| *k);
            for (key, val) in sorted_items {
                key.hash(hasher);
                hash_json_value(val, hasher);
            }
        }
    }
}

fn flatten_final_object(object: JsonMap<String, JsonValue>) -> JsonValue {
    let mut flattened = flatten_serde_json::flatten(&object);

    // Compute and add metric hash
    let metric_hash = compute_metric_hash(&flattened);
    flattened.insert(
        "metric.hash".to_string(),
        JsonValue::String(format!("{:016x}", metric_hash)),
    );

    JsonValue::Object(flattened)
}

fn create_default_metric_configs() -> HashMap<String, NetdataMetricConfig> {
    let mut configs = HashMap::new();

    // Example: system.cpu.time metric
    configs.insert(
        "system.cpu.time".to_string(),
        NetdataMetricConfig::new(vec!["cpu".to_string()], "state".to_string()),
    );

    // Example: system.memory.usage metric
    configs.insert(
        "system.memory.usage".to_string(),
        NetdataMetricConfig::new(vec![], "state".to_string()),
    );

    // Example: system.disk.io metric
    configs.insert(
        "system.disk.io".to_string(),
        NetdataMetricConfig::new(vec!["device".to_string()], "direction".to_string()),
    );

    // Example: system.network.io metric
    configs.insert(
        "system.network.io".to_string(),
        NetdataMetricConfig::new(vec!["device".to_string()], "direction".to_string()),
    );

    // Example: process.cpu.time metric
    configs.insert(
        "process.cpu.time".to_string(),
        NetdataMetricConfig::new(vec!["pid".to_string()], "state".to_string()),
    );

    configs
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:21212".parse()?;
    let metrics_service = MyMetricsService::new(create_default_metric_configs());

    println!("OTEL Metrics Receiver (nom) listening on {}", addr);
    println!(
        "Loaded {} metric configurations",
        metrics_service.metric_configs.len()
    );

    Server::builder()
        .add_service(
            MetricsServiceServer::new(metrics_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await?;

    Ok(())
}
