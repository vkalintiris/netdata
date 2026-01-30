//! Slot management for mapping OpenTelemetry's event-based metrics to Netdata's
//! fixed-interval collection model.
//!
//! The `SlotManager` tracks a single active slot and:
//! - Drops data for previous slots
//! - Finalizes the active slot when data arrives for a newer slot
//! - Finalizes the active slot after a grace period with no data

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::Instant;

use crate::aggregation::Aggregator;

/// Identifier for a dimension within a chart.
pub type DimensionId = u64;

/// A finalized dimension value.
#[derive(Debug)]
pub struct FinalizedDimension {
    pub dimension_id: DimensionId,
    /// The value to emit. `None` if no value could be produced
    /// (e.g., first observation for cumulative).
    pub value: Option<f64>,
}

/// Manages a single active slot for a chart.
///
/// Generic over the aggregator type - all dimensions in a chart use the same
/// aggregator type since they have the same metric semantics.
pub struct SlotManager<A: Aggregator + Default> {
    /// Collection interval in seconds
    collection_interval_secs: u64,
    /// Grace period in seconds before finalizing an idle slot
    grace_period_secs: u64,

    /// Per-dimension aggregators (maintain cross-slot state)
    aggregators: HashMap<DimensionId, A>,

    /// All known dimension IDs (for gap filling)
    dimension_ids: BTreeSet<DimensionId>,

    /// The currently active slot timestamp (if any)
    active_slot: Option<u64>,

    /// Dimensions that received data in the current slot
    dimensions_with_data: HashSet<DimensionId>,

    /// When the active slot last received data (for grace period timeout)
    last_data_instant: Option<Instant>,
}

impl<A: Aggregator + Default> SlotManager<A> {
    /// Create a new slot manager with the given timing configuration.
    pub fn new(interval_secs: u64, grace_period_secs: u64) -> Self {
        Self {
            collection_interval_secs: interval_secs,
            grace_period_secs,
            aggregators: HashMap::new(),
            dimension_ids: BTreeSet::new(),
            active_slot: None,
            dimensions_with_data: HashSet::new(),
            last_data_instant: None,
        }
    }

    /// Compute the slot timestamp for a given nanosecond timestamp.
    fn slot_for_timestamp(&self, timestamp_ns: u64) -> u64 {
        let timestamp_secs = timestamp_ns / 1_000_000_000;
        (timestamp_secs / self.collection_interval_secs) * self.collection_interval_secs
    }

