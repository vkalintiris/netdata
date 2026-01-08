//! Dense vector-based storage for charts and dimensions.
//!
//! The consumer uses dense vectors indexed by consumer-assigned IDs,
//! providing O(1) access to chart and dimension data.

use slotlog::ChartType;

/// Metadata for a single dimension within a chart.
#[derive(Debug, Clone)]
pub struct DimensionMeta {
    /// Human-readable name for display/logging.
    pub name: String,
}

/// A single chart's data and metadata.
#[derive(Debug)]
pub struct ChartData {
    /// Human-readable name for display/logging.
    pub name: String,
    /// The aggregation type for this chart.
    pub chart_type: ChartType,
    /// Dimensions stored as a dense vector, indexed by dimension index.
    pub dimensions: Vec<DimensionMeta>,
    /// Current values for each dimension (same indexing as dimensions).
    /// None indicates no value / unknown.
    pub values: Vec<Option<f64>>,
}

impl ChartData {
    /// Create a new chart with the given metadata.
    pub fn new(name: String, chart_type: ChartType) -> Self {
        Self {
            name,
            chart_type,
            dimensions: Vec::new(),
            values: Vec::new(),
        }
    }

    /// Add a new dimension to this chart.
    /// Returns the assigned dimension index.
    pub fn add_dimension(&mut self, name: String, initial_value: Option<f64>) -> u32 {
        let idx = self.dimensions.len() as u32;
        self.dimensions.push(DimensionMeta { name });
        self.values.push(initial_value);
        idx
    }

    /// Set the value for a dimension.
    pub fn set_value(&mut self, dim_idx: u32, value: Option<f64>) {
        if let Some(slot) = self.values.get_mut(dim_idx as usize) {
            *slot = value;
        }
    }

    /// Get the current value for a dimension.
    pub fn get_value(&self, dim_idx: u32) -> Option<f64> {
        self.values.get(dim_idx as usize).copied().flatten()
    }

    /// Remove a dimension by index. Returns true if removed.
    /// Note: This shifts indices! Use with care and return remapping info.
    pub fn remove_dimension(&mut self, dim_idx: u32) -> bool {
        let idx = dim_idx as usize;
        if idx < self.dimensions.len() {
            self.dimensions.remove(idx);
            self.values.remove(idx);
            true
        } else {
            false
        }
    }
}

/// The main storage container for all charts.
#[derive(Debug, Default)]
pub struct Storage {
    /// Charts stored as a dense vector, indexed by chart ID.
    charts: Vec<Option<ChartData>>,
}

impl Storage {
    /// Create a new empty storage.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new chart and return its assigned ID.
    ///
    /// Always appends to the end of the storage (O(1)).
    /// Use `compact_charts` to reclaim space from deleted charts.
    pub fn add_chart(&mut self, name: String, chart_type: ChartType) -> u32 {
        let idx = self.charts.len() as u32;
        self.charts.push(Some(ChartData::new(name, chart_type)));
        idx
    }

    /// Get a chart by ID.
    pub fn get_chart(&self, chart_id: u32) -> Option<&ChartData> {
        self.charts.get(chart_id as usize).and_then(|c| c.as_ref())
    }

    /// Get a mutable reference to a chart by ID.
    pub fn get_chart_mut(&mut self, chart_id: u32) -> Option<&mut ChartData> {
        self.charts
            .get_mut(chart_id as usize)
            .and_then(|c| c.as_mut())
    }

    /// Remove a chart by ID. Returns true if removed.
    pub fn remove_chart(&mut self, chart_id: u32) -> bool {
        if let Some(slot) = self.charts.get_mut(chart_id as usize) {
            if slot.is_some() {
                *slot = None;
                return true;
            }
        }
        false
    }

    /// Compact storage by removing None entries and returning remapping info.
    /// Returns a vector of (old_id, new_id) pairs for charts that moved.
    pub fn compact_charts(&mut self) -> Vec<(u32, u32)> {
        let mut remappings = Vec::new();
        let mut write_idx = 0;

        for read_idx in 0..self.charts.len() {
            if self.charts[read_idx].is_some() {
                if read_idx != write_idx {
                    self.charts.swap(read_idx, write_idx);
                    remappings.push((read_idx as u32, write_idx as u32));
                }
                write_idx += 1;
            }
        }

        self.charts.truncate(write_idx);
        remappings
    }

    /// Get the number of active charts.
    pub fn chart_count(&self) -> usize {
        self.charts.iter().filter(|c| c.is_some()).count()
    }

    /// Iterate over all active charts with their IDs.
    pub fn iter_charts(&self) -> impl Iterator<Item = (u32, &ChartData)> {
        self.charts
            .iter()
            .enumerate()
            .filter_map(|(idx, opt)| opt.as_ref().map(|chart| (idx as u32, chart)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_get_chart() {
        let mut storage = Storage::new();
        let id = storage.add_chart("test.chart".to_string(), ChartType::Gauge);
        assert_eq!(id, 0);

        let chart = storage.get_chart(id).unwrap();
        assert_eq!(chart.name, "test.chart");
        assert_eq!(chart.chart_type, ChartType::Gauge);
    }

    #[test]
    fn test_add_dimension() {
        let mut storage = Storage::new();
        let chart_id = storage.add_chart("test.chart".to_string(), ChartType::Gauge);

        let chart = storage.get_chart_mut(chart_id).unwrap();
        let dim_idx = chart.add_dimension("dim1".to_string(), Some(42.0));
        assert_eq!(dim_idx, 0);

        assert_eq!(chart.get_value(0), Some(42.0));
    }

    #[test]
    fn test_remove_and_compact() {
        let mut storage = Storage::new();
        let id0 = storage.add_chart("chart0".to_string(), ChartType::Gauge);
        let id1 = storage.add_chart("chart1".to_string(), ChartType::DeltaSum);
        let id2 = storage.add_chart("chart2".to_string(), ChartType::CumulativeSum);

        assert_eq!(id0, 0);
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        // Remove middle chart
        assert!(storage.remove_chart(id1));
        assert_eq!(storage.chart_count(), 2);

        // Compact - chart2 should move to slot 1
        let remappings = storage.compact_charts();
        assert_eq!(remappings, vec![(2, 1)]);

        // Verify chart2 is now at index 1
        let chart = storage.get_chart(1).unwrap();
        assert_eq!(chart.name, "chart2");
    }

    #[test]
    fn test_always_append() {
        let mut storage = Storage::new();
        let id0 = storage.add_chart("chart0".to_string(), ChartType::Gauge);
        let _id1 = storage.add_chart("chart1".to_string(), ChartType::Gauge);

        storage.remove_chart(id0);

        // Next add should append (not reuse slot 0)
        let id2 = storage.add_chart("chart2".to_string(), ChartType::Gauge);
        assert_eq!(id2, 2); // Appended at end

        let chart = storage.get_chart(2).unwrap();
        assert_eq!(chart.name, "chart2");

        // Slot 0 is empty, slot 1 and 2 have charts
        assert!(storage.get_chart(0).is_none());
        assert!(storage.get_chart(1).is_some());

        // Compact to reclaim space
        let remappings = storage.compact_charts();
        assert_eq!(remappings, vec![(1, 0), (2, 1)]);

        // Now charts are at 0 and 1
        assert_eq!(storage.get_chart(0).unwrap().name, "chart1");
        assert_eq!(storage.get_chart(1).unwrap().name, "chart2");
    }
}
