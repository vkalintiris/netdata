//! Chart management for Netdata metrics.
//!
//! A `Chart` represents a Netdata chart backed by a `SlotManager` with the
//! appropriate aggregator type based on the OpenTelemetry metric's data kind
//! and aggregation temporality.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::io;

use opentelemetry_proto::tonic::metrics::v1::AggregationTemporality;
use twox_hash::XxHash64;

use crate::aggregation::{CumulativeSumAggregator, DeltaSumAggregator, GaugeAggregator};
use crate::iter::MetricDataKind;
use crate::output::NetdataOutput;
use crate::slot::{DimensionId, FinalizedDimension, SlotManager};

/// Configuration for chart timing.
#[derive(Debug, Clone, Copy)]
pub struct ChartConfig {
    /// Collection interval in seconds
    pub interval_secs: u64,
    /// Grace period in seconds before finalizing an idle slot
    pub grace_period_secs: u64,
}

impl Default for ChartConfig {
    fn default() -> Self {
        Self {
            interval_secs: 10,
            grace_period_secs: 60,
        }
    }
}

/// The type of aggregation used by a chart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartAggregationType {
    Gauge,
    DeltaSum,
    CumulativeSum,
}

impl ChartAggregationType {
    /// Determine the aggregation type from metric metadata.
    pub fn from_metric(
        data_kind: MetricDataKind,
        temporality: Option<AggregationTemporality>,
    ) -> Option<Self> {
        match data_kind {
            MetricDataKind::Gauge => Some(ChartAggregationType::Gauge),
            MetricDataKind::Sum => match temporality {
                Some(AggregationTemporality::Delta) => Some(ChartAggregationType::DeltaSum),
                Some(AggregationTemporality::Cumulative) => {
                    Some(ChartAggregationType::CumulativeSum)
                }
                _ => None, // Unspecified temporality
            },
            // Histograms, ExponentialHistograms, and Summaries not supported yet
            _ => None,
        }
    }
}

/// A Netdata chart backed by a slot manager.
///
/// Contains all state for a chart including:
/// - The underlying slot manager for aggregation
/// - Metadata (title, units, family) for Netdata
/// - Dimension name mappings
/// - Protocol state (whether definitions have been emitted)
pub struct Chart {
    /// The chart's ID (for Netdata identification)
    name: String,
    /// The aggregation type
    aggregation_type: ChartAggregationType,
    /// The underlying slot manager (type-erased via enum)
    inner: ChartInner,
    /// Chart title (for Netdata UI)
    title: String,
    /// Chart units (for Netdata UI)
    units: String,
    /// Chart family (for Netdata grouping)
    family: String,
    /// Collection interval in seconds
    update_every: u64,
    /// Map from dimension ID to dimension name
    dimension_names: HashMap<DimensionId, String>,
    /// Whether the chart definition (including all dimensions) has been emitted to Netdata
    defined: bool,
}

enum ChartInner {
    Gauge(SlotManager<GaugeAggregator>),
    DeltaSum(SlotManager<DeltaSumAggregator>),
    CumulativeSum(SlotManager<CumulativeSumAggregator>),
}

impl Chart {
    /// Create a chart from metric metadata.
    ///
    /// Returns `None` if the metric type is not supported (e.g., histograms).
    pub fn from_metric(
        name: String,
        title: String,
        units: String,
        family: String,
        data_kind: MetricDataKind,
        temporality: Option<AggregationTemporality>,
        config: ChartConfig,
    ) -> Option<Self> {
        let aggregation_type = ChartAggregationType::from_metric(data_kind, temporality)?;

        let inner = match aggregation_type {
            ChartAggregationType::Gauge => ChartInner::Gauge(SlotManager::new(
                config.interval_secs,
                config.grace_period_secs,
            )),
            ChartAggregationType::DeltaSum => ChartInner::DeltaSum(SlotManager::new(
                config.interval_secs,
                config.grace_period_secs,
            )),
            ChartAggregationType::CumulativeSum => ChartInner::CumulativeSum(SlotManager::new(
                config.interval_secs,
                config.grace_period_secs,
            )),
        };

        Some(Self {
            name,
            aggregation_type,
            inner,
            title,
            units,
            family,
            update_every: config.interval_secs,
            dimension_names: HashMap::new(),
            defined: false,
        })
    }

