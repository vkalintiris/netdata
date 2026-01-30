//! Chart management for Netdata metrics.
//!
//! A `Chart` manages dimensions and slot-based aggregation, mapping OpenTelemetry's
//! event-based metrics to Netdata's fixed-interval collection model.

use std::collections::HashMap;
use std::time::Instant;

use opentelemetry_proto::tonic::metrics::v1::AggregationTemporality;

use crate::aggregation::{
    Aggregator, CumulativeSumAggregator, DeltaSumAggregator, GaugeAggregator,
};
use crate::iter::MetricDataKind;

/// A dimension with its name, aggregator, and slot state.
pub struct Dimension<A: Aggregator> {
    /// The dimension's display name.
    pub name: String,
    /// The aggregator for this dimension.
    pub aggregator: A,
    /// Whether this dimension has received data in the current slot.
    pub has_data_in_slot: bool,
}

impl<A: Aggregator + Default> Dimension<A> {
    /// Create a new dimension with the given name.
    pub fn new(name: String) -> Self {
        Self {
            name,
            aggregator: A::default(),
            has_data_in_slot: false,
        }
    }
}

/// A finalized dimension value ready for output.
#[derive(Debug)]
pub struct FinalizedDimension {
    /// The dimension's display name.
    pub name: String,
    /// The value to emit. `None` if no value could be produced
    /// (e.g., first observation for cumulative).
    pub value: Option<f64>,
}

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

/// A Netdata chart that manages dimensions and slot-based aggregation.
pub struct Chart {
    /// Collection interval in seconds
    collection_interval_secs: u64,
    /// Grace period in seconds before finalizing an idle slot
    grace_period_secs: u64,
    /// The currently active slot timestamp (if any)
    active_slot: Option<u64>,
    /// When the active slot last received data (for grace period timeout)
    last_data_instant: Option<Instant>,
    /// The type-erased dimension storage
    inner: ChartInner,
}

/// Type-erased dimension storage for different aggregator types.
enum ChartInner {
    Gauge(HashMap<String, Dimension<GaugeAggregator>>),
    DeltaSum(HashMap<String, Dimension<DeltaSumAggregator>>),
    CumulativeSum(HashMap<String, Dimension<CumulativeSumAggregator>>),
}

impl Chart {
    /// Create a new chart with the given aggregation type.
    pub fn new(aggregation_type: ChartAggregationType, config: ChartConfig) -> Self {
        let inner = match aggregation_type {
            ChartAggregationType::Gauge => ChartInner::Gauge(HashMap::new()),
            ChartAggregationType::DeltaSum => ChartInner::DeltaSum(HashMap::new()),
            ChartAggregationType::CumulativeSum => ChartInner::CumulativeSum(HashMap::new()),
        };

        Self {
            collection_interval_secs: config.interval_secs,
            grace_period_secs: config.grace_period_secs,
            active_slot: None,
            last_data_instant: None,
            inner,
        }
    }

    /// Create a chart from metric metadata.
    ///
    /// Returns `None` if the metric type is not supported.
    pub fn from_metric(
        data_kind: MetricDataKind,
        temporality: Option<AggregationTemporality>,
        config: ChartConfig,
    ) -> Option<Self> {
        let aggregation_type = ChartAggregationType::from_metric(data_kind, temporality)?;
        Some(Self::new(aggregation_type, config))
    }

    /// Compute the slot timestamp for a given nanosecond timestamp.
    fn slot_for_timestamp(&self, timestamp_ns: u64) -> u64 {
        let timestamp_secs = timestamp_ns / 1_000_000_000;
        (timestamp_secs / self.collection_interval_secs) * self.collection_interval_secs
    }

    /// Ingest a data point for a dimension.
    ///
    /// If this data belongs to a newer slot, the current slot is finalized
    /// into the provided buffer and the slot timestamp is returned.
    /// Data for older slots is dropped.
    pub fn ingest(
        &mut self,
        dimension_name: &str,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
        finalized: &mut Vec<FinalizedDimension>,
    ) -> Option<u64> {
        let data_slot = self.slot_for_timestamp(timestamp_ns);

        let slot_timestamp = match self.active_slot {
            None => {
                // No active slot yet - this becomes the active slot
                self.active_slot = Some(data_slot);
                None
            }
            Some(active) if data_slot < active => {
                // Data for a previous slot - drop it
                return None;
            }
            Some(active) if data_slot > active => {
                // Data for a newer slot - finalize current slot first
                let slot_ts = self.finalize_active_slot_into(finalized);
                self.active_slot = Some(data_slot);
                slot_ts
            }
            Some(_) => {
                // Data for the current active slot
                None
            }
        };

        // Ingest into the dimension's aggregator
        self.inner
            .ingest(dimension_name, value, timestamp_ns, start_time_ns);

        // Update last data time
        self.last_data_instant = Some(Instant::now());

        slot_timestamp
    }

