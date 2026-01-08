//! High-level producer implementation.
//!
//! The producer manages the lifecycle of slot logs: defining charts and
//! dimensions, accumulating updates, and sending to the consumer.

use rustc_hash::FxHashMap;
use slotlog::{
    ChartRegistration, ChartType, ChartUpdate, Deletion, DimensionRegistration, DimensionValue,
    LateUpdate, SlotLog, SlotLogResponse, metrics_processor_client::MetricsProcessorClient,
};
use tonic::transport::Channel;

/// Error type for producer operations.
#[derive(Debug)]
pub enum ProducerError {
    /// gRPC transport error.
    Transport(tonic::transport::Error),
    /// gRPC status error.
    Status(tonic::Status),
    /// Invalid URI.
    InvalidUri(String),
    /// Chart was not defined before use.
    ChartNotDefined(String),
    /// Dimension was not defined before use.
    DimensionNotDefined { chart: String, dimension: String },
    /// Chart was already defined.
    ChartAlreadyDefined(String),
    /// No active slot (begin_slot not called).
    NoActiveSlot,
}

impl std::fmt::Display for ProducerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProducerError::Transport(e) => write!(f, "transport error: {e}"),
            ProducerError::Status(s) => write!(f, "gRPC status: {s}"),
            ProducerError::InvalidUri(uri) => write!(f, "invalid URI: {uri}"),
            ProducerError::ChartNotDefined(name) => write!(f, "chart not defined: {name}"),
            ProducerError::DimensionNotDefined { chart, dimension } => {
                write!(f, "dimension not defined: {chart}.{dimension}")
            }
            ProducerError::ChartAlreadyDefined(name) => {
                write!(f, "chart already defined: {name}")
            }
            ProducerError::NoActiveSlot => write!(f, "no active slot (call begin_slot first)"),
        }
    }
}

impl std::error::Error for ProducerError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ProducerError::Transport(e) => Some(e),
            ProducerError::Status(s) => Some(s),
            ProducerError::InvalidUri(_)
            | ProducerError::ChartNotDefined(_)
            | ProducerError::DimensionNotDefined { .. }
            | ProducerError::ChartAlreadyDefined(_)
            | ProducerError::NoActiveSlot => None,
        }
    }
}

impl From<tonic::transport::Error> for ProducerError {
    fn from(e: tonic::transport::Error) -> Self {
        ProducerError::Transport(e)
    }
}

impl From<tonic::Status> for ProducerError {
    fn from(s: tonic::Status) -> Self {
        ProducerError::Status(s)
    }
}

/// Trait for sending slot logs to a consumer.
///
/// This abstraction allows for both gRPC and in-process communication.
#[tonic::async_trait]
pub trait SlotLogSender: Send + Sync {
    /// Send a slot log and receive the response.
    async fn send(&mut self, log: SlotLog) -> Result<SlotLogResponse, ProducerError>;
}

/// gRPC-based slot log sender.
pub struct GrpcSender {
    client: MetricsProcessorClient<Channel>,
}

/// Default max message size (4MB, same as tonic default).
const DEFAULT_MAX_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

impl GrpcSender {
    /// Connect to a consumer at the given address with default message size limits.
    pub async fn connect(addr: &str) -> Result<Self, ProducerError> {
        Self::connect_with_config(addr, DEFAULT_MAX_MESSAGE_SIZE).await
    }

    /// Connect to a consumer with custom max message size.
    pub async fn connect_with_config(
        addr: &str,
        max_message_size: usize,
    ) -> Result<Self, ProducerError> {
        let channel = Channel::from_shared(addr.to_string())
            .map_err(|_| ProducerError::InvalidUri(addr.to_string()))?
            .connect()
            .await?;

        let client = MetricsProcessorClient::new(channel)
            .max_decoding_message_size(max_message_size)
            .max_encoding_message_size(max_message_size);

        Ok(Self { client })
    }
}

#[tonic::async_trait]
impl SlotLogSender for GrpcSender {
    async fn send(&mut self, log: SlotLog) -> Result<SlotLogResponse, ProducerError> {
        let response = self.client.process_slot(log).await?;
        Ok(response.into_inner())
    }
}