    /// Ingest a data point for a dimension.
    ///
    /// If this ingestion causes the previous active slot to be finalized
    /// (because data arrived for a newer slot), the `dimensions` buffer
    /// is filled with dimension values and the slot timestamp is returned.
    ///
    /// Returns `None` if no finalization occurred (data was ingested or dropped).
    pub fn ingest(
        &mut self,
        dimension_id: DimensionId,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
        dimensions: &mut Vec<FinalizedDimension>,
    ) -> Option<u64> {
        let data_slot = self.slot_for_timestamp(timestamp_ns);

        // Track this dimension
        self.dimension_ids.insert(dimension_id);
        self.aggregators.entry(dimension_id).or_default();

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
                // Data for a newer slot - finalize current and start new
                let slot_timestamp = self.finalize_active_slot_into(dimensions);
                self.active_slot = Some(data_slot);
                slot_timestamp
            }
            Some(_) => {
                // Data for the current active slot
                None
            }
        };

        // Pass directly to the aggregator
        self.aggregators
            .get_mut(&dimension_id)
            .unwrap()
            .ingest(value, timestamp_ns, start_time_ns);
        self.dimensions_with_data.insert(dimension_id);

        // Update last data time
        self.last_data_instant = Some(Instant::now());

        slot_timestamp
    }

    /// Check if the grace period has expired and finalize if so.
    ///
    /// If the active slot is finalized due to grace period expiration,
    /// the `dimensions` buffer is filled and the slot timestamp is returned.
    pub fn tick(&mut self, dimensions: &mut Vec<FinalizedDimension>) -> Option<u64> {
        let last_data = self.last_data_instant?;
        let grace_period = std::time::Duration::from_secs(self.grace_period_secs);

        if last_data.elapsed() >= grace_period {
            self.finalize_active_slot_into(dimensions)
        } else {
            None
        }
    }

    /// Finalize the active slot into the provided buffer.
    /// Returns the slot timestamp if there was an active slot.
    fn finalize_active_slot_into(
        &mut self,
        dimensions: &mut Vec<FinalizedDimension>,
    ) -> Option<u64> {
        let slot_timestamp = self.active_slot.take()?;

        dimensions.clear();
        dimensions.reserve(self.dimension_ids.len());

        for &dim_id in &self.dimension_ids {
            let value = if let Some(agg) = self.aggregators.get_mut(&dim_id) {
                if self.dimensions_with_data.contains(&dim_id) {
                    agg.finalize_slot()
                } else {
                    // No data for this dimension in this slot - gap fill
                    Some(agg.gap_fill())
                }
            } else {
                None
            };

            dimensions.push(FinalizedDimension {
                dimension_id: dim_id,
                value,
            });
        }

        // Clear received data tracking and reset timing
        self.dimensions_with_data.clear();
        self.last_data_instant = None;

        Some(slot_timestamp)
    }

    /// Force finalize the active slot. Useful for shutdown or flushing remaining data.
    pub fn finalize(&mut self, dimensions: &mut Vec<FinalizedDimension>) -> Option<u64> {
        self.finalize_active_slot_into(dimensions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregation::{CumulativeSumAggregator, DeltaSumAggregator, GaugeAggregator};

    const INTERVAL_SECS: u64 = 10;
    const GRACE_PERIOD_SECS: u64 = 5;

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    mod basic_operations {
        use super::*;

        #[test]
        fn slot_assignment() {
            let mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            assert_eq!(mgr.slot_for_timestamp(ns(0)), 0);
            assert_eq!(mgr.slot_for_timestamp(ns(5)), 0);
            assert_eq!(mgr.slot_for_timestamp(ns(9)), 0);
            assert_eq!(mgr.slot_for_timestamp(ns(10)), 10);
            assert_eq!(mgr.slot_for_timestamp(ns(15)), 10);
            assert_eq!(mgr.slot_for_timestamp(ns(25)), 20);
        }

        #[test]
        fn ingest_creates_dimension() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Ingest a data point for dimension 1
            mgr.ingest(1, 42.0, ns(5), 0, &mut dimensions);

            // Verify dimension appears in finalized output
            mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(dimensions.len(), 1);
            assert_eq!(dimensions[0].dimension_id, 1);
            assert_eq!(dimensions[0].value, Some(42.0));
        }

        #[test]
        fn ingest_sets_active_slot() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // No active slot initially - finalize returns None
            assert!(mgr.finalize(&mut dimensions).is_none());

            // Ingest creates an active slot
            mgr.ingest(1, 42.0, ns(5), 0, &mut dimensions);

            // Now finalize returns Some
            assert!(mgr.finalize(&mut dimensions).is_some());
        }

        #[test]
        fn drops_data_for_previous_slot() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Set active slot to 10
            mgr.ingest(1, 50.0, ns(15), 0, &mut dimensions);

            // Try to ingest for slot 0 - should be dropped
            let result = mgr.ingest(1, 42.0, ns(5), 0, &mut dimensions);
            assert!(result.is_none());

            // Finalize and check only the slot 10 value is present
            let slot_timestamp = mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(slot_timestamp, 10);
            assert_eq!(dimensions[0].value, Some(50.0));
        }
    }

    mod finalization {
        use super::*;

        #[test]
        fn finalizes_on_newer_slot_data() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Slot 0
            mgr.ingest(1, 42.0, ns(5), 0, &mut dimensions);

            // Slot 10 - should finalize slot 0
            let slot_timestamp = mgr.ingest(1, 50.0, ns(15), 0, &mut dimensions);

            assert!(slot_timestamp.is_some());
            assert_eq!(slot_timestamp.unwrap(), 0);
            assert_eq!(dimensions[0].value, Some(42.0));
        }

        #[test]
        fn no_finalization_for_same_slot() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            mgr.ingest(1, 42.0, ns(5), 0, &mut dimensions);
            let result = mgr.ingest(1, 43.0, ns(7), 0, &mut dimensions);

            // Same slot data doesn't trigger finalization
            assert!(result.is_none());

            // But active slot still exists and can be finalized
            assert!(mgr.finalize(&mut dimensions).is_some());
        }

        #[test]
        fn force_finalize() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            mgr.ingest(1, 42.0, ns(5), 0, &mut dimensions);

            // Force finalize returns the slot
            let slot_timestamp = mgr.finalize(&mut dimensions);
            assert!(slot_timestamp.is_some());

            // After finalization, no active slot remains
            assert!(mgr.finalize(&mut dimensions).is_none());
        }
    }

    mod gauge_aggregation {
        use super::*;

        #[test]
        fn keeps_last_value_by_timestamp() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            mgr.ingest(1, 10.0, ns(1), 0, &mut dimensions);
            mgr.ingest(1, 30.0, ns(3), 0, &mut dimensions); // Latest
            mgr.ingest(1, 20.0, ns(2), 0, &mut dimensions);

            mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(dimensions[0].value, Some(30.0));
        }

        #[test]
        fn gap_fills_missing_dimension() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Slot 0: both dimensions
            mgr.ingest(1, 10.0, ns(5), 0, &mut dimensions);
            mgr.ingest(2, 20.0, ns(5), 0, &mut dimensions);

            // Finalize slot 0 via newer slot data, only dim 1 has data
            mgr.ingest(1, 15.0, ns(15), 0, &mut dimensions);
            mgr.finalize(&mut dimensions).unwrap();

            // Dim 2 should be gap-filled with previous value (20.0)
            let dim2 = dimensions.iter().find(|d| d.dimension_id == 2).unwrap();
            assert_eq!(dim2.value, Some(20.0));
        }
    }

    mod delta_sum_aggregation {
        use super::*;

        #[test]
        fn sums_deltas() {
            let mut mgr: SlotManager<DeltaSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            mgr.ingest(1, 10.0, ns(1), 0, &mut dimensions);
            mgr.ingest(1, 20.0, ns(2), ns(1), &mut dimensions);
            mgr.ingest(1, 5.0, ns(3), ns(2), &mut dimensions);

            mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(dimensions[0].value, Some(35.0));
        }
    }

    mod cumulative_sum_aggregation {
        use super::*;

        const START_TIME: u64 = 1_000_000_000;

        #[test]
        fn first_slot_returns_none() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            mgr.ingest(1, 100.0, ns(5), START_TIME, &mut dimensions);

            mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(dimensions[0].value, None);
        }

        #[test]
        fn computes_deltas_across_slots() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Slot 0: baseline
            mgr.ingest(1, 100.0, ns(5), START_TIME, &mut dimensions);

            // Slot 10: finalize slot 0 and buffer slot 10 data
            let slot_timestamp = mgr.ingest(1, 150.0, ns(15), START_TIME, &mut dimensions);
            assert!(slot_timestamp.is_some());
            assert_eq!(dimensions[0].value, None); // First slot

            // Finalize slot 10
            mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(dimensions[0].value, Some(50.0)); // 150 - 100
        }

        #[test]
        fn detects_restart() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Establish baseline
            mgr.ingest(1, 100.0, ns(5), START_TIME, &mut dimensions);
            mgr.ingest(1, 150.0, ns(15), START_TIME, &mut dimensions);
            mgr.finalize(&mut dimensions);

            // Restart with new start_time
            let new_start = START_TIME + 1_000_000;
            mgr.ingest(1, 20.0, ns(25), new_start, &mut dimensions);

            mgr.finalize(&mut dimensions).unwrap();
            assert_eq!(dimensions[0].value, Some(0.0));
        }
    }

    /// Tests to verify incremental aggregation produces correct results.
    /// These tests ensure our optimization (aggregating as data arrives vs batching)
    /// doesn't change behavior.
    mod incremental_aggregation {
        use super::*;

        #[test]
        fn gauge_out_of_order_timestamps_keeps_latest() {
            // Verifies that even with incremental aggregation, we correctly
            // keep the value with the latest timestamp, not the last-arrived value.
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Ingest out of order: middle, latest, earliest
            mgr.ingest(1, 20.0, ns(2), 0, &mut dimensions); // t=2
            mgr.ingest(1, 30.0, ns(3), 0, &mut dimensions); // t=3 (latest)
            mgr.ingest(1, 10.0, ns(1), 0, &mut dimensions); // t=1 (arrives last but oldest)

            mgr.finalize(&mut dimensions).unwrap();

            // Should keep value at t=3, not the last-arrived value
            assert_eq!(dimensions[0].value, Some(30.0));
        }

        #[test]
        fn multi_dimension_gap_fill_across_slots() {
            // Verifies gap-fill works correctly when dimensions receive data
            // in different slots.
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Slot 0: dim1=100, dim2=200, dim3=300
            mgr.ingest(1, 100.0, ns(5), 0, &mut dimensions);
            mgr.ingest(2, 200.0, ns(5), 0, &mut dimensions);
            mgr.ingest(3, 300.0, ns(5), 0, &mut dimensions);

            // Slot 10: only dim1 gets new data
            let slot_ts = mgr.ingest(1, 110.0, ns(15), 0, &mut dimensions);
            assert_eq!(slot_ts, Some(0)); // Slot 0 finalized

            // Verify slot 0 values
            assert_eq!(dimensions.len(), 3);
            let dim1 = dimensions.iter().find(|d| d.dimension_id == 1).unwrap();
            let dim2 = dimensions.iter().find(|d| d.dimension_id == 2).unwrap();
            let dim3 = dimensions.iter().find(|d| d.dimension_id == 3).unwrap();
            assert_eq!(dim1.value, Some(100.0));
            assert_eq!(dim2.value, Some(200.0));
            assert_eq!(dim3.value, Some(300.0));

            // Slot 20: only dim2 gets new data, dim1 and dim3 should gap-fill
            let slot_ts = mgr.ingest(2, 220.0, ns(25), 0, &mut dimensions);
            assert_eq!(slot_ts, Some(10)); // Slot 10 finalized

            // Verify slot 10: dim1=110 (new), dim2=200 (gap-fill), dim3=300 (gap-fill)
            let dim1 = dimensions.iter().find(|d| d.dimension_id == 1).unwrap();
            let dim2 = dimensions.iter().find(|d| d.dimension_id == 2).unwrap();
            let dim3 = dimensions.iter().find(|d| d.dimension_id == 3).unwrap();
            assert_eq!(dim1.value, Some(110.0)); // New data
            assert_eq!(dim2.value, Some(200.0)); // Gap-filled from slot 0
            assert_eq!(dim3.value, Some(300.0)); // Gap-filled from slot 0
        }

        #[test]
        fn delta_sum_accumulates_correctly_with_incremental_ingest() {
            // Verifies that delta sums are accumulated correctly when
            // ingested incrementally (not batched).
            let mut mgr: SlotManager<DeltaSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);
            let mut dimensions = Vec::new();

            // Multiple deltas for same dimension in same slot
            mgr.ingest(1, 5.0, ns(1), 0, &mut dimensions);
            mgr.ingest(1, 10.0, ns(2), ns(1), &mut dimensions);
            mgr.ingest(1, 15.0, ns(3), ns(2), &mut dimensions);
            mgr.ingest(1, 20.0, ns(4), ns(3), &mut dimensions);

            mgr.finalize(&mut dimensions).unwrap();

            // Should sum all deltas: 5 + 10 + 15 + 20 = 50
            assert_eq!(dimensions[0].value, Some(50.0));
        }
    }
}