    /// Check if the grace period has expired and finalize if so.
    ///
    /// Returns the slot timestamp if finalization occurred.
    pub fn tick(&mut self, finalized: &mut Vec<FinalizedDimension>) -> Option<u64> {
        let grace_expired = self
            .last_data_instant
            .map(|t| t.elapsed() >= std::time::Duration::from_secs(self.grace_period_secs))
            .unwrap_or(false);

        if !grace_expired {
            return None;
        }

        self.finalize_active_slot_into(finalized)
    }

    /// Force finalize the active slot. Useful for shutdown or flushing remaining data.
    pub fn finalize(&mut self, finalized: &mut Vec<FinalizedDimension>) -> Option<u64> {
        self.finalize_active_slot_into(finalized)
    }

    /// Finalize the active slot into the provided buffer.
    /// Returns the slot timestamp if there was an active slot.
    fn finalize_active_slot_into(
        &mut self,
        finalized: &mut Vec<FinalizedDimension>,
    ) -> Option<u64> {
        let slot_timestamp = self.active_slot.take()?;

        self.inner.finalize_into(finalized);

        // Reset timing
        self.last_data_instant = None;

        Some(slot_timestamp)
    }
}

impl ChartInner {
    /// Ingest a value into a dimension's aggregator, creating the dimension if needed.
    fn ingest(&mut self, name: &str, value: f64, timestamp_ns: u64, start_time_ns: u64) {
        match self {
            ChartInner::Gauge(dims) => {
                let dim = if let Some(dim) = dims.get_mut(name) {
                    dim
                } else {
                    dims.insert(name.to_string(), Dimension::new(name.to_string()));
                    dims.get_mut(name).unwrap()
                };
                dim.aggregator.ingest(value, timestamp_ns, start_time_ns);
                dim.has_data_in_slot = true;
            }
            ChartInner::DeltaSum(dims) => {
                let dim = if let Some(dim) = dims.get_mut(name) {
                    dim
                } else {
                    dims.insert(name.to_string(), Dimension::new(name.to_string()));
                    dims.get_mut(name).unwrap()
                };
                dim.aggregator.ingest(value, timestamp_ns, start_time_ns);
                dim.has_data_in_slot = true;
            }
            ChartInner::CumulativeSum(dims) => {
                let dim = if let Some(dim) = dims.get_mut(name) {
                    dim
                } else {
                    dims.insert(name.to_string(), Dimension::new(name.to_string()));
                    dims.get_mut(name).unwrap()
                };
                dim.aggregator.ingest(value, timestamp_ns, start_time_ns);
                dim.has_data_in_slot = true;
            }
        }
    }

