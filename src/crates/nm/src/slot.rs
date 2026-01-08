#![allow(dead_code)]

//! Slot management for mapping OpenTelemetry's event-based metrics to Netdata's
//! fixed-interval collection model.
//!
//! The `SlotManager` handles:
//! - Assigning data points to slots based on timestamp
//! - Buffering data within the grace period
//! - Finalizing slots in order
//! - Gap-filling for dimensions without data

use std::collections::{BTreeSet, HashMap};

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
    /// (e.g., first observation for cumulative, or dimension not yet seen).
    pub value: Option<f64>,
    /// Whether this value came from gap filling (no data received for this slot)
    pub is_gap_fill: bool,
}

/// Manages slot timing and aggregation for a single chart.
///
/// Generic over the aggregator type - all dimensions in a chart use the same
/// aggregator type since they have the same metric semantics.
pub struct SlotManager<A: Aggregator + Default> {
    /// Collection interval in seconds
    interval_secs: u64,
    /// Grace period in seconds for accepting late data
    grace_period_secs: u64,

    /// Per-dimension aggregators (maintain cross-slot state)
    dimensions: HashMap<DimensionId, A>,

    /// Buffered data points: (slot_timestamp, dimension_id) -> points
    buffered: HashMap<(u64, DimensionId), Vec<BufferedPoint>>,

    /// Set of slots with pending data (ordered for sequential finalization)
    pending_slots: BTreeSet<u64>,

    /// All known dimensions (for gap filling)
    known_dimensions: BTreeSet<DimensionId>,

    /// Last finalized slot timestamp (for monotonicity enforcement)
    last_finalized_slot: Option<u64>,

    /// Timestamp of the most recent data point seen (for eager finalization)
    latest_data_timestamp_ns: u64,
}

impl<A: Aggregator + Default> SlotManager<A> {
    /// Create a new slot manager with the given timing configuration.
    pub fn new(interval_secs: u64, grace_period_secs: u64) -> Self {
        Self {
            interval_secs,
            grace_period_secs,
            dimensions: HashMap::new(),
            buffered: HashMap::new(),
            pending_slots: BTreeSet::new(),
            known_dimensions: BTreeSet::new(),
            last_finalized_slot: None,
            latest_data_timestamp_ns: 0,
        }
    }

    /// Compute the slot timestamp for a given nanosecond timestamp.
    fn slot_for_timestamp(&self, timestamp_ns: u64) -> u64 {
        let timestamp_secs = timestamp_ns / 1_000_000_000;
        (timestamp_secs / self.interval_secs) * self.interval_secs
    }

    /// Ingest a data point for a dimension.
    ///
    /// Returns `true` if the point was accepted, `false` if it was dropped
    /// (e.g., for a slot that's already been finalized).
    pub fn ingest(
        &mut self,
        dimension_id: DimensionId,
        value: f64,
        timestamp_ns: u64,
        start_time_ns: u64,
    ) -> bool {
        let slot = self.slot_for_timestamp(timestamp_ns);

        // Reject data for already-finalized slots
        if let Some(last) = self.last_finalized_slot {
            if slot <= last {
                return false;
            }
        }

        // Track this dimension
        self.known_dimensions.insert(dimension_id);

        // Ensure we have an aggregator for this dimension
        self.dimensions.entry(dimension_id).or_default();

        // Buffer the point
        self.buffered
            .entry((slot, dimension_id))
            .or_default()
            .push(BufferedPoint {
                value,
                timestamp_ns,
                start_time_ns,
            });

        // Track pending slot
        self.pending_slots.insert(slot);

        // Track latest timestamp for eager finalization
        if timestamp_ns > self.latest_data_timestamp_ns {
            self.latest_data_timestamp_ns = timestamp_ns;
        }

        true
    }

    /// Process a tick and finalize any slots that are ready.
    ///
    /// A slot is ready for finalization when:
    /// - Its time window has passed (wall_time > slot_end), AND
    /// - The grace period has expired (wall_time > slot_end + grace_period)
    ///
    /// Returns finalized slots in chronological order.
    pub fn tick(&mut self, current_time_ns: u64) -> Vec<FinalizedSlot> {
        let current_time_secs = current_time_ns / 1_000_000_000;
        let finalization_threshold = current_time_secs.saturating_sub(self.grace_period_secs);

        self.finalize_slots_before(finalization_threshold)
    }

