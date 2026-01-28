//! Chart management for Netdata metrics.
//!
//! A `Chart` wraps a `SlotManager` with the appropriate aggregator type based on
//! the OpenTelemetry metric's data kind and aggregation temporality.

use opentelemetry_proto::tonic::metrics::v1::AggregationTemporality;

use crate::aggregation::{CumulativeSumAggregator, DeltaSumAggregator, GaugeAggregator};
use crate::iter::MetricDataKind;
use crate::slot::{DimensionId, FinalizedSlot, SlotManager};

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
/// Wraps the appropriate `SlotManager` type based on the metric's aggregation semantics.
pub struct Chart {
    /// The chart's name (for Netdata identification)
    pub name: String,
    /// The aggregation type
    pub aggregation_type: ChartAggregationType,
    /// The underlying slot manager (type-erased via enum)
    inner: ChartInner,
}

enum ChartInner {
    Gauge(SlotManager<GaugeAggregator>),
    DeltaSum(SlotManager<DeltaSumAggregator>),
    CumulativeSum(SlotManager<CumulativeSumAggregator>),
}

impl Chart {
    /// Create a new chart with the given name and aggregation type.
    pub fn new(name: String, aggregation_type: ChartAggregationType, config: ChartConfig) -> Self {
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

        Self {
            name,
            aggregation_type,
            inner,
        }
    }

    /// Create a chart from metric metadata.
    ///
    /// Returns `None` if the metric type is not supported.
    pub fn from_metric(
        name: String,
        data_kind: MetricDataKind,
        temporality: Option<AggregationTemporality>,
        config: ChartConfig,
    ) -> Option<Self> {
        let aggregation_type = ChartAggregationType::from_metric(data_kind, temporality)?;
        Some(Self::new(name, aggregation_type, config))
    }

    /// Ingest a data point for a dimension.
    ///
    /// Returns `Some(FinalizedSlot)` if this ingestion caused the previous
    /// active slot to be finalized (because data arrived for a newer slot).
    pub fn ingest(
        &mut self,
        dimension_id: DimensionId,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
    ) -> Option<FinalizedSlot> {
        match &mut self.inner {
            ChartInner::Gauge(mgr) => mgr.ingest(dimension_id, value, timestamp_ns, start_time_ns),
            ChartInner::DeltaSum(mgr) => {
                mgr.ingest(dimension_id, value, timestamp_ns, start_time_ns)
            }
            ChartInner::CumulativeSum(mgr) => {
                mgr.ingest(dimension_id, value, timestamp_ns, start_time_ns)
            }
        }
    }

    /// Check if the grace period has expired and finalize if so.
    pub fn tick(&mut self) -> Option<FinalizedSlot> {
        match &mut self.inner {
            ChartInner::Gauge(mgr) => mgr.tick(),
            ChartInner::DeltaSum(mgr) => mgr.tick(),
            ChartInner::CumulativeSum(mgr) => mgr.tick(),
        }
    }

    /// Force finalize the active slot.
    pub fn finalize(&mut self) -> Option<FinalizedSlot> {
        match &mut self.inner {
            ChartInner::Gauge(mgr) => mgr.finalize(),
            ChartInner::DeltaSum(mgr) => mgr.finalize(),
            ChartInner::CumulativeSum(mgr) => mgr.finalize(),
        }
    }

    /// Check if there's an active slot.
    pub fn has_active_slot(&self) -> bool {
        match &self.inner {
            ChartInner::Gauge(mgr) => mgr.has_active_slot(),
            ChartInner::DeltaSum(mgr) => mgr.has_active_slot(),
            ChartInner::CumulativeSum(mgr) => mgr.has_active_slot(),
        }
    }

    /// Get the number of known dimensions.
    pub fn dimension_count(&self) -> usize {
        match &self.inner {
            ChartInner::Gauge(mgr) => mgr.dimension_count(),
            ChartInner::DeltaSum(mgr) => mgr.dimension_count(),
            ChartInner::CumulativeSum(mgr) => mgr.dimension_count(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    #[test]
    fn creates_gauge_chart() {
        let chart = Chart::from_metric(
            "test.gauge".to_string(),
            MetricDataKind::Gauge,
            None,
            ChartConfig::default(),
        );

        assert!(chart.is_some());
        let chart = chart.unwrap();
        assert_eq!(chart.aggregation_type, ChartAggregationType::Gauge);
    }

    #[test]
    fn creates_delta_sum_chart() {
        let chart = Chart::from_metric(
            "test.delta".to_string(),
            MetricDataKind::Sum,
            Some(AggregationTemporality::Delta),
            ChartConfig::default(),
        );

        assert!(chart.is_some());
        let chart = chart.unwrap();
        assert_eq!(chart.aggregation_type, ChartAggregationType::DeltaSum);
    }

    #[test]
    fn creates_cumulative_sum_chart() {
        let chart = Chart::from_metric(
            "test.cumulative".to_string(),
            MetricDataKind::Sum,
            Some(AggregationTemporality::Cumulative),
            ChartConfig::default(),
        );

        assert!(chart.is_some());
        let chart = chart.unwrap();
        assert_eq!(chart.aggregation_type, ChartAggregationType::CumulativeSum);
    }

    #[test]
    fn rejects_unsupported_types() {
        let chart = Chart::from_metric(
            "test.histogram".to_string(),
            MetricDataKind::Histogram,
            None,
            ChartConfig::default(),
        );

        assert!(chart.is_none());
    }

    #[test]
    fn gauge_chart_ingests_and_finalizes() {
        let mut chart = Chart::new(
            "test".to_string(),
            ChartAggregationType::Gauge,
            ChartConfig {
                interval_secs: 10,
                grace_period_secs: 30,
            },
        );

        chart.ingest(1, 42.0, ns(5), 0);

        // Data for slot 10 finalizes slot 0
        let finalized = chart.ingest(1, 50.0, ns(15), 0);

        assert!(finalized.is_some());
        let finalized = finalized.unwrap();
        assert_eq!(finalized.slot_timestamp, 0);
        assert_eq!(finalized.dimensions[0].value, Some(42.0));
    }

    #[test]
    fn cumulative_chart_computes_deltas() {
        let start_time = 1_000_000_000u64;

        let mut chart = Chart::new(
            "test".to_string(),
            ChartAggregationType::CumulativeSum,
            ChartConfig {
                interval_secs: 10,
                grace_period_secs: 30,
            },
        );

        // Slot 0: establish baseline
        chart.ingest(1, 100.0, ns(5), start_time);

        // Slot 10: finalize slot 0 (returns None for first slot)
        let finalized = chart.ingest(1, 150.0, ns(15), start_time);
        assert!(finalized.is_some());
        assert_eq!(finalized.unwrap().dimensions[0].value, None);

        // Finalize slot 10
        let finalized = chart.finalize();
        assert!(finalized.is_some());
        assert_eq!(finalized.unwrap().dimensions[0].value, Some(50.0)); // 150 - 100
    }
}