    /// Finalize all dimensions into the provided buffer.
    fn finalize_into(&mut self, finalized: &mut Vec<FinalizedDimension>) {
        finalized.clear();

        match self {
            ChartInner::Gauge(dims) => {
                finalized.reserve(dims.len());
                for dim in dims.values_mut() {
                    let value = if dim.has_data_in_slot {
                        dim.aggregator.finalize_slot()
                    } else {
                        Some(dim.aggregator.gap_fill())
                    };
                    finalized.push(FinalizedDimension {
                        name: dim.name.clone(),
                        value,
                    });
                    dim.has_data_in_slot = false;
                }
            }
            ChartInner::DeltaSum(dims) => {
                finalized.reserve(dims.len());
                for dim in dims.values_mut() {
                    let value = if dim.has_data_in_slot {
                        dim.aggregator.finalize_slot()
                    } else {
                        Some(dim.aggregator.gap_fill())
                    };
                    finalized.push(FinalizedDimension {
                        name: dim.name.clone(),
                        value,
                    });
                    dim.has_data_in_slot = false;
                }
            }
            ChartInner::CumulativeSum(dims) => {
                finalized.reserve(dims.len());
                for dim in dims.values_mut() {
                    let value = if dim.has_data_in_slot {
                        dim.aggregator.finalize_slot()
                    } else {
                        Some(dim.aggregator.gap_fill())
                    };
                    finalized.push(FinalizedDimension {
                        name: dim.name.clone(),
                        value,
                    });
                    dim.has_data_in_slot = false;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERVAL_SECS: u64 = 10;
    const GRACE_PERIOD_SECS: u64 = 5;

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    fn gauge_chart() -> Chart {
        Chart::new(
            ChartAggregationType::Gauge,
            ChartConfig {
                interval_secs: INTERVAL_SECS,
                grace_period_secs: GRACE_PERIOD_SECS,
            },
        )
    }

    fn delta_sum_chart() -> Chart {
        Chart::new(
            ChartAggregationType::DeltaSum,
            ChartConfig {
                interval_secs: INTERVAL_SECS,
                grace_period_secs: GRACE_PERIOD_SECS,
            },
        )
    }

    fn cumulative_sum_chart() -> Chart {
        Chart::new(
            ChartAggregationType::CumulativeSum,
            ChartConfig {
                interval_secs: INTERVAL_SECS,
                grace_period_secs: GRACE_PERIOD_SECS,
            },
        )
    }

    mod chart_creation {
        use super::*;

        #[test]
        fn creates_gauge_chart() {
            let chart = Chart::from_metric(MetricDataKind::Gauge, None, ChartConfig::default());
            assert!(chart.is_some());
        }

        #[test]
        fn creates_delta_sum_chart() {
            let chart = Chart::from_metric(
                MetricDataKind::Sum,
                Some(AggregationTemporality::Delta),
                ChartConfig::default(),
            );
            assert!(chart.is_some());
        }

        #[test]
        fn creates_cumulative_sum_chart() {
            let chart = Chart::from_metric(
                MetricDataKind::Sum,
                Some(AggregationTemporality::Cumulative),
                ChartConfig::default(),
            );
            assert!(chart.is_some());
        }

        #[test]
        fn rejects_unsupported_types() {
            let chart = Chart::from_metric(MetricDataKind::Histogram, None, ChartConfig::default());
            assert!(chart.is_none());
        }
    }

    mod basic_operations {
        use super::*;

        #[test]
        fn slot_assignment() {
            let chart = gauge_chart();

            assert_eq!(chart.slot_for_timestamp(ns(0)), 0);
            assert_eq!(chart.slot_for_timestamp(ns(5)), 0);
            assert_eq!(chart.slot_for_timestamp(ns(9)), 0);
            assert_eq!(chart.slot_for_timestamp(ns(10)), 10);
            assert_eq!(chart.slot_for_timestamp(ns(15)), 10);
            assert_eq!(chart.slot_for_timestamp(ns(25)), 20);
        }

        #[test]
        fn ingest_creates_dimension() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            chart.ingest("dim1", 42.0, ns(5), 0, &mut finalized);

            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized.len(), 1);
            assert_eq!(finalized[0].name, "dim1");
            assert_eq!(finalized[0].value, Some(42.0));
        }

        #[test]
        fn ingest_sets_active_slot() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            // No active slot initially - finalize returns None
            assert!(chart.finalize(&mut finalized).is_none());

            // Ingest creates an active slot
            chart.ingest("dim1", 42.0, ns(5), 0, &mut finalized);

            // Now finalize returns Some
            assert!(chart.finalize(&mut finalized).is_some());
        }

        #[test]
        fn drops_data_for_previous_slot() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            // Set active slot to 10
            chart.ingest("dim1", 50.0, ns(15), 0, &mut finalized);

            // Try to ingest for slot 0 - should be dropped (no effect)
            chart.ingest("dim1", 42.0, ns(5), 0, &mut finalized);

            // Finalize and check only the slot 10 value is present
            let slot_timestamp = chart.finalize(&mut finalized).unwrap();
            assert_eq!(slot_timestamp, 10);
            assert_eq!(finalized[0].value, Some(50.0));
        }
    }

    mod finalization {
        use super::*;

        #[test]
        fn ingest_finalizes_on_newer_slot_data() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            // Slot 0
            chart.ingest("dim1", 42.0, ns(5), 0, &mut finalized);

