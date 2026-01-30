//! Dimension types for chart data management.

use crate::aggregation::Aggregator;

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