    /// Get the aggregation type.
    pub fn aggregation_type(&self) -> ChartAggregationType {
        self.aggregation_type
    }

    /// Get or create the dimension ID for a dimension name.
    ///
    /// If a new dimension is added, the chart will be marked as needing
    /// re-definition (CHART + all DIMENSIONs will be re-emitted on next emit).
    pub fn dimension_id(&mut self, name: &str) -> DimensionId {
        let mut hasher = XxHash64::default();
        name.hash(&mut hasher);
        let id = hasher.finish();

        if !self.dimension_names.contains_key(&id) {
            self.dimension_names.insert(id, name.to_string());
            // New dimension added - need to re-emit chart definition
            self.defined = false;
        }

        id
    }

    /// Get the dimension name for an ID.
    pub fn dimension_name(&self, id: DimensionId) -> Option<&str> {
        self.dimension_names.get(&id).map(|s| s.as_str())
    }

    /// Ingest a data point for a dimension.
    ///
    /// If this ingestion causes the previous active slot to be finalized
    /// (because data arrived for a newer slot), the `dimensions` buffer
    /// is filled with dimension values and the slot timestamp is returned.
    pub fn ingest(
        &mut self,
        dimension_id: DimensionId,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
        dimensions: &mut Vec<FinalizedDimension>,
    ) -> Option<u64> {
        match &mut self.inner {
            ChartInner::Gauge(mgr) => {
                mgr.ingest(dimension_id, value, timestamp_ns, start_time_ns, dimensions)
            }
            ChartInner::DeltaSum(mgr) => {
                mgr.ingest(dimension_id, value, timestamp_ns, start_time_ns, dimensions)
            }
            ChartInner::CumulativeSum(mgr) => {
                mgr.ingest(dimension_id, value, timestamp_ns, start_time_ns, dimensions)
            }
        }
    }

    /// Check if the grace period has expired and finalize if so.
    pub fn tick(&mut self, dimensions: &mut Vec<FinalizedDimension>) -> Option<u64> {
        match &mut self.inner {
            ChartInner::Gauge(mgr) => mgr.tick(dimensions),
            ChartInner::DeltaSum(mgr) => mgr.tick(dimensions),
            ChartInner::CumulativeSum(mgr) => mgr.tick(dimensions),
        }
    }

    /// Force finalize the active slot. Useful for shutdown or flushing remaining data.
    pub fn finalize(&mut self, dimensions: &mut Vec<FinalizedDimension>) -> Option<u64> {
        match &mut self.inner {
            ChartInner::Gauge(mgr) => mgr.finalize(dimensions),
            ChartInner::DeltaSum(mgr) => mgr.finalize(dimensions),
            ChartInner::CumulativeSum(mgr) => mgr.finalize(dimensions),
        }
    }