            // Slot 10 data - triggers finalization of slot 0
            let slot_timestamp = chart.ingest("dim1", 50.0, ns(15), 0, &mut finalized);
            assert_eq!(slot_timestamp, Some(0));
            assert_eq!(finalized[0].value, Some(42.0));

            // Slot 10 data should now be in active slot
            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized[0].value, Some(50.0));
        }

        #[test]
        fn no_finalization_for_same_slot() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            chart.ingest("dim1", 42.0, ns(5), 0, &mut finalized);
            let result = chart.ingest("dim1", 43.0, ns(7), 0, &mut finalized);

            // Same slot data doesn't trigger finalization
            assert!(result.is_none());

            // Tick should not finalize (no grace period expiry)
            assert!(chart.tick(&mut finalized).is_none());

            // But active slot still exists and can be force-finalized
            assert!(chart.finalize(&mut finalized).is_some());
        }

        #[test]
        fn force_finalize() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            chart.ingest("dim1", 42.0, ns(5), 0, &mut finalized);

            // Force finalize returns the slot
            let slot_timestamp = chart.finalize(&mut finalized);
            assert!(slot_timestamp.is_some());

            // After finalization, no active slot remains
            assert!(chart.finalize(&mut finalized).is_none());
        }
    }

    mod gauge_aggregation {
        use super::*;

        #[test]
        fn keeps_last_value_by_timestamp() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            chart.ingest("dim1", 10.0, ns(1), 0, &mut finalized);
            chart.ingest("dim1", 30.0, ns(3), 0, &mut finalized); // Latest
            chart.ingest("dim1", 20.0, ns(2), 0, &mut finalized);

            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized[0].value, Some(30.0));
        }

        #[test]
        fn gap_fills_missing_dimension() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            // Slot 0: both dimensions
            chart.ingest("dim1", 10.0, ns(5), 0, &mut finalized);
            chart.ingest("dim2", 20.0, ns(5), 0, &mut finalized);

            // Slot 10 data (only dim1) - triggers finalization of slot 0
            let slot_ts = chart.ingest("dim1", 15.0, ns(15), 0, &mut finalized);
            assert_eq!(slot_ts, Some(0));

            // Slot 0: dim1=10, dim2=20
            let dim1 = finalized.iter().find(|d| d.name == "dim1").unwrap();
            let dim2 = finalized.iter().find(|d| d.name == "dim2").unwrap();
            assert_eq!(dim1.value, Some(10.0));
            assert_eq!(dim2.value, Some(20.0));

            // Finalize slot 10: dim2 should be gap-filled with previous value
            chart.finalize(&mut finalized).unwrap();
            let dim2 = finalized.iter().find(|d| d.name == "dim2").unwrap();
            assert_eq!(dim2.value, Some(20.0));
        }
    }

    mod delta_sum_aggregation {
        use super::*;

        #[test]
        fn sums_deltas() {
            let mut chart = delta_sum_chart();
            let mut finalized = Vec::new();

            chart.ingest("dim1", 10.0, ns(1), 0, &mut finalized);
            chart.ingest("dim1", 20.0, ns(2), ns(1), &mut finalized);
            chart.ingest("dim1", 5.0, ns(3), ns(2), &mut finalized);

            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized[0].value, Some(35.0));
        }
    }

    mod cumulative_sum_aggregation {
        use super::*;

        const START_TIME: u64 = 1_000_000_000;

        #[test]
        fn first_slot_returns_none() {
            let mut chart = cumulative_sum_chart();
            let mut finalized = Vec::new();

            chart.ingest("dim1", 100.0, ns(5), START_TIME, &mut finalized);

            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized[0].value, None);
        }

        #[test]
        fn computes_deltas_across_slots() {
            let mut chart = cumulative_sum_chart();
            let mut finalized = Vec::new();

            // Slot 0: baseline
            chart.ingest("dim1", 100.0, ns(5), START_TIME, &mut finalized);

            // Slot 10: triggers finalization of slot 0
            let slot_ts = chart.ingest("dim1", 150.0, ns(15), START_TIME, &mut finalized);
            assert_eq!(slot_ts, Some(0));
            assert_eq!(finalized[0].value, None); // First slot

            // Finalize slot 10
            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized[0].value, Some(50.0)); // 150 - 100
        }

        #[test]
        fn detects_restart() {
            let mut chart = cumulative_sum_chart();
            let mut finalized = Vec::new();

            // Establish baseline in slot 0
            chart.ingest("dim1", 100.0, ns(5), START_TIME, &mut finalized);

            // Slot 10: triggers finalization of slot 0
            chart.ingest("dim1", 150.0, ns(15), START_TIME, &mut finalized);

            // Slot 20: restart with new start_time, triggers finalization of slot 10
            let new_start = START_TIME + 1_000_000;
            chart.ingest("dim1", 20.0, ns(25), new_start, &mut finalized);

            // Finalize slot 20
            chart.finalize(&mut finalized).unwrap();
            assert_eq!(finalized[0].value, Some(0.0));
        }
    }

    mod incremental_aggregation {
        use super::*;

        #[test]
        fn gauge_out_of_order_timestamps_keeps_latest() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            // Ingest out of order: middle, latest, earliest
            chart.ingest("dim1", 20.0, ns(2), 0, &mut finalized); // t=2
            chart.ingest("dim1", 30.0, ns(3), 0, &mut finalized); // t=3 (latest)
            chart.ingest("dim1", 10.0, ns(1), 0, &mut finalized); // t=1 (arrives last but oldest)

            chart.finalize(&mut finalized).unwrap();

            // Should keep value at t=3, not the last-arrived value
            assert_eq!(finalized[0].value, Some(30.0));
        }

        #[test]
        fn multi_dimension_gap_fill_across_slots() {
            let mut chart = gauge_chart();
            let mut finalized = Vec::new();

            // Slot 0: dim1=100, dim2=200, dim3=300
            chart.ingest("dim1", 100.0, ns(5), 0, &mut finalized);
            chart.ingest("dim2", 200.0, ns(5), 0, &mut finalized);
            chart.ingest("dim3", 300.0, ns(5), 0, &mut finalized);

            // Slot 10: only dim1 gets new data - triggers finalization of slot 0
            let slot_ts = chart.ingest("dim1", 110.0, ns(15), 0, &mut finalized);
            assert_eq!(slot_ts, Some(0));

            // Verify slot 0 values
            assert_eq!(finalized.len(), 3);
            let dim1 = finalized.iter().find(|d| d.name == "dim1").unwrap();
            let dim2 = finalized.iter().find(|d| d.name == "dim2").unwrap();
            let dim3 = finalized.iter().find(|d| d.name == "dim3").unwrap();
            assert_eq!(dim1.value, Some(100.0));
            assert_eq!(dim2.value, Some(200.0));
            assert_eq!(dim3.value, Some(300.0));

            // Slot 20: only dim2 gets new data - triggers finalization of slot 10
            let slot_ts = chart.ingest("dim2", 220.0, ns(25), 0, &mut finalized);
            assert_eq!(slot_ts, Some(10));

            // Verify slot 10: dim1=110 (new), dim2=200 (gap-fill), dim3=300 (gap-fill)
            let dim1 = finalized.iter().find(|d| d.name == "dim1").unwrap();
            let dim2 = finalized.iter().find(|d| d.name == "dim2").unwrap();
            let dim3 = finalized.iter().find(|d| d.name == "dim3").unwrap();
            assert_eq!(dim1.value, Some(110.0)); // New data
            assert_eq!(dim2.value, Some(200.0)); // Gap-filled from slot 0
            assert_eq!(dim3.value, Some(300.0)); // Gap-filled from slot 0
        }

        #[test]
        fn delta_sum_accumulates_correctly_with_incremental_ingest() {
            let mut chart = delta_sum_chart();
            let mut finalized = Vec::new();

            // Multiple deltas for same dimension in same slot
            chart.ingest("dim1", 5.0, ns(1), 0, &mut finalized);
            chart.ingest("dim1", 10.0, ns(2), ns(1), &mut finalized);
            chart.ingest("dim1", 15.0, ns(3), ns(2), &mut finalized);
            chart.ingest("dim1", 20.0, ns(4), ns(3), &mut finalized);

            chart.finalize(&mut finalized).unwrap();

            // Should sum all deltas: 5 + 10 + 15 + 20 = 50
            assert_eq!(finalized[0].value, Some(50.0));
        }
    }
}