    /// Eager finalization: finalize slots that have passed their time window
    /// and for which we've received data for a later slot.
    ///
    /// This provides low-latency emission in the happy path where data
    /// arrives promptly.
    ///
    /// Call this after ingesting data to potentially finalize older slots.
    pub fn eager_finalize(&mut self) -> Vec<FinalizedSlot> {
        if self.pending_slots.is_empty() {
            return Vec::new();
        }

        // Find the latest slot with data
        let latest_slot = *self.pending_slots.iter().next_back().unwrap();

        // Finalize all slots before the latest one (they won't receive more data
        // if we're already seeing data for a later slot)
        self.finalize_slots_before(latest_slot)
    }

    /// Finalize all slots with timestamp < threshold.
    fn finalize_slots_before(&mut self, threshold_secs: u64) -> Vec<FinalizedSlot> {
        let mut result = Vec::new();

        // Collect slots to finalize
        let slots_to_finalize: Vec<u64> = self
            .pending_slots
            .iter()
            .take_while(|&&slot| slot < threshold_secs)
            .copied()
            .collect();

        // Also check for gap slots between last_finalized and first pending
        let first_pending = slots_to_finalize.first().copied();
        let gap_slots = self.find_gap_slots(first_pending, threshold_secs);

        // Merge and sort all slots to finalize
        let mut all_slots: Vec<u64> = gap_slots
            .into_iter()
            .chain(slots_to_finalize.iter().copied())
            .collect();
        all_slots.sort_unstable();
        all_slots.dedup();

        for slot in all_slots {
            if let Some(finalized) = self.finalize_slot(slot) {
                result.push(finalized);
            }
        }

        result
    }

    /// Find gap slots between the last finalized slot and the threshold.
    fn find_gap_slots(&self, first_pending: Option<u64>, threshold_secs: u64) -> Vec<u64> {
        let start = match self.last_finalized_slot {
            Some(last) => last + self.interval_secs,
            None => return Vec::new(), // No baseline yet, can't gap-fill
        };

        let end = first_pending.unwrap_or(threshold_secs);

        let mut gaps = Vec::new();
        let mut slot = start;
        while slot < end && slot < threshold_secs {
            if !self.pending_slots.contains(&slot) {
                gaps.push(slot);
            }
            slot += self.interval_secs;
        }
        gaps
    }

    /// Finalize a single slot.
    fn finalize_slot(&mut self, slot: u64) -> Option<FinalizedSlot> {
        // Skip if already finalized
        if let Some(last) = self.last_finalized_slot {
            if slot <= last {
                return None;
            }
        }

        let mut dimensions = Vec::with_capacity(self.known_dimensions.len());

        for &dim_id in &self.known_dimensions {
            let aggregator = self.dimensions.get_mut(&dim_id);
            let buffered_points = self.buffered.remove(&(slot, dim_id));

            let (value, is_gap_fill) = match (aggregator, buffered_points) {
                (Some(agg), Some(points)) if !points.is_empty() => {
                    // Feed all buffered points to the aggregator
                    for point in points {
                        agg.ingest(point.value, point.timestamp_ns, point.start_time_ns);
                    }
                    // Finalize and get the value
                    (agg.finalize_slot(), false)
                }
                (Some(agg), _) => {
                    // No data for this slot - gap fill
                    (Some(agg.gap_fill()), true)
                }
                (None, _) => {
                    // Dimension not yet initialized (shouldn't happen)
                    (None, true)
                }
            };

            dimensions.push(FinalizedDimension {
                dimension_id: dim_id,
                value,
                is_gap_fill,
            });
        }

        // Update state
        self.pending_slots.remove(&slot);
        self.last_finalized_slot = Some(slot);

        Some(FinalizedSlot {
            slot_timestamp: slot,
            dimensions,
        })
    }