    /// Emit a chart update to the Netdata output.
    ///
    /// If the chart hasn't been defined yet (or a new dimension was added),
    /// emits the full chart definition (CHART + all DIMENSIONs) first.
    pub fn emit<W: io::Write>(
        &mut self,
        output: &mut NetdataOutput<W>,
        slot_timestamp: u64,
        dimensions: &[FinalizedDimension],
    ) {
        // Emit chart definition with all dimensions if not yet defined
        if !self.defined {
            output.write_chart_definition(
                &self.name,
                &self.title,
                &self.units,
                &self.family,
                self.update_every,
            );

            // Emit all known dimensions as part of the chart definition
            for dim_name in self.dimension_names.values() {
                output.write_dimension_definition(dim_name);
            }

            self.defined = true;
        }

        // Emit the update
        output.write_begin(&self.name);

        for dim in dimensions {
            if let Some(value) = dim.value {
                let dim_name = self.dimension_name(dim.dimension_id).unwrap_or("unknown");
                output.write_set(dim_name, value);
            }
            // Dimensions with None value are not emitted (gap-fill)
        }

        output.write_end(slot_timestamp);
        output.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    fn test_chart(data_kind: MetricDataKind, temporality: Option<AggregationTemporality>, config: ChartConfig) -> Chart {
        Chart::from_metric(
            "test".to_string(),
            "Test Chart".to_string(),
            "value".to_string(),
            "test".to_string(),
            data_kind,
            temporality,
            config,
        )
        .expect("test chart should be supported")
    }

    #[test]
    fn creates_gauge_chart() {
        let chart = Chart::from_metric(
            "test.gauge".to_string(),
            "Test Gauge".to_string(),
            "value".to_string(),
            "test".to_string(),
            MetricDataKind::Gauge,
            None,
            ChartConfig::default(),
        );

        assert!(chart.is_some());
        let chart = chart.unwrap();
        assert_eq!(chart.aggregation_type(), ChartAggregationType::Gauge);
    }

    #[test]
    fn creates_delta_sum_chart() {
        let chart = Chart::from_metric(
            "test.delta".to_string(),
            "Test Delta".to_string(),
            "value".to_string(),
            "test".to_string(),
            MetricDataKind::Sum,
            Some(AggregationTemporality::Delta),
            ChartConfig::default(),
        );

        assert!(chart.is_some());
        let chart = chart.unwrap();
        assert_eq!(chart.aggregation_type(), ChartAggregationType::DeltaSum);
    }

    #[test]
    fn creates_cumulative_sum_chart() {
        let chart = Chart::from_metric(
            "test.cumulative".to_string(),
            "Test Cumulative".to_string(),
            "value".to_string(),
            "test".to_string(),
            MetricDataKind::Sum,
            Some(AggregationTemporality::Cumulative),
            ChartConfig::default(),
        );

        assert!(chart.is_some());
        let chart = chart.unwrap();
        assert_eq!(chart.aggregation_type(), ChartAggregationType::CumulativeSum);
    }

    #[test]
    fn rejects_unsupported_types() {
        let chart = Chart::from_metric(
            "test.histogram".to_string(),
            "Test Histogram".to_string(),
            "value".to_string(),
            "test".to_string(),
            MetricDataKind::Histogram,
            None,
            ChartConfig::default(),
        );

        assert!(chart.is_none());
    }

    #[test]
    fn gauge_chart_ingests_and_finalizes() {
        let mut chart = test_chart(
            MetricDataKind::Gauge,
            None,
            ChartConfig {
                interval_secs: 10,
                grace_period_secs: 30,
            },
        );
        let mut dimensions = Vec::new();

        let dim_id = chart.dimension_id("value");
        chart.ingest(dim_id, 42.0, ns(5), 0, &mut dimensions);

        // Data for slot 10 finalizes slot 0
        let slot_timestamp = chart.ingest(dim_id, 50.0, ns(15), 0, &mut dimensions);

        assert!(slot_timestamp.is_some());
        assert_eq!(slot_timestamp.unwrap(), 0);
        assert_eq!(dimensions[0].value, Some(42.0));
    }

    #[test]
    fn cumulative_chart_computes_deltas() {
        let start_time = 1_000_000_000u64;

        let mut chart = test_chart(
            MetricDataKind::Sum,
            Some(AggregationTemporality::Cumulative),
            ChartConfig {
                interval_secs: 10,
                grace_period_secs: 30,
            },
        );
        let mut dimensions = Vec::new();

        let dim_id = chart.dimension_id("value");

        // Slot 0: establish baseline
        chart.ingest(dim_id, 100.0, ns(5), start_time, &mut dimensions);

        // Slot 10: finalize slot 0 (returns None for first slot)
        let slot_timestamp = chart.ingest(dim_id, 150.0, ns(15), start_time, &mut dimensions);
        assert!(slot_timestamp.is_some());
        assert_eq!(dimensions[0].value, None);

        // Finalize slot 10
        let slot_timestamp = chart.finalize(&mut dimensions);
        assert!(slot_timestamp.is_some());
        assert_eq!(dimensions[0].value, Some(50.0)); // 150 - 100
    }
}
