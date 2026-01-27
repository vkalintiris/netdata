//! Slot management for mapping OpenTelemetry's event-based metrics to Netdata's
//! fixed-interval collection model.
//!
//! The `SlotManager` tracks a single active slot and:
//! - Drops data for previous slots
//! - Finalizes the active slot when data arrives for a newer slot
//! - Finalizes the active slot after a grace period with no data

use std::collections::{BTreeSet, HashMap};
use std::time::Instant;

use crate::aggregation::Aggregator;

/// Identifier for a dimension within a chart.
pub type DimensionId = u64;

/// A data point buffered for later processing.
#[derive(Debug, Clone, Copy)]
struct BufferedPoint {
    value: f64,
    timestamp_ns: u64,
    start_time_ns: u64,
}

/// Result of finalizing a slot - contains values for all dimensions.
#[derive(Debug)]
pub struct FinalizedSlot {
    /// The slot timestamp (start of the interval)
    pub slot_timestamp: u64,
    /// Values for each dimension
    pub dimensions: Vec<FinalizedDimension>,
}

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
    interval_secs: u64,
    /// Grace period in seconds before finalizing an idle slot
    grace_period_secs: u64,

    /// Per-dimension aggregators (maintain cross-slot state)
    dimensions: HashMap<DimensionId, A>,

    /// All known dimensions (for gap filling)
    known_dimensions: BTreeSet<DimensionId>,

    /// The currently active slot timestamp (if any)
    active_slot: Option<u64>,

    /// Buffered points for the active slot: dimension_id -> points
    buffered: HashMap<DimensionId, Vec<BufferedPoint>>,

    /// When the active slot last received data (for grace period timeout)
    last_data_instant: Option<Instant>,
}

impl<A: Aggregator + Default> SlotManager<A> {
    /// Create a new slot manager with the given timing configuration.
    pub fn new(interval_secs: u64, grace_period_secs: u64) -> Self {
        Self {
            interval_secs,
            grace_period_secs,
            dimensions: HashMap::new(),
            known_dimensions: BTreeSet::new(),
            active_slot: None,
            buffered: HashMap::new(),
            last_data_instant: None,
        }
    }

    /// Compute the slot timestamp for a given nanosecond timestamp.
    fn slot_for_timestamp(&self, timestamp_ns: u64) -> u64 {
        let timestamp_secs = timestamp_ns / 1_000_000_000;
        (timestamp_secs / self.interval_secs) * self.interval_secs
    }

    /// Ingest a data point for a dimension.
    ///
    /// Returns `Some(FinalizedSlot)` if this ingestion caused the previous
    /// active slot to be finalized (because data arrived for a newer slot).
    /// Returns `None` if the data was buffered or dropped.
    pub fn ingest(
        &mut self,
        dimension_id: DimensionId,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
    ) -> Option<FinalizedSlot> {
        let data_slot = self.slot_for_timestamp(timestamp_ns);

        // Track this dimension
        self.known_dimensions.insert(dimension_id);
        self.dimensions.entry(dimension_id).or_default();

        let finalized = match self.active_slot {
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
                let finalized = self.finalize_active_slot();
                self.active_slot = Some(data_slot);
                finalized
            }
            Some(_) => {
                // Data for the current active slot
                None
            }
        };

        // Buffer the point for the active slot
        self.buffered
            .entry(dimension_id)
            .or_default()
            .push(BufferedPoint {
                value,
                timestamp_ns,
                start_time_ns,
            });

        // Update last data time
        self.last_data_instant = Some(Instant::now());

