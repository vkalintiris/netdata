//! Chart management for Netdata metrics.
//!
//! A `Chart` represents a Netdata chart that owns its dimensions directly.
//! Slot timing is handled via `SlotConfig` (shared) and `SlotState` (per-chart).

use std::io;

use opentelemetry_proto::tonic::metrics::v1::AggregationTemporality;

use crate::aggregation::{CumulativeSumAggregator, DeltaSumAggregator, GaugeAggregator};
use crate::iter::MetricDataKind;
use crate::output::NetdataOutput;
use crate::slot::{Dimension, SlotConfig, SlotState};

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

/// Type-erased dimension storage.
enum DimensionStore {
    Gauge(Vec<Dimension<GaugeAggregator>>),
    DeltaSum(Vec<Dimension<DeltaSumAggregator>>),
    CumulativeSum(Vec<Dimension<CumulativeSumAggregator>>),
}

impl DimensionStore {
    fn new(aggregation_type: ChartAggregationType) -> Self {
        match aggregation_type {
            ChartAggregationType::Gauge => DimensionStore::Gauge(Vec::new()),
            ChartAggregationType::DeltaSum => DimensionStore::DeltaSum(Vec::new()),
            ChartAggregationType::CumulativeSum => DimensionStore::CumulativeSum(Vec::new()),
        }
    }

    fn has_dimension(&self, name: &str) -> bool {
        match self {
            DimensionStore::Gauge(dims) => dims.iter().any(|d| d.name() == name),
            DimensionStore::DeltaSum(dims) => dims.iter().any(|d| d.name() == name),
            DimensionStore::CumulativeSum(dims) => dims.iter().any(|d| d.name() == name),
        }
    }

    fn dimension_names(&self) -> Box<dyn Iterator<Item = &str> + '_> {
        match self {
            DimensionStore::Gauge(dims) => Box::new(dims.iter().map(|d| d.name())),
            DimensionStore::DeltaSum(dims) => Box::new(dims.iter().map(|d| d.name())),
            DimensionStore::CumulativeSum(dims) => Box::new(dims.iter().map(|d| d.name())),
        }
    }

    fn ingest(&mut self, dimension_name: &str, value: f64, timestamp_ns: u64, start_time_ns: u64) {
        match self {
            DimensionStore::Gauge(dims) => {
                let dim = Self::get_or_create_dimension(dims, dimension_name);
                dim.ingest(value, timestamp_ns, start_time_ns);
            }
            DimensionStore::DeltaSum(dims) => {
                let dim = Self::get_or_create_dimension(dims, dimension_name);
                dim.ingest(value, timestamp_ns, start_time_ns);
            }
            DimensionStore::CumulativeSum(dims) => {
                let dim = Self::get_or_create_dimension(dims, dimension_name);
                dim.ingest(value, timestamp_ns, start_time_ns);
            }
        }
    }

    fn get_or_create_dimension<'a, A: crate::aggregation::Aggregator + Default>(
        dims: &'a mut Vec<Dimension<A>>,
        name: &str,
    ) -> &'a mut Dimension<A> {
        if let Some(pos) = dims.iter().position(|d| d.name() == name) {
            &mut dims[pos]
        } else {
            dims.push(Dimension::new(name.to_string()));
            dims.last_mut().unwrap()
        }
    }

    /// Finalize and emit all dimensions, calling the provided closure for each.
    fn finalize_and_emit<F>(&mut self, mut emit_fn: F)
    where
        F: FnMut(&str, Option<f64>),
    {
        match self {
            DimensionStore::Gauge(dims) => {
                for dim in dims.iter_mut() {
                    let value = dim.finalize_slot();
                    emit_fn(dim.name(), value);
                }
            }
            DimensionStore::DeltaSum(dims) => {
                for dim in dims.iter_mut() {
                    let value = dim.finalize_slot();
                    emit_fn(dim.name(), value);
                }
            }
            DimensionStore::CumulativeSum(dims) => {
                for dim in dims.iter_mut() {
                    let value = dim.finalize_slot();
                    emit_fn(dim.name(), value);
                }
            }
        }
    }
}

