//! gRPC service implementation for OTLP metrics ingestion.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
    metrics_service_server::MetricsService,
};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tonic::{Request, Response, Status};
use twox_hash::XxHash64;

use crate::chart::{Chart, ChartConfig};
use crate::config::ChartConfigManager;
use crate::iter::DataPointContextIterExt;
use crate::otel;
use crate::slot::{DimensionId, FinalizedSlot};
use std::fmt::Write;

/// State for a chart including dimension name mappings.
pub struct ChartState {
    pub chart: Chart,
    /// Map from dimension ID to dimension name (for output)
    dimension_names: HashMap<DimensionId, String>,
}

impl ChartState {
    fn new(chart: Chart) -> Self {
        Self {
            chart,
            dimension_names: HashMap::new(),
        }
    }

    /// Get or create the dimension ID for a dimension name.
    pub fn dimension_id(&mut self, name: &str) -> DimensionId {
        let mut hasher = XxHash64::default();
        name.hash(&mut hasher);
        let id = hasher.finish();

        self.dimension_names
            .entry(id)
            .or_insert_with(|| name.to_string());

        id
    }

    /// Get the dimension name for an ID.
    pub fn dimension_name(&self, id: DimensionId) -> Option<&str> {
        self.dimension_names.get(&id).map(|s| s.as_str())
    }
}

/// Manages all charts for the service.
pub struct ChartManager {
    charts: HashMap<String, ChartState>,
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
    ) -> Option<&mut ChartState> {
        if !self.charts.contains_key(chart_name) {
            let data_kind = dp.data_kind()?;
            let temporality = dp.aggregation_temporality();

            let chart =
                Chart::from_metric(chart_name.to_string(), data_kind, temporality, self.config)?;

            self.charts
                .insert(chart_name.to_string(), ChartState::new(chart));
        }

        self.charts.get_mut(chart_name)
    }

    /// Trigger tick-based finalization for all charts.
    /// Returns charts that had their active slot finalized due to grace period expiration.
    pub fn tick_all(&mut self) -> Vec<(String, FinalizedSlot)> {
        self.charts
            .iter_mut()
            .filter_map(|(name, state)| state.chart.tick().map(|slot| (name.clone(), slot)))
            .collect()
    }

    /// Get chart state for outputting dimension names.
    pub fn get_chart(&self, name: &str) -> Option<&ChartState> {
        self.charts.get(name)
    }
}

/// Emit a finalized slot to Netdata.
fn emit_slot(chart_name: &str, slot: &FinalizedSlot, chart_state: Option<&ChartState>) {
    // For now, just print the values. Later this will use the Netdata plugin protocol.
    println!(
        "CHART {} @ {} (slot_timestamp={})",
        chart_name, slot.slot_timestamp, slot.slot_timestamp
    );

    for dim in &slot.dimensions {
        let dim_name = chart_state
            .and_then(|s| s.dimension_name(dim.dimension_id))
            .unwrap_or("unknown");

        let value_str = match dim.value {
            Some(v) => format!("{:.6}", v),
            None => "U".to_string(), // Unknown/undefined in Netdata
        };

        println!("  DIM {} = {}", dim_name, value_str);
    }
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

        loop {
            interval.tick().await;

            let mut manager = chart_manager.write().await;
            let finalized_charts = manager.tick_all();

            if !finalized_charts.is_empty() {
                println!("Tick finalized {} charts", finalized_charts.len());
            }

            // Emit finalized slots
            for (chart_name, slot) in &finalized_charts {
                let chart_state = manager.get_chart(chart_name);
                emit_slot(chart_name, slot, chart_state);
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
            let Some(chart_state) = chart_manager.get_or_create_chart(&chart_name_buf, &dp) else {
                // Unsupported metric type
                continue;
            };

            // Get dimension ID
            let dimension_id = chart_state.dimension_id(dimension_name);

            // Ingest the data point - may return a finalized slot if this
            // data belongs to a newer slot
            if let Some(finalized) =
                chart_state
                    .chart
                    .ingest(dimension_id, value, timestamp_ns, start_time_ns)
            {
                emit_slot(&chart_name_buf, &finalized, Some(chart_state));
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
