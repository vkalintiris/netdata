//! gRPC service implementation for OTLP metrics ingestion.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    metrics_service_server::MetricsService,
};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tonic::{Request, Response, Status};

use crate::chart::{Chart, ChartConfig, FinalizedDimension};
use crate::config::ChartConfigManager;
use crate::iter::DataPointContextIterExt;
use crate::otel;
use crate::output::{DebugOutput, DimensionValue, MetricsOutput};

/// Manages all charts for the service.
pub struct ChartManager {
    charts: HashMap<String, Chart>,
    config: ChartConfig,
}

impl ChartManager {
    pub fn new(config: ChartConfig) -> Self {
        Self {
            charts: HashMap::new(),
            config,
        }
    }

    /// Get or create a chart for the given data point context.
    /// Returns None if the metric type is not supported.
    pub fn get_or_create_chart(
        &mut self,
        chart_name: &str,
        dp: &crate::iter::DataPointContext<'_>,
    ) -> Option<&mut Chart> {
        if !self.charts.contains_key(chart_name) {
            let data_kind = dp.data_kind()?;
            let temporality = dp.aggregation_temporality();

            let chart = Chart::from_metric(data_kind, temporality, self.config)?;

            self.charts.insert(chart_name.to_string(), chart);
        }

        self.charts.get_mut(chart_name)
    }

    /// Trigger tick-based finalization for all charts, emitting any finalized slots.
    /// Returns the number of charts that were finalized.
    pub fn tick_all_and_emit(
        &mut self,
        dimensions: &mut Vec<FinalizedDimension>,
        output: &mut impl MetricsOutput,
    ) -> usize {
        let mut count = 0;
        for (name, chart) in &mut self.charts {
            if let Some(slot_timestamp) = chart.tick(dimensions) {
                emit_dimensions(output, name, slot_timestamp, dimensions);
                count += 1;
            }
        }
        count
    }
}

/// Build dimension values and emit via the output trait.
fn emit_dimensions(
    output: &mut impl MetricsOutput,
    chart_name: &str,
    slot_timestamp: u64,
    dimensions: &[FinalizedDimension],
) {
    let dim_values: Vec<DimensionValue<'_>> = dimensions
        .iter()
        .map(|d| DimensionValue {
            name: &d.name,
            value: d.value,
        })
        .collect();

    output.emit_update(chart_name, slot_timestamp, &dim_values);
}

/// Handle for the background tick task.
pub struct TickTaskHandle {
    handle: JoinHandle<()>,
}

impl TickTaskHandle {
    /// Abort the background tick task.
    pub fn abort(&self) {
        self.handle.abort();
    }
}

/// Spawn a background task that periodically calls tick on the chart manager.
///
/// The tick interval determines how often we check for slots that have passed
/// their grace period.
pub fn spawn_tick_task(
    chart_manager: Arc<RwLock<ChartManager>>,
    tick_interval: Duration,
) -> TickTaskHandle {
    let handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(tick_interval);
        let mut dimensions = Vec::new();
        let mut output = DebugOutput;

        loop {
            interval.tick().await;

            let mut manager = chart_manager.write().await;
            let count = manager.tick_all_and_emit(&mut dimensions, &mut output);

            if count > 0 {
                println!("Tick finalized {} charts", count);
            }
        }
    });

    TickTaskHandle { handle }
}

pub struct NetdataMetricsService {
    pub chart_config_manager: Arc<RwLock<ChartConfigManager>>,
    pub chart_manager: Arc<RwLock<ChartManager>>,
}

impl NetdataMetricsService {
    pub fn new(
        chart_config_manager: Arc<RwLock<ChartConfigManager>>,
        chart_manager: Arc<RwLock<ChartManager>>,
    ) -> Self {
        Self {
            chart_config_manager,
            chart_manager,
        }
    }

    async fn process_request(&self, req: &mut ExportMetricsServiceRequest) {
        otel::normalize_request(req);

        let ccm = self.chart_config_manager.read().await;
        let mut chart_manager = self.chart_manager.write().await;
        let mut chart_name_buf = String::with_capacity(128);
        let mut finalized = Vec::new();
        let mut output = DebugOutput;

        for dp in req.datapoint_iter(&ccm) {
            // Skip non-number data points (histograms, etc.)
            let Some(value) = dp.datapoint_ref.value_as_f64() else {
                continue;
            };

            let dimension_name = dp.dimension_name();
            let attrs_hash = dp.attrs_hash();
            let timestamp_ns = dp.datapoint_ref.time_unix_nano();
            let start_time_ns = dp.datapoint_ref.start_time_unix_nano();

            // Build chart name
            chart_name_buf.clear();
            let _ = write!(
                &mut chart_name_buf,
                "{}.{}",
                dp.metric_ref.metric.name, attrs_hash
            );

            // Get or create the chart
            let Some(chart) = chart_manager.get_or_create_chart(&chart_name_buf, &dp) else {
                // Unsupported metric type
                continue;
            };

            // Ingest the data point - may finalize the previous slot if this
            // data belongs to a newer slot
            if let Some(slot_timestamp) = chart.ingest(
                dimension_name,
                value,
                timestamp_ns,
                start_time_ns,
                &mut finalized,
            ) {
                emit_dimensions(&mut output, &chart_name_buf, slot_timestamp, &finalized);
            }
        }
    }
}

impl Default for NetdataMetricsService {
    fn default() -> Self {
        Self {
            chart_config_manager: Arc::new(RwLock::new(ChartConfigManager::with_default_configs())),
            chart_manager: Arc::new(RwLock::new(ChartManager::new(ChartConfig::default()))),
        }
    }
}

#[tonic::async_trait]
impl MetricsService for NetdataMetricsService {
    async fn export(
        &self,
        request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        let mut req = request.into_inner();

        self.process_request(&mut req).await;

        Ok(Response::new(ExportMetricsServiceResponse {
            partial_success: None,
        }))
    }
}
