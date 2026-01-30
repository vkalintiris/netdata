//! Slot management for mapping OpenTelemetry's event-based metrics to Netdata's
//! fixed-interval collection model.

use std::time::Instant;

use crate::aggregation::Aggregator;

/// A dimension with its name and aggregator state.
pub struct Dimension<A: Aggregator> {
    name: String,
    aggregator: A,
    had_data_this_slot: bool,
}

impl<A: Aggregator + Default> Dimension<A> {
    /// Create a new dimension with the given name.
    pub fn new(name: String) -> Self {
        Self {
            name,
            aggregator: A::default(),
            had_data_this_slot: false,
        }
    }

    /// Get the dimension name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ingest a data point into this dimension.
    pub fn ingest(&mut self, value: f64, timestamp_ns: u64, start_time_ns: u64) {
        self.aggregator.ingest(value, timestamp_ns, start_time_ns);
        self.had_data_this_slot = true;
    }

    /// Finalize this dimension for the current slot.
    /// Returns the value to emit (None means skip this dimension in output).
    /// Resets the had_data flag for the next slot.
    pub fn finalize_slot(&mut self) -> Option<f64> {
        let value = if self.had_data_this_slot {
            self.aggregator.finalize_slot()
        } else {
            Some(self.aggregator.gap_fill())
        };
        self.had_data_this_slot = false;
        value
    }
}

/// Configuration for slot timing (stateless, can be shared).
#[derive(Debug, Clone, Copy)]
pub struct SlotConfig {
    /// Collection interval in seconds
    pub interval_secs: u64,
    /// Grace period in seconds before finalizing an idle slot
    pub grace_period_secs: u64,
}

impl Default for SlotConfig {
    fn default() -> Self {
        Self {
            interval_secs: 10,
            grace_period_secs: 60,
        }
    }
}

impl SlotConfig {
    /// Compute the slot timestamp for a given nanosecond timestamp.
    pub fn slot_for_timestamp(&self, timestamp_ns: u64) -> u64 {
        let timestamp_secs = timestamp_ns / 1_000_000_000;
        (timestamp_secs / self.interval_secs) * self.interval_secs
    }
}

/// Per-chart slot state.
#[derive(Default)]
pub struct SlotState {
    /// The currently active slot timestamp (if any)
    pub active_slot: Option<u64>,
    /// When the active slot last received data (for grace period timeout)
    pub last_data_instant: Option<Instant>,
}

impl SlotState {
    /// Check if there's an active slot.
    pub fn has_active_slot(&self) -> bool {
        self.active_slot.is_some()
    }

    /// Get the active slot timestamp.
    pub fn active_slot_timestamp(&self) -> Option<u64> {
        self.active_slot
    }

    /// Check if the grace period has expired.
    pub fn check_grace_period(&self, config: &SlotConfig) -> Option<u64> {
        let last_data = self.last_data_instant?;
        let grace_period = std::time::Duration::from_secs(config.grace_period_secs);

        if last_data.elapsed() >= grace_period {
            self.active_slot
        } else {
            None
        }
    }

    /// Clear the active slot after grace period finalization.
    pub fn clear(&mut self) {
        self.active_slot = None;
        self.last_data_instant = None;
    }

    /// Record that data was received now.
    pub fn touch(&mut self) {
        self.last_data_instant = Some(Instant::now());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregation::{CumulativeSumAggregator, DeltaSumAggregator, GaugeAggregator};

    const INTERVAL_SECS: u64 = 10;
    const GRACE_PERIOD_SECS: u64 = 5;

    fn test_config() -> SlotConfig {
        SlotConfig {
            interval_secs: INTERVAL_SECS,
            grace_period_secs: GRACE_PERIOD_SECS,
        }
    }

    fn ns(secs: u64) -> u64 {
        secs * 1_000_000_000
    }

    mod slot_config {
        use super::*;

        #[test]
        fn slot_assignment() {
            let config = test_config();

            assert_eq!(config.slot_for_timestamp(ns(0)), 0);
            assert_eq!(config.slot_for_timestamp(ns(5)), 0);
            assert_eq!(config.slot_for_timestamp(ns(9)), 0);
            assert_eq!(config.slot_for_timestamp(ns(10)), 10);
            assert_eq!(config.slot_for_timestamp(ns(15)), 10);
            assert_eq!(config.slot_for_timestamp(ns(25)), 20);
        }
    }

    mod dimension {
        use super::*;

        #[test]
        fn gauge_keeps_last_value_by_timestamp() {
            let mut dim: Dimension<GaugeAggregator> = Dimension::new("cpu".to_string());

            dim.ingest(10.0, ns(1), 0);
            dim.ingest(30.0, ns(3), 0); // Latest
            dim.ingest(20.0, ns(2), 0);

            assert_eq!(dim.finalize_slot(), Some(30.0));
        }

        #[test]
        fn gauge_gap_fills_with_last_value() {
            let mut dim: Dimension<GaugeAggregator> = Dimension::new("cpu".to_string());

            // First slot with data
            dim.ingest(42.0, ns(5), 0);
            assert_eq!(dim.finalize_slot(), Some(42.0));

            // Second slot without data - gap fill
            assert_eq!(dim.finalize_slot(), Some(42.0));
        }

        #[test]
        fn delta_sum_accumulates() {
            let mut dim: Dimension<DeltaSumAggregator> = Dimension::new("requests".to_string());

            dim.ingest(10.0, ns(1), 0);
            dim.ingest(20.0, ns(2), ns(1));
            dim.ingest(5.0, ns(3), ns(2));

            assert_eq!(dim.finalize_slot(), Some(35.0));
        }

        #[test]
        fn cumulative_sum_first_slot_returns_none() {
            let mut dim: Dimension<CumulativeSumAggregator> =
                Dimension::new("counter".to_string());

            dim.ingest(100.0, ns(5), 1_000_000_000);

            assert_eq!(dim.finalize_slot(), None);
        }

        #[test]
        fn cumulative_sum_computes_delta() {
            let mut dim: Dimension<CumulativeSumAggregator> =
                Dimension::new("counter".to_string());
            let start_time = 1_000_000_000u64;

            // First slot - baseline
            dim.ingest(100.0, ns(5), start_time);
            assert_eq!(dim.finalize_slot(), None);

            // Second slot - delta
            dim.ingest(150.0, ns(15), start_time);
            assert_eq!(dim.finalize_slot(), Some(50.0));
        }
    }

    mod slot_state {
        use super::*;

        #[test]
        fn initially_no_active_slot() {
            let state = SlotState::default();
            assert!(!state.has_active_slot());
            assert_eq!(state.active_slot_timestamp(), None);
        }

        #[test]
        fn clear_resets_state() {
            let mut state = SlotState::default();
            state.active_slot = Some(10);
            state.touch();

            state.clear();

            assert!(!state.has_active_slot());
            assert!(state.last_data_instant.is_none());
        }
    }
}