/// A Netdata chart that owns its dimensions and slot state.
pub struct Chart {
    /// The chart's ID (for Netdata identification)
    name: String,
    /// Chart title (for Netdata UI)
    title: String,
    /// Chart units (for Netdata UI)
    units: String,
    /// Chart family (for Netdata grouping)
    family: String,
    /// Collection interval in seconds
    update_every: u64,
    /// Whether the chart definition (including all dimensions) has been emitted to Netdata
    defined: bool,
    /// The dimensions owned by this chart
    dimensions: DimensionStore,
    /// Per-chart slot state
    slot_state: SlotState,
    /// Slot timing configuration
    slot_config: SlotConfig,
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
        config: SlotConfig,
    ) -> Option<Self> {
        let aggregation_type = ChartAggregationType::from_metric(data_kind, temporality)?;

        Some(Self {
            name,
            title,
            units,
            family,
            update_every: config.interval_secs,
            defined: false,
            dimensions: DimensionStore::new(aggregation_type),
            slot_state: SlotState::default(),
            slot_config: config,
        })
    }

    /// Ingest a data point for a dimension.
    ///
    /// If a new dimension is added, the chart will be marked as needing
    /// re-definition (CHART + all DIMENSIONs will be re-emitted on next emit).
    ///
    /// If this ingestion causes the previous active slot to be finalized
    /// (because data arrived for a newer slot), the slot is emitted to output.
    pub fn ingest<W: io::Write>(
        &mut self,
        dimension_name: &str,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
        output: &mut NetdataOutput<W>,
    ) {
        // Check if this is a new dimension before ingesting
        if !self.dimensions.has_dimension(dimension_name) {
            self.defined = false;
        }

        let data_slot = self.slot_config.slot_for_timestamp(timestamp_ns);

        match self.slot_state.active_slot {
            None => {
                // No active slot yet - this becomes the active slot
                self.slot_state.active_slot = Some(data_slot);
                self.dimensions
                    .ingest(dimension_name, value, timestamp_ns, start_time_ns);
                self.slot_state.touch();
            }
            Some(active) if data_slot < active => {
                // Data for a previous slot - drop it
            }
            Some(active) if data_slot > active => {
                // Data for a newer slot - finalize current, then apply new data
                self.emit_slot(output, active);

                // Now apply the new data
                self.slot_state.active_slot = Some(data_slot);
                self.dimensions
                    .ingest(dimension_name, value, timestamp_ns, start_time_ns);
                self.slot_state.touch();
            }
            Some(_) => {
                // Data for the current active slot
                self.dimensions
                    .ingest(dimension_name, value, timestamp_ns, start_time_ns);
                self.slot_state.touch();
            }
        }
    }

    /// Check if the grace period has expired and emit if so.
    /// Returns true if a slot was emitted.
    pub fn tick<W: io::Write>(&mut self, output: &mut NetdataOutput<W>) -> bool {
        if let Some(slot_timestamp) = self.slot_state.check_grace_period(&self.slot_config) {
            self.emit_slot(output, slot_timestamp);
            self.slot_state.clear();
            true
        } else {
            false
        }
    }

    /// Emit a finalized slot to the Netdata output.
    fn emit_slot<W: io::Write>(&mut self, output: &mut NetdataOutput<W>, slot_timestamp: u64) {
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
            for dim_name in self.dimensions.dimension_names() {
                output.write_dimension_definition(dim_name);
            }

            self.defined = true;
        }

        // Emit the update
        output.write_begin(&self.name);

        self.dimensions.finalize_and_emit(|dim_name, value| {
            if let Some(v) = value {
                output.write_set(dim_name, v);
            }
        });

        output.write_end(slot_timestamp);
        output.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    fn test_config() -> SlotConfig {
        SlotConfig {
            interval_secs: 10,
            grace_period_secs: 30,
        }
    }

    fn test_chart(
        data_kind: MetricDataKind,
        temporality: Option<AggregationTemporality>,
    ) -> Chart {
        Chart::from_metric(
            "test".to_string(),
            "Test Chart".to_string(),
            "value".to_string(),
            "test".to_string(),
            data_kind,
            temporality,
            test_config(),
        )
        .expect("test chart should be supported")
    }

    fn test_output() -> NetdataOutput<Cursor<Vec<u8>>> {
        NetdataOutput::new(Cursor::new(Vec::new()))
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
            SlotConfig::default(),
        );

        assert!(chart.is_some());
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
            SlotConfig::default(),
        );

        assert!(chart.is_some());
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
            SlotConfig::default(),
        );

        assert!(chart.is_some());
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
            SlotConfig::default(),
        );

        assert!(chart.is_none());
    }

    #[test]
    fn gauge_chart_ingests_and_emits_on_slot_transition() {
        let mut chart = test_chart(MetricDataKind::Gauge, None);
        let mut output = test_output();

        // Ingest data for slot 0
        chart.ingest("value", 42.0, ns(5), 0, &mut output);

        // No output yet (slot not finalized)
        assert!(output.writer().get_ref().is_empty());

        // Data for slot 10 finalizes slot 0 and emits
        chart.ingest("value", 50.0, ns(15), 0, &mut output);

        // Should have emitted chart definition and data
        let output_str = String::from_utf8_lossy(output.writer().get_ref());
        assert!(output_str.contains("CHART test"));
        assert!(output_str.contains("DIMENSION value"));
        assert!(output_str.contains("BEGIN test"));
        assert!(output_str.contains("SET value ="));
        assert!(output_str.contains("END 0"));
    }

    #[test]
    fn cumulative_chart_computes_deltas() {
        let start_time = 1_000_000_000u64;

        let mut chart = test_chart(
            MetricDataKind::Sum,
            Some(AggregationTemporality::Cumulative),
        );
        let mut output = test_output();

        // Slot 0: establish baseline
        chart.ingest("value", 100.0, ns(5), start_time, &mut output);

        // Slot 10: finalize slot 0 (emits None for first slot - no SET)
        chart.ingest("value", 150.0, ns(15), start_time, &mut output);

        // Slot 20: finalize slot 10 (should emit delta of 50)
        chart.ingest("value", 200.0, ns(25), start_time, &mut output);

        let output_str = String::from_utf8_lossy(output.writer().get_ref());
        // The second slot should have emitted the delta (50.0 * 1000000)
        assert!(output_str.contains("SET value = 50000000"));
    }
}
