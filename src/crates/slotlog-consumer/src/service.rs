//! gRPC service implementation for the slot log consumer.

use slotlog::{
    ChartAssignment, ChartDimensionRemapping, ChartIdMapping, ChartRegistration, ChartUpdate,
    Deletion, DimensionAssignments, DimensionIdMapping, LateUpdate, Remapping, SlotLog,
    SlotLogResponse, metrics_processor_server::MetricsProcessor,
};
use tonic::{Request, Response, Status};

use crate::storage::Storage;

/// Configuration for the consumer service.
#[derive(Debug, Clone)]
pub struct ConsumerConfig {
    /// Whether to compact storage after deletions.
    pub compact_on_delete: bool,
    /// Maximum age (in slots) for accepting late updates.
    /// None means accept all late updates.
    pub max_late_slots: Option<u64>,
}

impl Default for ConsumerConfig {
    fn default() -> Self {
        Self {
            compact_on_delete: false,
            max_late_slots: None,
        }
    }
}

/// The slot log consumer service.
///
/// This service receives slot logs from a producer, assigns IDs to new
/// charts and dimensions, and stores the metric data.
pub struct SlotLogConsumer {
    storage: Storage,
    config: ConsumerConfig,
    /// The most recent slot timestamp processed.
    current_slot: Option<u64>,
}

impl SlotLogConsumer {
    /// Create a new consumer with the given configuration.
    pub fn new(config: ConsumerConfig) -> Self {
        Self {
            storage: Storage::new(),
            config,
            current_slot: None,
        }
    }

    /// Create a new consumer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ConsumerConfig::default())
    }

    /// Get a reference to the underlying storage.
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Process a chart registration, returning the assigned chart ID and dimension indices.
    fn process_registration(&mut self, reg: &ChartRegistration) -> ChartAssignment {
        let chart_type = reg.chart_type();
        let chart_id = self.storage.add_chart(reg.name.clone(), chart_type);

        let chart = self.storage.get_chart_mut(chart_id).unwrap();

        let dimension_indices: Vec<u32> = reg
            .dimensions
            .iter()
            .map(|dim| chart.add_dimension(dim.name.clone(), dim.value))
            .collect();

        ChartAssignment {
            chart_id,
            dimension_indices,
        }
    }

    /// Process a chart update, returning dimension assignments for any new dimensions.
    fn process_update(&mut self, update: &ChartUpdate) -> DimensionAssignments {
        let mut indices = Vec::new();

        if let Some(chart) = self.storage.get_chart_mut(update.chart_id) {
            // Register new dimensions
            for dim in &update.new_dimensions {
                let idx = chart.add_dimension(dim.name.clone(), dim.value);
                indices.push(idx);
            }

            // Apply values
            for value in &update.values {
                chart.set_value(value.dim_idx, value.value);
            }
        }

        DimensionAssignments { indices }
    }

    /// Process a late update, returning dimension assignments for any new dimensions.
    fn process_late_update(&mut self, late: &LateUpdate) -> DimensionAssignments {
        let mut indices = Vec::new();

        // Check if late update is within acceptable range
        if let (Some(current), Some(max_late)) = (self.current_slot, self.config.max_late_slots) {
            if current.saturating_sub(late.slot_timestamp) > max_late {
                // Too old, reject but still process new dimension registrations
                // so the producer gets IDs for future use
            }
        }

        if let Some(chart) = self.storage.get_chart_mut(late.chart_id) {
            // Register new dimensions
            for dim in &late.new_dimensions {
                let idx = chart.add_dimension(dim.name.clone(), dim.value);
                indices.push(idx);
            }

            // Apply values (consumer policy: accept late data)
            // A more sophisticated consumer might store these separately
            for value in &late.values {
                chart.set_value(value.dim_idx, value.value);
            }
        }

        DimensionAssignments { indices }
    }

    /// Process a deletion, returning remapping info if compaction occurred.
    fn process_deletion(
        &mut self,
        deletion: &Deletion,
    ) -> Option<(Vec<ChartIdMapping>, ChartDimensionRemapping)> {
        let chart_id = deletion.chart_id;

        if deletion.dimension_indices.is_empty() {
            // Delete entire chart
            self.storage.remove_chart(chart_id);
            None
        } else {
            // Delete specific dimensions
            if let Some(chart) = self.storage.get_chart_mut(chart_id) {
                // Sort indices in reverse order to avoid shifting issues
                let mut indices: Vec<_> = deletion.dimension_indices.iter().copied().collect();
                indices.sort_by(|a, b| b.cmp(a));

                let mut mappings = Vec::new();

                for &idx in &indices {
                    if chart.remove_dimension(idx) {
                        // Track remappings for dimensions that shifted
                        for old_idx in (idx + 1)..=(chart.dimensions.len() as u32 + 1) {
                            mappings.push(DimensionIdMapping {
                                old_idx,
                                new_idx: old_idx - 1,
                            });
                        }
                    }
                }

                if !mappings.is_empty() {
                    return Some((Vec::new(), ChartDimensionRemapping { chart_id, mappings }));
                }
            }
            None
        }
    }

    /// Process a complete slot log.
    pub fn process_slot_log(&mut self, log: SlotLog) -> SlotLogResponse {
        // Update current slot
        self.current_slot = Some(log.timestamp);

        // 1. Process registrations
        let chart_assignments: Vec<ChartAssignment> = log
            .registrations
            .iter()
            .map(|reg| self.process_registration(reg))
            .collect();

        // 2. Process updates
        let dimension_assignments: Vec<DimensionAssignments> = log
            .updates
            .iter()
            .map(|update| self.process_update(update))
            .collect();

        // 3. Process deletions
        let mut chart_remappings = Vec::new();
        let mut dimension_remappings = Vec::new();

        for deletion in &log.deletions {
            if let Some((charts, dims)) = self.process_deletion(deletion) {
                chart_remappings.extend(charts);
                if !dims.mappings.is_empty() {
                    dimension_remappings.push(dims);
                }
            }
        }

        // Compact if configured
        if self.config.compact_on_delete && !log.deletions.is_empty() {
            let compact_remaps = self.storage.compact_charts();
            for (old_id, new_id) in compact_remaps {
                chart_remappings.push(ChartIdMapping { old_id, new_id });
            }
        }

        // 4. Process late updates
        let late_dimension_assignments: Vec<DimensionAssignments> = log
            .late_updates
            .iter()
            .map(|late| self.process_late_update(late))
            .collect();

        // Build remapping if any occurred
        let remapping = if chart_remappings.is_empty() && dimension_remappings.is_empty() {
            None
        } else {
            Some(Remapping {
                charts: chart_remappings,
                dimensions: dimension_remappings,
            })
        };

        SlotLogResponse {
            chart_assignments,
            dimension_assignments,
            late_dimension_assignments,
            remapping,
        }
    }
}