    /// Force finalize all pending slots. Useful for shutdown or testing.
    pub fn finalize_all(&mut self) -> Vec<FinalizedSlot> {
        let threshold = u64::MAX;
        self.finalize_slots_before(threshold)
    }

    /// Get the number of pending slots.
    pub fn pending_slot_count(&self) -> usize {
        self.pending_slots.len()
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
    const GRACE_PERIOD_SECS: u64 = 30; // 3 intervals

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    mod basic_operations {
        use super::*;

        #[test]
        fn slot_assignment() {
            let mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Timestamps within the same 10-second slot should map to same slot
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
        fn ingest_tracks_pending_slots() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 42.0, ns(5), 0); // slot 0
            mgr.ingest(1, 43.0, ns(15), 0); // slot 10

            assert_eq!(mgr.pending_slot_count(), 2);
        }

        #[test]
        fn rejects_data_for_finalized_slots() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Ingest and finalize slot 0
            mgr.ingest(1, 42.0, ns(5), 0);
            mgr.finalize_all();

            // Try to ingest for slot 0 again - should be rejected
            let accepted = mgr.ingest(1, 99.0, ns(5), 0);
            assert!(!accepted);
        }
    }

    mod gauge_finalization {
        use super::*;

        #[test]
        fn finalizes_with_last_value() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Multiple values in same slot - should use last by timestamp
            mgr.ingest(1, 10.0, ns(1), 0);
            mgr.ingest(1, 30.0, ns(3), 0); // Latest
            mgr.ingest(1, 20.0, ns(2), 0);

            let finalized = mgr.finalize_all();

            assert_eq!(finalized.len(), 1);
            assert_eq!(finalized[0].slot_timestamp, 0);
            assert_eq!(finalized[0].dimensions.len(), 1);
            assert_eq!(finalized[0].dimensions[0].value, Some(30.0));
            assert!(!finalized[0].dimensions[0].is_gap_fill);
        }

        #[test]
        fn gap_fills_with_last_value() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0: has data
            mgr.ingest(1, 42.0, ns(5), 0);
            mgr.finalize_all();

            // Slot 10: no data, but we force finalization
            // We need to add a slot 20 to create a gap at slot 10
            mgr.ingest(1, 50.0, ns(25), 0); // slot 20

            // Now finalize everything including the gap at slot 10
            let finalized = mgr.finalize_all();

            // Should have slots 10 and 20
            assert_eq!(finalized.len(), 2);

            // Slot 10 should be gap-filled with 42.0
            assert_eq!(finalized[0].slot_timestamp, 10);
            assert_eq!(finalized[0].dimensions[0].value, Some(42.0));
            assert!(finalized[0].dimensions[0].is_gap_fill);

            // Slot 20 should have actual data
            assert_eq!(finalized[1].slot_timestamp, 20);
            assert_eq!(finalized[1].dimensions[0].value, Some(50.0));
            assert!(!finalized[1].dimensions[0].is_gap_fill);
        }
    }

    mod delta_sum_finalization {
        use super::*;

        #[test]
        fn sums_deltas_in_slot() {
            let mut mgr: SlotManager<DeltaSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 10.0, ns(1), 0);
            mgr.ingest(1, 20.0, ns(2), ns(1));
            mgr.ingest(1, 5.0, ns(3), ns(2));

            let finalized = mgr.finalize_all();

            assert_eq!(finalized[0].dimensions[0].value, Some(35.0));
        }

        #[test]
        fn gap_fills_with_zero() {
            let mut mgr: SlotManager<DeltaSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0
            mgr.ingest(1, 10.0, ns(5), 0);
            mgr.finalize_all();

            // Skip slot 10, add slot 20
            mgr.ingest(1, 5.0, ns(25), ns(15));

            let finalized = mgr.finalize_all();

            // Slot 10 should be gap-filled with 0
            assert_eq!(finalized[0].slot_timestamp, 10);
            assert_eq!(finalized[0].dimensions[0].value, Some(0.0));
            assert!(finalized[0].dimensions[0].is_gap_fill);
        }
    }

    mod cumulative_sum_finalization {
        use super::*;

        const START_TIME: u64 = 1_000_000_000;

        #[test]
        fn first_slot_returns_none() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 100.0, ns(5), START_TIME);

            let finalized = mgr.finalize_all();

            // First observation - no delta can be computed
            assert_eq!(finalized[0].dimensions[0].value, None);
        }

        #[test]
        fn computes_deltas_across_slots() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0: establish baseline
            mgr.ingest(1, 100.0, ns(5), START_TIME);
            mgr.finalize_all();

            // Slot 10: should compute delta
            mgr.ingest(1, 150.0, ns(15), START_TIME);
            let finalized = mgr.finalize_all();

            assert_eq!(finalized[0].dimensions[0].value, Some(50.0));
        }

        #[test]
        fn detects_restart() {
            let mut mgr: SlotManager<CumulativeSumAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Establish baseline
            mgr.ingest(1, 100.0, ns(5), START_TIME);
            mgr.finalize_all();

            mgr.ingest(1, 150.0, ns(15), START_TIME);
            mgr.finalize_all();

            // Restart with new start_time
            let new_start = START_TIME + 1_000_000;
            mgr.ingest(1, 20.0, ns(25), new_start);

            let finalized = mgr.finalize_all();

            // Should return 0 for restart slot
            assert_eq!(finalized[0].dimensions[0].value, Some(0.0));
        }
    }

    mod timing {
        use super::*;

        #[test]
        fn tick_respects_grace_period() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Add data for slot 0
            mgr.ingest(1, 42.0, ns(5), 0);

            // Tick at t=20 (slot 0 ended at t=10, grace period is 30s)
            // Slot 0 should NOT be finalized yet (10 + 30 = 40)
            let finalized = mgr.tick(ns(20));
            assert!(finalized.is_empty());

            // Tick at t=45 (past grace period for slot 0)
            let finalized = mgr.tick(ns(45));
            assert_eq!(finalized.len(), 1);
            assert_eq!(finalized[0].slot_timestamp, 0);
        }

        #[test]
        fn eager_finalize_on_later_data() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Add data for slot 0
            mgr.ingest(1, 42.0, ns(5), 0);

            // No eager finalization yet - only one slot
            let finalized = mgr.eager_finalize();
            assert!(finalized.is_empty());

            // Add data for slot 10
            mgr.ingest(1, 50.0, ns(15), 0);

            // Now slot 0 should be eagerly finalized
            let finalized = mgr.eager_finalize();
            assert_eq!(finalized.len(), 1);
            assert_eq!(finalized[0].slot_timestamp, 0);
            assert_eq!(finalized[0].dimensions[0].value, Some(42.0));

            // Slot 10 still pending
            assert_eq!(mgr.pending_slot_count(), 1);
        }
    }

    mod multiple_dimensions {
        use super::*;

        #[test]
        fn tracks_multiple_dimensions() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            mgr.ingest(1, 10.0, ns(5), 0);
            mgr.ingest(2, 20.0, ns(5), 0);
            mgr.ingest(3, 30.0, ns(5), 0);

            let finalized = mgr.finalize_all();

            assert_eq!(finalized.len(), 1);
            assert_eq!(finalized[0].dimensions.len(), 3);
        }

        #[test]
        fn gap_fills_missing_dimensions() {
            let mut mgr: SlotManager<GaugeAggregator> =
                SlotManager::new(INTERVAL_SECS, GRACE_PERIOD_SECS);

            // Slot 0: both dimensions have data
            mgr.ingest(1, 10.0, ns(5), 0);
            mgr.ingest(2, 20.0, ns(5), 0);
            mgr.finalize_all();

            // Slot 10: only dimension 1 has data
            mgr.ingest(1, 15.0, ns(15), 0);

            let finalized = mgr.finalize_all();

            assert_eq!(finalized[0].dimensions.len(), 2);

            // Find dimension 2's result
            let dim2 = finalized[0]
                .dimensions
                .iter()
                .find(|d| d.dimension_id == 2)
                .unwrap();

            assert_eq!(dim2.value, Some(20.0)); // Gap-filled with previous value
            assert!(dim2.is_gap_fill);
        }
    }
}
