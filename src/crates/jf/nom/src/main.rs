use flatten_otel::flatten_metrics_request;
use opentelemetry_proto::tonic::collector::metrics::v1::{
    metrics_service_server::{MetricsService, MetricsServiceServer},
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use serde_json::{Map as JsonMap, Value as JsonValue};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::RwLock;
use tonic::{transport::Server, Request, Response, Status};

use regex::Regex;
use std::sync::{Arc, Mutex};

mod utils;

use std::hash::{Hash, Hasher};

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

    fn ingest(&mut self, dimension_name: String, timestamp: u64, value: f64) {
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

#[derive(Default)]
struct MyMetricsService {
    regex_cache: RegexCache,
    charts: Arc<RwLock<HashMap<String, NetdataChart>>>,
}

// impl MyMetricsService {
//     fn _process_flattened_metric(&self, flattened_metric: &JsonValue) -> Result<(), String> {
//         // Extract key information from flattened metric
//         let metric_hash = flattened_metric["metric.hash"]
//             .as_str()
//             .ok_or("Missing metric.hash")?;
//         let metric_name = flattened_metric["metric.name"]
//             .as_str()
//             .ok_or("Missing metric.name")?;
//         let metric_unit = flattened_metric["metric.unit"].as_str().unwrap_or("count");
//         let metric_type = flattened_metric["metric.type"].as_str().unwrap_or("gauge");
//         let metric_value = flattened_metric["metric.value"]
//             .as_f64()
//             .ok_or("Missing or invalid metric.value")?;
//         let timestamp = flattened_metric["metric.time_unix_nano"]
//             .as_u64()
//             .ok_or("Missing metric.time_unix_nano")?;

//         // Extract dimension name from metadata or default
//         let dimension_name = if let Some(dim_attr) = flattened_metric
//             .get("metric.metadata._nd_dimension_attribute")
//             .and_then(|v| v.as_str())
//         {
//             // Look for the dimension attribute value in metric attributes
//             let attr_key = format!("metric.attributes.{}", dim_attr);
//             flattened_metric
//                 .get(&attr_key)
//                 .and_then(|v| v.as_str())
//                 .unwrap_or("value")
//                 .to_string()
//         } else {
//             "value".to_string()
//         };

//         // Update or create chart
//         {
//             let mut charts = self
//                 ._charts
//                 .write()
//                 .map_err(|_| "Failed to acquire charts lock")?;

//             let chart = charts.entry(metric_hash.to_string()).or_insert_with(|| {
//                 NetdataChart::new(
//                     metric_hash.to_string(),
//                     metric_name.to_string(),
//                     metric_unit.to_string(),
//                     metric_type.to_string(),
//                 )
//             });

//             // Ingest sample into buffering system
//             chart.ingest(dimension_name, metric_value, timestamp);
//         }

//         Ok(())
//     }

//     fn _process_all_charts(&self) {
//         if let Ok(mut charts) = self._charts.write() {
//             for chart in charts.values_mut() {
//                 chart.process();
//             }
//         }
//     }
// }

#[derive(Default, Debug)]
pub struct RegexCache {
    cache: Arc<Mutex<HashMap<String, Regex>>>,
}

impl RegexCache {
    pub fn new() -> Self {
        Self {
            cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn get_regex(&self, pattern: &str) -> Result<Regex, regex::Error> {
        let mut cache = self.cache.lock().unwrap();

        if let Some(regex) = cache.get(pattern) {
            return Ok(regex.clone());
        }

        let compiled_regex = Regex::new(pattern)?;
        cache.insert(pattern.to_string(), compiled_regex.clone());
        Ok(compiled_regex)
    }
}

#[derive(Default)]
struct FlattenedMetric {
    json_map: JsonMap<String, JsonValue>,
    nd_instance_pattern: Option<String>,
    nd_dimension_key: Option<String>,

    metric_name: String,
    metric_unit: String,
    metric_type: String,

    metric_time_unix_nano: u64,
    metric_value: f64,
}

impl FlattenedMetric {
    pub fn new(mut json_map: JsonMap<String, JsonValue>, regex_cache: &RegexCache) -> Option<Self> {
        let metric_name = match json_map.remove("metric.name").unwrap() {
            JsonValue::String(s) => s,
            _ => return None,
        };
        let metric_unit = match json_map.remove("metric.unit").unwrap() {
            JsonValue::String(s) => s,
            _ => return None,
        };
        let metric_type = match json_map.remove("metric.type").unwrap() {
            JsonValue::String(s) => s,
            _ => return None,
        };
        let metric_time_unix_nano = match json_map.remove("metric.time_unix_nano").unwrap() {
            JsonValue::Number(n) => n.as_u64()?,
            _ => return None,
        };
        let metric_value = match json_map.remove("metric.value").unwrap() {
            JsonValue::Number(n) => n.as_f64()?,
            _ => return None,
        };

        // TODO: we need the instance/dimension strings.
        // Handle the case where the annotations are missing or invalid.

        let nd_instance_pattern = match json_map.remove("metric.attributes._nd_chart_instance") {
            Some(JsonValue::String(s)) => regex_cache.get_regex(&s).unwrap(),
            _ => return None,
        };

        let nd_dimension_key = match json_map.remove("metric.attributes._nd_dimension") {
            Some(JsonValue::String(s)) => Some(s),
            _ => return None,
        };

        Some(Self {
            json_map,
            nd_instance_pattern,
            nd_dimension_key,
            metric_name,
            metric_unit,
            metric_type,
            metric_time_unix_nano,
            metric_value,
        })
    }

    pub fn instance(
        &self,
        regex_cache: &RegexCache,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let Some(pattern) = self.nd_instance_pattern.as_ref() else {
            return Ok(None);
        };
        let regex = regex_cache.get_regex(pattern)?;

        let metric_name = self
            .json_map
            .get("metric.name")
            .expect("a valid metric name")
            .as_str()
            .expect("a string metric name");

        let mut matched_values = Vec::new();
        for (key, value) in &self.json_map {
            if regex.is_match(key) {
                let value_str = match value {
                    JsonValue::String(s) => s.clone(),
                    JsonValue::Number(n) => n.to_string(),
                    JsonValue::Bool(b) => b.to_string(),
                    JsonValue::Null => "null".to_string(),
                    _ => serde_json::to_string(value).unwrap_or_default(),
                };
                matched_values.push(value_str);
            }
        }

        let suffix = matched_values.join(".");
        Ok(Some(format!("{}.{}", metric_name, suffix)))
    }

    pub fn dimension(&self) -> Result<Option<String>, Box<dyn std::error::Error>> {
        let Some(key) = self.nd_dimension_key.as_ref() else {
            return Ok(None);
        };

        let Some(value) = self.json_map.get(key) else {
            return Ok(None);
        };

        let s = match value {
            JsonValue::String(s) => s.clone(),
            JsonValue::Number(n) => n.to_string(),
            JsonValue::Bool(b) => b.to_string(),
            JsonValue::Null => "null".to_string(),
            _ => serde_json::to_string(value).unwrap_or_default(),
        };

        Ok(Some(s))
    }

    pub fn metric_name(&self) -> &str {
        self.json_map.get("metric.name").unwrap().as_str().unwrap()
    }

    pub fn metric_unit(&self) -> &str {
        self.json_map.get("metric.unit").unwrap().as_str().unwrap()
    }

    pub fn metric_type(&self) -> &str {
        self.json_map.get("metric.type").unwrap().as_str().unwrap()
    }

    pub fn metric_time_unix_nano(&self) -> u64 {
        self.json_map
            .get("metric.time_unix_nano")
            .unwrap()
            .as_u64()
            .unwrap()
    }

    pub fn metric_value(&self) -> f64 {
        self.json_map.get("metric.value").unwrap().as_f64().unwrap()
    }
}

impl Hash for FlattenedMetric {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for (key, value) in self.json_map.iter() {
            if key == "metric.start_time_unix_nano" {
                continue;
            }
            if key == "metric.time_unix_nano" {
                continue;
            }
            if key == "metric.value" {
                continue;
            }
            if Some(key) == self.nd_dimension_key.as_ref() {
                continue;
            }

            key.hash(state);
            value.hash(state);
        }
    }
}

impl std::fmt::Debug for FlattenedMetric {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut debug_struct = f.debug_struct("FlattenedMetric");

        // Format the JSON map as pretty JSON
        match serde_json::to_string_pretty(&self.json_map) {
            Ok(pretty_json) => {
                debug_struct.field("json_map", &format_args!("\n{}", pretty_json));
            }
            Err(_) => {
                debug_struct.field("json_map", &"<invalid JSON>");
            }
        }

        debug_struct
            .field("nd_instance_pattern", &self.nd_instance_pattern)
            .field("nd_dimension_key", &self.nd_dimension_key)
            .finish()
    }
}

#[tonic::async_trait]
impl MetricsService for MyMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let req = request.into_inner();

        let flattened_metrics = flatten_metrics_request(&req)
            .into_iter()
            .map(FlattenedMetric::new);

        let filtered_metrics = flattened_metrics.filter(|fm| fm.metric_name() == "system.cpu.time");

        for fm in filtered_metrics.clone() {
            let mut guard = self.charts.write().unwrap();

            let chart_id = fm.instance(&self.regex_cache).unwrap().unwrap();

            let netdata_chart = guard.entry(chart_id.clone()).or_insert_with(|| {
                let chart = NetdataChart::new(
                    chart_id.clone(),
                    String::from(fm.metric_name()),
                    String::from(fm.metric_unit()),
                    String::from(fm.metric_type()),
                );
                println!("Chart {:#?}", chart);

                chart
            });

            let dimension_name = fm.dimension().unwrap().unwrap();
            let value = fm.metric_value();
            let timestamp = fm.metric_time_unix_nano();

            netdata_chart.ingest(dimension_name, timestamp, value);
        }

        {
            let guard = self.charts.write().unwrap();
            for (idx, key) in guard.keys().enumerate() {}

            println!();
        }

        // hashing?

        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr = "127.0.0.1:21212".parse()?;
    let metrics_service = MyMetricsService::default();

    println!("OTEL Metrics Receiver listening on {}", addr);

    Server::builder()
        .add_service(
            MetricsServiceServer::new(metrics_service)
                .accept_compressed(tonic::codec::CompressionEncoding::Gzip),
        )
        .serve(addr)
        .await?;

    Ok(())
}