/// Internal storage for a dimension.
struct DimensionSlot {
    name: String,
    /// Consumer-assigned index (None until first send).
    consumer_idx: Option<u32>,
}

/// Internal storage for a chart.
struct ChartSlot {
    name: String,
    chart_type: ChartType,
    /// Consumer-assigned ID (None until first send).
    consumer_id: Option<u32>,
    /// Dimensions indexed by internal index.
    dimensions: Vec<DimensionSlot>,
    /// Dimension name to internal index lookup.
    dimension_names: FxHashMap<String, usize>,
}

/// A pending late update entry.
struct LateEntry {
    slot_timestamp: u64,
    chart_idx: usize,
    dim_idx: usize,
    value: Option<f64>,
}

/// A deferred late entry (waiting for ID assignment).
struct DeferredLateEntry {
    slot_timestamp: u64,
    chart_idx: usize,
    dim_idx: usize,
    value: Option<f64>,
}

/// A pending deletion entry.
struct DeletionEntry {
    chart_idx: usize,
    /// None = delete entire chart, Some = delete specific dimensions.
    dim_indices: Option<Vec<usize>>,
}

/// The main producer that builds and sends slot logs.
pub struct Producer<S: SlotLogSender> {
    /// The sender for communicating with the consumer.
    sender: S,

    /// Charts indexed by internal index.
    charts: Vec<ChartSlot>,
    /// Chart name to internal index lookup.
    chart_names: FxHashMap<String, usize>,

    /// Current slot timestamp (None if begin_slot not called).
    current_slot: Option<u64>,
    /// Pending values for current slot: (chart_idx, dim_idx) -> value.
    pending_values: FxHashMap<(usize, usize), Option<f64>>,
    /// Pending late updates.
    pending_late: Vec<LateEntry>,
    /// Pending deletions.
    pending_deletions: Vec<DeletionEntry>,

    /// Deferred late data waiting for IDs to be assigned.
    deferred_late: Vec<DeferredLateEntry>,
}

impl<S: SlotLogSender> Producer<S> {
    /// Create a new producer with the given sender.
    pub fn new(sender: S) -> Self {
        Self {
            sender,
            charts: Vec::new(),
            chart_names: FxHashMap::default(),
            current_slot: None,
            pending_values: FxHashMap::default(),
            pending_late: Vec::new(),
            pending_deletions: Vec::new(),
            deferred_late: Vec::new(),
        }
    }

    /// Define a new chart.
    ///
    /// Must be called before updating any dimensions on this chart.
    /// Returns error if chart is already defined.
    pub fn define_chart(&mut self, name: &str, chart_type: ChartType) -> Result<(), ProducerError> {
        if self.chart_names.contains_key(name) {
            return Err(ProducerError::ChartAlreadyDefined(name.to_string()));
        }

        let idx = self.charts.len();
        self.charts.push(ChartSlot {
            name: name.to_string(),
            chart_type,
            consumer_id: None,
            dimensions: Vec::new(),
            dimension_names: FxHashMap::default(),
        });
        self.chart_names.insert(name.to_string(), idx);
        Ok(())
    }

    /// Define a dimension on an existing chart.
    ///
    /// Must be called before updating this dimension.
    /// No-op if dimension already exists.
    pub fn define_dimension(&mut self, chart: &str, dimension: &str) -> Result<(), ProducerError> {
        let chart_idx = self
            .chart_names
            .get(chart)
            .copied()
            .ok_or_else(|| ProducerError::ChartNotDefined(chart.to_string()))?;

        let chart_slot = &mut self.charts[chart_idx];

        // No-op if already defined
        if chart_slot.dimension_names.contains_key(dimension) {
            return Ok(());
        }

        let dim_idx = chart_slot.dimensions.len();
        chart_slot.dimensions.push(DimensionSlot {
            name: dimension.to_string(),
            consumer_idx: None,
        });
        chart_slot
            .dimension_names
            .insert(dimension.to_string(), dim_idx);
        Ok(())
    }

    /// Begin accumulating updates for a new slot.
    ///
    /// This clears any pending values from the previous slot.
    pub fn begin_slot(&mut self, timestamp: u64) {
        self.current_slot = Some(timestamp);
        self.pending_values.clear();
        // Note: pending_late and pending_deletions accumulate until send()
    }