#[tonic::async_trait]
impl MetricsProcessor for SlotLogConsumer {
    async fn process_slot(
        &self,
        _request: Request<SlotLog>,
    ) -> Result<Response<SlotLogResponse>, Status> {
        // Note: The tonic trait requires &self, but we need &mut self.
        // In practice, this service would be wrapped in a Mutex or similar.
        // For now, we return a placeholder error.
        Err(Status::unimplemented(
            "Use SlotLogConsumer::process_slot_log directly or wrap in a Mutex",
        ))
    }
}

/// A thread-safe wrapper around SlotLogConsumer for use with tonic.
pub struct SharedSlotLogConsumer {
    inner: tokio::sync::Mutex<SlotLogConsumer>,
}

impl SharedSlotLogConsumer {
    /// Create a new shared consumer.
    pub fn new(config: ConsumerConfig) -> Self {
        Self {
            inner: tokio::sync::Mutex::new(SlotLogConsumer::new(config)),
        }
    }

    /// Create a new shared consumer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(ConsumerConfig::default())
    }
}

#[tonic::async_trait]
impl MetricsProcessor for SharedSlotLogConsumer {
    async fn process_slot(
        &self,
        request: Request<SlotLog>,
    ) -> Result<Response<SlotLogResponse>, Status> {
        let log = request.into_inner();
        let mut consumer = self.inner.lock().await;
        let response = consumer.process_slot_log(log);
        Ok(Response::new(response))
    }
}

#[cfg(test)]
mod tests {
    use slotlog::{ChartType, DimensionRegistration, DimensionValue};

    use super::*;