        finalized
    }

    /// Check if the grace period has expired and finalize if so.
    ///
    /// Returns `Some(FinalizedSlot)` if the active slot was finalized due to
    /// grace period expiration, `None` otherwise.
    pub fn tick(&mut self) -> Option<FinalizedSlot> {
        let last_data = self.last_data_instant?;
        let grace_period = std::time::Duration::from_secs(self.grace_period_secs);

        if last_data.elapsed() >= grace_period {
            self.finalize_active_slot()
        } else {
            None
        }
    }

    /// Finalize the active slot and return it.
    fn finalize_active_slot(&mut self) -> Option<FinalizedSlot> {
        let slot_timestamp = self.active_slot.take()?;

        let mut dimensions = Vec::with_capacity(self.known_dimensions.len());

        for &dim_id in &self.known_dimensions {
            let aggregator = self.dimensions.get_mut(&dim_id);
            let buffered_points = self.buffered.remove(&dim_id);

            let value = match (aggregator, buffered_points) {
                (Some(agg), Some(points)) if !points.is_empty() => {
                    // Feed all buffered points to the aggregator
                    for point in points {
                        agg.ingest(point.value, point.timestamp_ns, point.start_time_ns);
                    }
                    agg.finalize_slot()
                }
                (Some(agg), _) => {
                    // No data for this dimension in this slot - gap fill
                    Some(agg.gap_fill())
                }
                (None, _) => None,
            };

            dimensions.push(FinalizedDimension {
                dimension_id: dim_id,
                value,
            });
        }

        // Clear any remaining buffered data and reset timing
        self.buffered.clear();
        self.last_data_instant = None;

        Some(FinalizedSlot {
            slot_timestamp,
            dimensions,
        })
    }

    /// Force finalize the active slot. Useful for shutdown or testing.
    pub fn finalize(&mut self) -> Option<FinalizedSlot> {
        self.finalize_active_slot()
    }

    /// Check if there's an active slot.
    pub fn has_active_slot(&self) -> bool {
        self.active_slot.is_some()
    }

    /// Get the number of known dimensions.
    pub fn dimension_count(&self) -> usize {
        self.known_dimensions.len()
    }

    /// Check if a dimension exists.
    pub fn has_dimension(&self, dimension_id: DimensionId) -> bool {
        self.known_dimensions.contains(&dimension_id)
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

            assert!(!mgr.has_dimension(1));

            mgr.ingest(1, 42.0, ns(5), 0);

            assert!(mgr.has_dimension(1));
            assert_eq!(mgr.dimension_count(), 1);
        }

        #[test]
        fn ingest_sets_active_slot() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            assert!(!mgr.has_active_slot());

            mgr.ingest(1, 42.0, ns(5), 0);

            assert!(mgr.has_active_slot());
        }

        #[test]
        fn drops_data_for_previous_slot() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Set active slot to 10
            mgr.ingest(1, 50.0, ns(15), 0);

            // Try to ingest for slot 0 - should be dropped
            let result = mgr.ingest(1, 42.0, ns(5), 0);
            assert!(result.is_none());

            // Finalize and check only the slot 10 value is present
            let finalized = mgr.finalize().unwrap();
            assert_eq!(finalized.slot_timestamp, 10);
            assert_eq!(finalized.dimensions[0].value, Some(50.0));
        }
    }

    mod finalization {
        use super::*;

        #[test]
        fn finalizes_on_newer_slot_data() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0
            mgr.ingest(1, 42.0, ns(5), 0);

            // Slot 10 - should finalize slot 0
            let finalized = mgr.ingest(1, 50.0, ns(15), 0);

            assert!(finalized.is_some());
            let finalized = finalized.unwrap();
            assert_eq!(finalized.slot_timestamp, 0);
            assert_eq!(finalized.dimensions[0].value, Some(42.0));
        }

        #[test]
        fn no_finalization_for_same_slot() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 42.0, ns(5), 0);
            let result = mgr.ingest(1, 43.0, ns(7), 0);

            assert!(result.is_none());
            assert!(mgr.has_active_slot());
        }

        #[test]
        fn force_finalize() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 42.0, ns(5), 0);

            let finalized = mgr.finalize();

            assert!(finalized.is_some());
            assert!(!mgr.has_active_slot());
        }
    }

    mod gauge_aggregation {
        use super::*;

        #[test]
        fn keeps_last_value_by_timestamp() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 10.0, ns(1), 0);
            mgr.ingest(1, 30.0, ns(3), 0); // Latest
            mgr.ingest(1, 20.0, ns(2), 0);

            let finalized = mgr.finalize().unwrap();
            assert_eq!(finalized.dimensions[0].value, Some(30.0));
        }

        #[test]
        fn gap_fills_missing_dimension() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0: both dimensions
            mgr.ingest(1, 10.0, ns(5), 0);
            mgr.ingest(2, 20.0, ns(5), 0);

            // Finalize slot 0 via newer slot data, only dim 1 has data
            mgr.ingest(1, 15.0, ns(15), 0);
            let finalized = mgr.finalize().unwrap();

            // Dim 2 should be gap-filled with previous value (20.0)
            let dim2 = finalized
                .dimensions
                .iter()
                .find(|d| d.dimension_id == 2)
                .unwrap();
            assert_eq!(dim2.value, Some(20.0));
        }
    }

    mod delta_sum_aggregation {
        use super::*;

        #[test]
        fn sums_deltas() {
            let mut mgr: SlotManager<DeltaSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 10.0, ns(1), 0);
            mgr.ingest(1, 20.0, ns(2), ns(1));
            mgr.ingest(1, 5.0, ns(3), ns(2));

            let finalized = mgr.finalize().unwrap();
            assert_eq!(finalized.dimensions[0].value, Some(35.0));
        }
    }

    mod cumulative_sum_aggregation {
        use super::*;

        const START_TIME: u64 = 1_000_000_000;

        #[test]
        fn first_slot_returns_none() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 100.0, ns(5), START_TIME);

            let finalized = mgr.finalize().unwrap();
            assert_eq!(finalized.dimensions[0].value, None);
        }

        #[test]
        fn computes_deltas_across_slots() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0: baseline
            mgr.ingest(1, 100.0, ns(5), START_TIME);

            // Slot 10: finalize slot 0 and buffer slot 10 data
            let finalized = mgr.ingest(1, 150.0, ns(15), START_TIME);
            assert!(finalized.is_some());
            assert_eq!(finalized.unwrap().dimensions[0].value, None); // First slot

            // Finalize slot 10
            let finalized = mgr.finalize().unwrap();
            assert_eq!(finalized.dimensions[0].value, Some(50.0)); // 150 - 100
        }

        #[test]
        fn detects_restart() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Establish baseline
            mgr.ingest(1, 100.0, ns(5), START_TIME);
            mgr.ingest(1, 150.0, ns(15), START_TIME);
            mgr.finalize();

            // Restart with new start_time
            let new_start = START_TIME + 1_000_000;
            mgr.ingest(1, 20.0, ns(25), new_start);

            let finalized = mgr.finalize().unwrap();
            assert_eq!(finalized.dimensions[0].value, Some(0.0));
        }
    }
}