    /// Update a dimension value for the current slot.
    ///
    /// Returns error if chart or dimension not defined, or if begin_slot not called.
    pub fn update(
        &mut self,
        chart: &str,
        dimension: &str,
        value: Option<f64>,
    ) -> Result<(), ProducerError> {
        if self.current_slot.is_none() {
            return Err(ProducerError::NoActiveSlot);
        }

        let (chart_idx, dim_idx) = self.resolve_dimension(chart, dimension)?;
        self.pending_values.insert((chart_idx, dim_idx), value);
        Ok(())
    }

    /// Update a dimension value for a past slot (late data).
    ///
    /// Returns error if chart or dimension not defined.
    pub fn update_late(
        &mut self,
        slot: u64,
        chart: &str,
        dimension: &str,
        value: Option<f64>,
    ) -> Result<(), ProducerError> {
        let (chart_idx, dim_idx) = self.resolve_dimension(chart, dimension)?;
        self.pending_late.push(LateEntry {
            slot_timestamp: slot,
            chart_idx,
            dim_idx,
            value,
        });
        Ok(())
    }

    /// Delete a chart.
    ///
    /// Returns error if chart not defined.
    pub fn delete_chart(&mut self, chart: &str) -> Result<(), ProducerError> {
        let chart_idx = self
            .chart_names
            .get(chart)
            .copied()
            .ok_or_else(|| ProducerError::ChartNotDefined(chart.to_string()))?;

        self.pending_deletions.push(DeletionEntry {
            chart_idx,
            dim_indices: None,
        });
        Ok(())
    }

    /// Delete a dimension from a chart.
    ///
    /// Returns error if chart or dimension not defined.
    pub fn delete_dimension(&mut self, chart: &str, dimension: &str) -> Result<(), ProducerError> {
        let (chart_idx, dim_idx) = self.resolve_dimension(chart, dimension)?;

        // Find existing deletion entry for this chart or create new one
        if let Some(entry) = self
            .pending_deletions
            .iter_mut()
            .find(|e| e.chart_idx == chart_idx && e.dim_indices.is_some())
        {
            entry.dim_indices.as_mut().unwrap().push(dim_idx);
        } else {
            self.pending_deletions.push(DeletionEntry {
                chart_idx,
                dim_indices: Some(vec![dim_idx]),
            });
        }
        Ok(())
    }