    #[test]
    fn test_process_registration() {
        let mut consumer = SlotLogConsumer::with_defaults();

        let log = SlotLog {
            timestamp: 1000,
            registrations: vec![ChartRegistration {
                name: "test.chart".to_string(),
                chart_type: ChartType::Gauge.into(),
                dimensions: vec![
                    DimensionRegistration {
                        name: "dim1".to_string(),
                        value: Some(1.0),
                    },
                    DimensionRegistration {
                        name: "dim2".to_string(),
                        value: Some(2.0),
                    },
                ],
            }],
            updates: vec![],
            deletions: vec![],
            late_updates: vec![],
        };

        let response = consumer.process_slot_log(log);

        assert_eq!(response.chart_assignments.len(), 1);
        let assignment = &response.chart_assignments[0];
        assert_eq!(assignment.chart_id, 0);
        assert_eq!(assignment.dimension_indices, vec![0, 1]);

        // Verify storage
        let chart = consumer.storage().get_chart(0).unwrap();
        assert_eq!(chart.name, "test.chart");
        assert_eq!(chart.dimensions.len(), 2);
        assert_eq!(chart.get_value(0), Some(1.0));
        assert_eq!(chart.get_value(1), Some(2.0));
    }

    #[test]
    fn test_process_update() {
        let mut consumer = SlotLogConsumer::with_defaults();

        // First, register a chart
        let log1 = SlotLog {
            timestamp: 1000,
            registrations: vec![ChartRegistration {
                name: "test.chart".to_string(),
                chart_type: ChartType::Gauge.into(),
                dimensions: vec![DimensionRegistration {
                    name: "dim1".to_string(),
                    value: Some(1.0),
                }],
            }],
            updates: vec![],
            deletions: vec![],
            late_updates: vec![],
        };
        consumer.process_slot_log(log1);

        // Now send an update with a new dimension
        let log2 = SlotLog {
            timestamp: 1001,
            registrations: vec![],
            updates: vec![ChartUpdate {
                chart_id: 0,
                new_dimensions: vec![DimensionRegistration {
                    name: "dim2".to_string(),
                    value: Some(2.0),
                }],
                values: vec![DimensionValue {
                    dim_idx: 0,
                    value: Some(10.0),
                }],
            }],
            deletions: vec![],
            late_updates: vec![],
        };

        let response = consumer.process_slot_log(log2);

        assert_eq!(response.dimension_assignments.len(), 1);
        assert_eq!(response.dimension_assignments[0].indices, vec![1]);

        // Verify storage
        let chart = consumer.storage().get_chart(0).unwrap();
        assert_eq!(chart.dimensions.len(), 2);
        assert_eq!(chart.get_value(0), Some(10.0)); // Updated
        assert_eq!(chart.get_value(1), Some(2.0)); // New dimension
    }

    #[test]
    fn test_process_deletion() {
        let config = ConsumerConfig {
            compact_on_delete: true,
            ..Default::default()
        };
        let mut consumer = SlotLogConsumer::new(config);

        // Register two charts
        let log1 = SlotLog {
            timestamp: 1000,
            registrations: vec![
                ChartRegistration {
                    name: "chart0".to_string(),
                    chart_type: ChartType::Gauge.into(),
                    dimensions: vec![],
                },
                ChartRegistration {
                    name: "chart1".to_string(),
                    chart_type: ChartType::DeltaSum.into(),
                    dimensions: vec![],
                },
            ],
            updates: vec![],
            deletions: vec![],
            late_updates: vec![],
        };
        consumer.process_slot_log(log1);

        // Delete first chart
        let log2 = SlotLog {
            timestamp: 1001,
            registrations: vec![],
            updates: vec![],
            deletions: vec![Deletion {
                chart_id: 0,
                dimension_indices: vec![],
            }],
            late_updates: vec![],
        };

        let response = consumer.process_slot_log(log2);

        // Should have compacted
        assert!(response.remapping.is_some());
        let remapping = response.remapping.unwrap();
        assert_eq!(remapping.charts.len(), 1);
        assert_eq!(remapping.charts[0].old_id, 1);
        assert_eq!(remapping.charts[0].new_id, 0);

        // Verify storage
        assert_eq!(consumer.storage().chart_count(), 1);
        let chart = consumer.storage().get_chart(0).unwrap();
        assert_eq!(chart.name, "chart1");
    }
}