    /// Build and send the accumulated updates.
    ///
    /// This method:
    /// 1. Builds registrations for new charts/dimensions
    /// 2. Builds updates for known charts/dimensions
    /// 3. Handles late data (potentially deferring if IDs not yet assigned)
    /// 4. Sends to consumer and applies response
    pub async fn send(&mut self) -> Result<(), ProducerError> {
        let slot_timestamp = self.current_slot.unwrap_or(0);

        let mut registrations = Vec::new();
        let mut updates = Vec::new();
        let mut late_updates = Vec::new();
        let mut deletions = Vec::new();

        // Track which charts/dimensions need ID assignment
        let mut pending_chart_indices: Vec<usize> = Vec::new();
        let mut pending_dim_indices: Vec<(usize, Vec<usize>)> = Vec::new();

        // Group pending values by chart
        let mut values_by_chart: FxHashMap<usize, Vec<(usize, Option<f64>)>> = FxHashMap::default();
        for ((chart_idx, dim_idx), value) in self.pending_values.drain() {
            values_by_chart
                .entry(chart_idx)
                .or_default()
                .push((dim_idx, value));
        }

        // Process each chart with pending values
        for (chart_idx, dim_values) in values_by_chart {
            let chart = &self.charts[chart_idx];

            if chart.consumer_id.is_none() {
                // New chart - needs registration
                let dim_regs: Vec<DimensionRegistration> = dim_values
                    .iter()
                    .map(|(dim_idx, value)| {
                        let dim = &chart.dimensions[*dim_idx];
                        DimensionRegistration {
                            name: dim.name.clone(),
                            value: *value,
                        }
                    })
                    .collect();

                let new_dim_indices: Vec<usize> = dim_values.iter().map(|(idx, _)| *idx).collect();

                registrations.push(ChartRegistration {
                    name: chart.name.clone(),
                    chart_type: chart.chart_type.into(),
                    dimensions: dim_regs,
                });
                pending_chart_indices.push(chart_idx);
                pending_dim_indices.push((chart_idx, new_dim_indices));
            } else {
                // Existing chart - build update
                let chart_id = chart.consumer_id.unwrap();
                let mut new_dimensions = Vec::new();
                let mut values = Vec::new();
                let mut new_dim_indices_for_chart = Vec::new();

                for (dim_idx, value) in dim_values {
                    let dim = &chart.dimensions[dim_idx];
                    if let Some(consumer_idx) = dim.consumer_idx {
                        // Known dimension
                        values.push(DimensionValue {
                            dim_idx: consumer_idx,
                            value,
                        });
                    } else {
                        // New dimension
                        new_dimensions.push(DimensionRegistration {
                            name: dim.name.clone(),
                            value,
                        });
                        new_dim_indices_for_chart.push(dim_idx);
                    }
                }

                updates.push(ChartUpdate {
                    chart_id,
                    new_dimensions,
                    values,
                });

                if !new_dim_indices_for_chart.is_empty() {
                    pending_dim_indices.push((chart_idx, new_dim_indices_for_chart));
                }
            }
        }

        // Process deferred late data from previous slots
        let deferred = std::mem::take(&mut self.deferred_late);
        for entry in deferred {
            let chart = &self.charts[entry.chart_idx];
            let dim = &chart.dimensions[entry.dim_idx];

            if let (Some(chart_id), Some(dim_idx)) = (chart.consumer_id, dim.consumer_idx) {
                // IDs now available - send the late update
                late_updates.push(LateUpdate {
                    slot_timestamp: entry.slot_timestamp,
                    chart_id,
                    new_dimensions: Vec::new(),
                    values: vec![DimensionValue {
                        dim_idx,
                        value: entry.value,
                    }],
                });
            } else {
                // Still waiting for IDs - keep deferred
                self.deferred_late.push(entry);
            }
        }

        // Process new late data
        let mut late_dim_indices: Vec<(usize, Vec<usize>)> = Vec::new();
        for entry in std::mem::take(&mut self.pending_late) {
            let chart = &self.charts[entry.chart_idx];
            let dim = &chart.dimensions[entry.dim_idx];

            if chart.consumer_id.is_none() {
                // Chart not registered - register it now, defer the value
                let dim_regs = vec![DimensionRegistration {
                    name: dim.name.clone(),
                    value: None, // Don't include value for late data registration
                }];

                registrations.push(ChartRegistration {
                    name: chart.name.clone(),
                    chart_type: chart.chart_type.into(),
                    dimensions: dim_regs,
                });
                pending_chart_indices.push(entry.chart_idx);
                pending_dim_indices.push((entry.chart_idx, vec![entry.dim_idx]));

                // Defer the actual value
                self.deferred_late.push(DeferredLateEntry {
                    slot_timestamp: entry.slot_timestamp,
                    chart_idx: entry.chart_idx,
                    dim_idx: entry.dim_idx,
                    value: entry.value,
                });
            } else {
                let chart_id = chart.consumer_id.unwrap();

                if let Some(consumer_idx) = dim.consumer_idx {
                    // Both chart and dimension registered - send late update
                    late_updates.push(LateUpdate {
                        slot_timestamp: entry.slot_timestamp,
                        chart_id,
                        new_dimensions: Vec::new(),
                        values: vec![DimensionValue {
                            dim_idx: consumer_idx,
                            value: entry.value,
                        }],
                    });
                } else {
                    // Chart registered but dimension is new - register with value
                    late_updates.push(LateUpdate {
                        slot_timestamp: entry.slot_timestamp,
                        chart_id,
                        new_dimensions: vec![DimensionRegistration {
                            name: dim.name.clone(),
                            value: entry.value,
                        }],
                        values: Vec::new(),
                    });
                    late_dim_indices.push((entry.chart_idx, vec![entry.dim_idx]));
                }
            }
        }

        // Process deletions
        for entry in std::mem::take(&mut self.pending_deletions) {
            let chart = &self.charts[entry.chart_idx];
            if let Some(chart_id) = chart.consumer_id {
                let dimension_indices = match entry.dim_indices {
                    None => Vec::new(), // Delete entire chart
                    Some(indices) => indices
                        .iter()
                        .filter_map(|&idx| chart.dimensions[idx].consumer_idx)
                        .collect(),
                };

                deletions.push(Deletion {
                    chart_id,
                    dimension_indices,
                });
            }
            // If chart has no consumer_id yet, we haven't registered it, so nothing to delete
        }

        // Build and send the SlotLog
        let log = SlotLog {
            timestamp: slot_timestamp,
            registrations,
            updates,
            deletions,
            late_updates,
        };

        let response = self.sender.send(log).await?;

        // Apply chart ID assignments
        for (assignment, &chart_idx) in response
            .chart_assignments
            .iter()
            .zip(pending_chart_indices.iter())
        {
            self.charts[chart_idx].consumer_id = Some(assignment.chart_id);

            // Apply dimension index assignments for this registration
            if let Some((_, dim_indices)) = pending_dim_indices
                .iter()
                .find(|(cidx, _)| *cidx == chart_idx)
            {
                for (local_dim_idx, &consumer_idx) in
                    dim_indices.iter().zip(assignment.dimension_indices.iter())
                {
                    self.charts[chart_idx].dimensions[*local_dim_idx].consumer_idx =
                        Some(consumer_idx);
                }
            }
        }

        // Apply dimension assignments from updates
        let update_dim_indices: Vec<_> = pending_dim_indices
            .iter()
            .filter(|(chart_idx, _)| self.charts[*chart_idx].consumer_id.is_some())
            .filter(|(chart_idx, _)| !pending_chart_indices.contains(chart_idx))
            .collect();

        for (dim_assignment, (chart_idx, dim_indices)) in response
            .dimension_assignments
            .iter()
            .zip(update_dim_indices.iter())
        {
            for (local_dim_idx, &consumer_idx) in
                dim_indices.iter().zip(dim_assignment.indices.iter())
            {
                self.charts[*chart_idx].dimensions[*local_dim_idx].consumer_idx =
                    Some(consumer_idx);
            }
        }

        // Apply dimension assignments from late updates
        for (dim_assignment, (chart_idx, dim_indices)) in response
            .late_dimension_assignments
            .iter()
            .zip(late_dim_indices.iter())
        {
            for (local_dim_idx, &consumer_idx) in
                dim_indices.iter().zip(dim_assignment.indices.iter())
            {
                self.charts[*chart_idx].dimensions[*local_dim_idx].consumer_idx =
                    Some(consumer_idx);
            }
        }

        // Apply remappings
        if let Some(remapping) = response.remapping {
            for chart_remap in &remapping.charts {
                for chart in &mut self.charts {
                    if chart.consumer_id == Some(chart_remap.old_id) {
                        chart.consumer_id = Some(chart_remap.new_id);
                        break;
                    }
                }
            }

            for dim_remap in &remapping.dimensions {
                for chart in &mut self.charts {
                    if chart.consumer_id == Some(dim_remap.chart_id) {
                        for mapping in &dim_remap.mappings {
                            for dim in &mut chart.dimensions {
                                if dim.consumer_idx == Some(mapping.old_idx) {
                                    dim.consumer_idx = Some(mapping.new_idx);
                                    break;
                                }
                            }
                        }
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Resolve a chart and dimension name to internal indices.
    fn resolve_dimension(
        &self,
        chart: &str,
        dimension: &str,
    ) -> Result<(usize, usize), ProducerError> {
        let chart_idx = self
            .chart_names
            .get(chart)
            .copied()
            .ok_or_else(|| ProducerError::ChartNotDefined(chart.to_string()))?;

        let chart_slot = &self.charts[chart_idx];
        let dim_idx = chart_slot
            .dimension_names
            .get(dimension)
            .copied()
            .ok_or_else(|| ProducerError::DimensionNotDefined {
                chart: chart.to_string(),
                dimension: dimension.to_string(),
            })?;

        Ok((chart_idx, dim_idx))
    }
}

/// A simple in-memory sender for testing.
pub struct InMemorySender<F>
where
    F: FnMut(SlotLog) -> SlotLogResponse + Send + Sync,
{
    handler: F,
}

impl<F> InMemorySender<F>
where
    F: FnMut(SlotLog) -> SlotLogResponse + Send + Sync,
{
    /// Create a new in-memory sender with the given handler function.
    pub fn new(handler: F) -> Self {
        Self { handler }
    }
}

#[tonic::async_trait]
impl<F> SlotLogSender for InMemorySender<F>
where
    F: FnMut(SlotLog) -> SlotLogResponse + Send + Sync,
{
    async fn send(&mut self, log: SlotLog) -> Result<SlotLogResponse, ProducerError> {
        Ok((self.handler)(log))
    }
}

#[cfg(test)]
mod tests {
    use slotlog::{ChartAssignment, DimensionAssignments};

    use super::*;

    fn create_mock_sender() -> InMemorySender<impl FnMut(SlotLog) -> SlotLogResponse + Send + Sync>
    {
        let mut next_chart_id = 0u32;
        let mut dim_counters: std::collections::HashMap<u32, u32> =
            std::collections::HashMap::new();

        InMemorySender::new(move |log: SlotLog| {
            let chart_assignments: Vec<ChartAssignment> = log
                .registrations
                .iter()
                .map(|reg| {
                    let id = next_chart_id;
                    next_chart_id += 1;

                    let dim_count = reg.dimensions.len() as u32;
                    let indices: Vec<u32> = (0..dim_count).collect();
                    dim_counters.insert(id, dim_count);

                    ChartAssignment {
                        chart_id: id,
                        dimension_indices: indices,
                    }
                })
                .collect();

            let dimension_assignments: Vec<DimensionAssignments> = log
                .updates
                .iter()
                .map(|update| {
                    let counter = dim_counters.entry(update.chart_id).or_insert(0);
                    let start_idx = *counter;
                    let count = update.new_dimensions.len() as u32;
                    *counter += count;

                    DimensionAssignments {
                        indices: (start_idx..start_idx + count).collect(),
                    }
                })
                .collect();

            let late_dimension_assignments: Vec<DimensionAssignments> = log
                .late_updates
                .iter()
                .map(|update| {
                    let counter = dim_counters.entry(update.chart_id).or_insert(0);
                    let start_idx = *counter;
                    let count = update.new_dimensions.len() as u32;
                    *counter += count;

                    DimensionAssignments {
                        indices: (start_idx..start_idx + count).collect(),
                    }
                })
                .collect();

            SlotLogResponse {
                chart_assignments,
                dimension_assignments,
                late_dimension_assignments,
                remapping: None,
            }
        })
    }

    #[tokio::test]
    async fn test_define_and_update() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        // Define chart and dimensions
        producer
            .define_chart("cpu.usage", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("cpu.usage", "user").unwrap();
        producer.define_dimension("cpu.usage", "system").unwrap();

        // First slot - should register
        producer.begin_slot(1000);
        producer.update("cpu.usage", "user", Some(25.5)).unwrap();
        producer.update("cpu.usage", "system", Some(10.2)).unwrap();
        producer.send().await.unwrap();

        // Verify IDs were assigned
        assert_eq!(producer.charts[0].consumer_id, Some(0));
        assert_eq!(producer.charts[0].dimensions[0].consumer_idx, Some(0));
        assert_eq!(producer.charts[0].dimensions[1].consumer_idx, Some(1));

        // Second slot - should be update only
        producer.begin_slot(1001);
        producer.update("cpu.usage", "user", Some(30.1)).unwrap();
        producer.update("cpu.usage", "system", Some(12.4)).unwrap();
        producer.send().await.unwrap();

        // IDs should remain the same
        assert_eq!(producer.charts[0].consumer_id, Some(0));
    }

    #[tokio::test]
    async fn test_error_on_undefined_chart() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        producer.begin_slot(1000);
        let result = producer.update("unknown.chart", "dim", Some(1.0));

        assert!(matches!(result, Err(ProducerError::ChartNotDefined(_))));
    }

    #[tokio::test]
    async fn test_error_on_undefined_dimension() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        producer
            .define_chart("test.chart", ChartType::Gauge)
            .unwrap();

        producer.begin_slot(1000);
        let result = producer.update("test.chart", "unknown.dim", Some(1.0));

        assert!(matches!(
            result,
            Err(ProducerError::DimensionNotDefined { .. })
        ));
    }

    #[tokio::test]
    async fn test_error_on_no_active_slot() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        producer
            .define_chart("test.chart", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("test.chart", "dim").unwrap();

        // Don't call begin_slot
        let result = producer.update("test.chart", "dim", Some(1.0));

        assert!(matches!(result, Err(ProducerError::NoActiveSlot)));
    }

    #[tokio::test]
    async fn test_error_on_duplicate_chart() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        producer
            .define_chart("test.chart", ChartType::Gauge)
            .unwrap();
        let result = producer.define_chart("test.chart", ChartType::Gauge);

        assert!(matches!(result, Err(ProducerError::ChartAlreadyDefined(_))));
    }

    #[tokio::test]
    async fn test_new_dimension_on_registered_chart() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        // Define and register chart with one dimension
        producer
            .define_chart("test.chart", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("test.chart", "dim1").unwrap();

        producer.begin_slot(1000);
        producer.update("test.chart", "dim1", Some(1.0)).unwrap();
        producer.send().await.unwrap();

        // Add a new dimension
        producer.define_dimension("test.chart", "dim2").unwrap();

        producer.begin_slot(1001);
        producer.update("test.chart", "dim1", Some(2.0)).unwrap();
        producer.update("test.chart", "dim2", Some(3.0)).unwrap();
        producer.send().await.unwrap();

        // New dimension should have been assigned an ID
        assert_eq!(producer.charts[0].dimensions[1].consumer_idx, Some(1));
    }

    #[tokio::test]
    async fn test_late_data_registered_chart() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        // Define and register chart
        producer
            .define_chart("test.chart", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("test.chart", "dim1").unwrap();

        producer.begin_slot(1000);
        producer.update("test.chart", "dim1", Some(1.0)).unwrap();
        producer.send().await.unwrap();

        // Send late data for a past slot
        producer.begin_slot(1001);
        producer
            .update_late(999, "test.chart", "dim1", Some(42.0))
            .unwrap();
        producer.send().await.unwrap();

        // Should work without errors
    }

    #[tokio::test]
    async fn test_late_data_unregistered_chart() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        // Define chart but don't register it yet (no update sent)
        producer
            .define_chart("late.chart", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("late.chart", "dim1").unwrap();

        // Send late data before any regular updates
        producer.begin_slot(1000);
        producer
            .update_late(999, "late.chart", "dim1", Some(42.0))
            .unwrap();
        producer.send().await.unwrap();

        // Chart should be registered
        assert_eq!(producer.charts[0].consumer_id, Some(0));

        // Value should be deferred
        assert_eq!(producer.deferred_late.len(), 1);

        // Next slot should send the deferred value
        producer.begin_slot(1001);
        producer.send().await.unwrap();

        assert_eq!(producer.deferred_late.len(), 0);
    }

    #[tokio::test]
    async fn test_multiple_charts() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        producer
            .define_chart("cpu.usage", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("cpu.usage", "user").unwrap();

        producer
            .define_chart("memory.usage", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("memory.usage", "used").unwrap();

        producer.begin_slot(1000);
        producer.update("cpu.usage", "user", Some(25.5)).unwrap();
        producer
            .update("memory.usage", "used", Some(1024.0))
            .unwrap();
        producer.send().await.unwrap();

        assert_eq!(producer.charts[0].consumer_id, Some(0));
        assert_eq!(producer.charts[1].consumer_id, Some(1));
    }

    #[tokio::test]
    async fn test_dimension_redefine_is_noop() {
        let sender = create_mock_sender();
        let mut producer = Producer::new(sender);

        producer
            .define_chart("test.chart", ChartType::Gauge)
            .unwrap();
        producer.define_dimension("test.chart", "dim1").unwrap();
        producer.define_dimension("test.chart", "dim1").unwrap(); // Should be no-op

        assert_eq!(producer.charts[0].dimensions.len(), 1);
    }
}
