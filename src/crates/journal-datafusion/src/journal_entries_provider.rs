use std::any::Any;
use std::sync::Arc;
use std::collections::HashMap;
use std::str;

use arrow::array::{StringArray, UInt64Array, ArrayRef, TimestampMicrosecondArray, Int32Array};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;

use datafusion_catalog::{Session, TableProvider};
use datafusion_common::{Result as DataFusionResult, DataFusionError};
use datafusion_expr::{Expr, TableProviderFilterPushDown, TableType};
use datafusion_common::{Column, ScalarValue};
use datafusion_physical_plan::ExecutionPlan;

use journal_registry::{JournalRegistry, RegistryFile};
use journal_file::{JournalFile, Mmap};

/// A DataFusion TableProvider that enables SQL queries over actual journal entries
pub struct JournalEntriesProvider {
    registry: Arc<JournalRegistry>,
    schema: SchemaRef,
}

impl std::fmt::Debug for JournalEntriesProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalEntriesProvider")
            .field("schema", &self.schema)
            .finish()
    }
}

impl JournalEntriesProvider {
    /// Create a new JournalEntriesProvider
    pub async fn new(journal_dirs: Vec<String>) -> anyhow::Result<Self> {
        let registry = Arc::new(JournalRegistry::new()?);
        
        // Add all specified directories
        for dir in journal_dirs {
            if let Err(e) = registry.add_directory(&dir) {
                tracing::warn!("Failed to add journal directory {}: {}", dir, e);
            } else {
                tracing::info!("Added journal directory: {}", dir);
            }
        }
        
        // Define schema for journal entries - common systemd journal fields
        let schema = Arc::new(Schema::new(vec![
            // Core journal fields
            Field::new("timestamp", DataType::Timestamp(TimeUnit::Microsecond, None), false),
            Field::new("monotonic", DataType::UInt64, true),
            Field::new("boot_id", DataType::Utf8, false),
            Field::new("seqnum", DataType::UInt64, false),
            
            // Standard message fields
            Field::new("message", DataType::Utf8, true),
            Field::new("priority", DataType::Int32, true),
            
            // Process information
            Field::new("pid", DataType::UInt64, true),
            Field::new("uid", DataType::UInt64, true),
            Field::new("gid", DataType::UInt64, true),
            Field::new("comm", DataType::Utf8, true),
            Field::new("exe", DataType::Utf8, true),
            Field::new("cmdline", DataType::Utf8, true),
            
            // Systemd unit information
            Field::new("systemd_unit", DataType::Utf8, true),
            Field::new("systemd_user_unit", DataType::Utf8, true),
            Field::new("systemd_slice", DataType::Utf8, true),
            
            // System information
            Field::new("hostname", DataType::Utf8, true),
            Field::new("machine_id", DataType::Utf8, true),
            
            // Source file information
            Field::new("source_file", DataType::Utf8, false),
            Field::new("source_file_size", DataType::UInt64, false),
        ]));
        
        Ok(Self { registry, schema })
    }

    /// Check if a filter can be pushed down to the journal reader level
    fn can_push_filter(&self, expr: &Expr) -> TableProviderFilterPushDown {
        use datafusion_expr::{BinaryExpr, Operator};
        
        match expr {
            // Time range filters - very important for performance
            Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
                if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                    match (col.name.as_str(), op) {
                        ("timestamp", Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq) => {
                            return TableProviderFilterPushDown::Exact;
                        }
                        ("seqnum", Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq) => {
                            return TableProviderFilterPushDown::Exact;
                        }
                        // String filters on systemd unit, etc.
                        ("systemd_unit" | "comm" | "hostname", Operator::Eq) => {
                            return TableProviderFilterPushDown::Inexact; // We can filter but may not be 100% accurate
                        }
                        ("priority", Operator::Eq | Operator::Lt | Operator::LtEq) => {
                            return TableProviderFilterPushDown::Exact;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        
        TableProviderFilterPushDown::Unsupported
    }
}

#[async_trait]
impl TableProvider for JournalEntriesProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    fn supports_filters_pushdown(&self, filters: &[&Expr]) -> DataFusionResult<Vec<TableProviderFilterPushDown>> {
        Ok(filters.iter().map(|expr| self.can_push_filter(expr)).collect())
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DataFusionResult<Arc<dyn ExecutionPlan>> {
        // Start with all files from registry
        let mut query = self.registry.query();
        
        // Apply basic file-level filters to registry query
        for filter in filters {
            query = apply_file_level_filter(query, filter).unwrap_or(query);
        }
        
        // Execute the query to get filtered registry files
        let registry_files = query.execute();
        
        // Create execution plan that will read actual journal entries
        Ok(Arc::new(JournalEntriesExecutionPlan::new(
            registry_files,
            self.schema(),
            projection.cloned(),
            filters.to_vec(),
            limit,
        )))
    }
}

/// Apply file-level filters to narrow down which journal files to read
fn apply_file_level_filter(
    query: journal_registry::RegistryQuery,
    filter: &Expr
) -> Option<journal_registry::RegistryQuery> {
    use datafusion_expr::{BinaryExpr, Operator};
    
    match filter {
        Expr::BinaryExpr(BinaryExpr { left, op: Operator::GtEq | Operator::Gt, right }) => {
            if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                if col.name == "timestamp" {
                    // Convert timestamp filter to file modification time filter
                    // This is a rough approximation but can help eliminate obviously irrelevant files
                    if let Some(timestamp_micros) = get_timestamp_from_literal(lit) {
                        let system_time = timestamp_to_system_time(timestamp_micros);
                        return Some(query.modified_after(system_time));
                    }
                }
            }
        }
        _ => {}
    }
    None
}

fn get_timestamp_from_literal(lit: &ScalarValue) -> Option<i64> {
    match lit {
        ScalarValue::TimestampMicrosecond(Some(ts), _) => Some(*ts),
        ScalarValue::TimestampMillisecond(Some(ts), _) => Some(*ts * 1000),
        ScalarValue::TimestampSecond(Some(ts), _) => Some(*ts * 1_000_000),
        _ => None,
    }
}

fn timestamp_to_system_time(timestamp_micros: i64) -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_micros(timestamp_micros as u64)
}

/// Custom ExecutionPlan for journal entries
pub struct JournalEntriesExecutionPlan {
    files: Vec<RegistryFile>,
    schema: SchemaRef,
    projection: Option<Vec<usize>>,
    filters: Vec<Expr>,
    limit: Option<usize>,
    properties: datafusion_physical_plan::PlanProperties,
}

impl JournalEntriesExecutionPlan {
    pub fn new(
        files: Vec<RegistryFile>,
        schema: SchemaRef,
        projection: Option<Vec<usize>>,
        filters: Vec<Expr>,
        limit: Option<usize>,
    ) -> Self {
        use datafusion_physical_expr::EquivalenceProperties;
        use datafusion_physical_plan::{Partitioning, execution_plan::{Boundedness, EmissionType}};
        
        let eq_properties = EquivalenceProperties::new(schema.clone());
        let properties = datafusion_physical_plan::PlanProperties::new(
            eq_properties,
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        );
        
        Self {
            files,
            schema,
            projection,
            filters,
            limit,
            properties,
        }
    }

    /// Parse a journal file and extract entries matching the filters
    fn parse_journal_entries(&self) -> DataFusionResult<RecordBatch> {
        let mut all_entries = Vec::new();
        let mut entries_count = 0;
        
        for file in &self.files {
            if let Some(limit) = self.limit {
                if entries_count >= limit {
                    break;
                }
            }
            
            match self.parse_single_file(file) {
                Ok(mut file_entries) => {
                    // Apply limit across all files
                    if let Some(limit) = self.limit {
                        let remaining = limit.saturating_sub(entries_count);
                        if file_entries.len() > remaining {
                            file_entries.truncate(remaining);
                        }
                    }
                    
                    entries_count += file_entries.len();
                    all_entries.extend(file_entries);
                }
                Err(e) => {
                    tracing::warn!("Failed to parse journal file {:?}: {}", file.path, e);
                    continue;
                }
            }
        }
        
        self.build_record_batch(all_entries)
    }
    
    /// Parse a single journal file
    fn parse_single_file(&self, file: &RegistryFile) -> anyhow::Result<Vec<JournalEntryData>> {
        // Use the memory mapping implementation from the journal_file crate
        let journal_file = JournalFile::<Mmap>::open(&file.path, 64 * 1024)?;
        let mut entries = Vec::new();
        
        // Iterate through all entries in the file
        let entry_offsets = journal_file.entry_offsets()?;
        
        for entry_offset_result in entry_offsets {
            let entry_offset = entry_offset_result?;
            
            // Get the entry
            let entry_guard = journal_file.entry_ref(entry_offset)?;
            
            // Parse entry data
            let mut entry_data = JournalEntryData {
                timestamp: entry_guard.header.realtime,
                monotonic: Some(entry_guard.header.monotonic),
                boot_id: hex::encode(&entry_guard.header.boot_id),
                seqnum: entry_guard.header.seqnum,
                
                message: None,
                priority: None,
                pid: None,
                uid: None,
                gid: None,
                comm: None,
                exe: None,
                cmdline: None,
                systemd_unit: None,
                systemd_user_unit: None,
                systemd_slice: None,
                hostname: None,
                machine_id: None,
                
                source_file: file.path.to_string_lossy().to_string(),
                source_file_size: file.size,
            };
            
            // Parse data objects (field-value pairs) for this entry
            let entry_data_iterator = journal_file.entry_data_objects(entry_offset)?;
            for data_result in entry_data_iterator {
                let data_guard = data_result?;
                
                // Get the field name from the field object
                // Note: This is a simplified version - we'd need to implement field lookup
                // For now, we'll extract common fields from the data payload
                let payload = data_guard.payload_bytes();
                
                if let Ok(field_value_str) = str::from_utf8(payload) {
                    if let Some((field, value)) = field_value_str.split_once('=') {
                        match field {
                            "MESSAGE" => entry_data.message = Some(value.to_string()),
                            "PRIORITY" => entry_data.priority = value.parse().ok(),
                            "_PID" => entry_data.pid = value.parse().ok(),
                            "_UID" => entry_data.uid = value.parse().ok(),
                            "_GID" => entry_data.gid = value.parse().ok(),
                            "_COMM" => entry_data.comm = Some(value.to_string()),
                            "_EXE" => entry_data.exe = Some(value.to_string()),
                            "_CMDLINE" => entry_data.cmdline = Some(value.to_string()),
                            "_SYSTEMD_UNIT" => entry_data.systemd_unit = Some(value.to_string()),
                            "_SYSTEMD_USER_UNIT" => entry_data.systemd_user_unit = Some(value.to_string()),
                            "_SYSTEMD_SLICE" => entry_data.systemd_slice = Some(value.to_string()),
                            "_HOSTNAME" => entry_data.hostname = Some(value.to_string()),
                            "_MACHINE_ID" => entry_data.machine_id = Some(value.to_string()),
                            _ => {} // Ignore other fields for now
                        }
                    }
                }
            }
            
            // Apply entry-level filters
            if self.matches_filters(&entry_data) {
                entries.push(entry_data);
            }
        }
        
        Ok(entries)
    }
    
    /// Check if an entry matches the pushed down filters
    fn matches_filters(&self, entry: &JournalEntryData) -> bool {
        for filter in &self.filters {
            if !self.entry_matches_filter(entry, filter) {
                return false;
            }
        }
        true
    }
    
    /// Check if a single entry matches a filter
    fn entry_matches_filter(&self, entry: &JournalEntryData, filter: &Expr) -> bool {
        use datafusion_expr::{BinaryExpr, Operator};
        
        match filter {
            Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
                if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                    match col.name.as_str() {
                        "timestamp" => {
                            if let Some(filter_ts) = get_timestamp_from_literal(lit) {
                                let entry_ts = entry.timestamp as i64;
                                match op {
                                    Operator::Gt => entry_ts > filter_ts,
                                    Operator::GtEq => entry_ts >= filter_ts,
                                    Operator::Lt => entry_ts < filter_ts,
                                    Operator::LtEq => entry_ts <= filter_ts,
                                    Operator::Eq => entry_ts == filter_ts,
                                    _ => true,
                                }
                            } else {
                                true
                            }
                        }
                        "systemd_unit" => {
                            if let (Some(ScalarValue::Utf8(Some(filter_val))), Some(unit)) = (right.as_literal(), &entry.systemd_unit) {
                                match op {
                                    Operator::Eq => unit == filter_val,
                                    _ => true,
                                }
                            } else {
                                true
                            }
                        }
                        "priority" => {
                            if let (Some(filter_priority), Some(entry_priority)) = (get_int_from_literal(lit), entry.priority) {
                                match op {
                                    Operator::Eq => entry_priority == filter_priority,
                                    Operator::Lt => entry_priority < filter_priority,
                                    Operator::LtEq => entry_priority <= filter_priority,
                                    _ => true,
                                }
                            } else {
                                true
                            }
                        }
                        _ => true, // Unknown filter - don't filter out
                    }
                } else {
                    true
                }
            }
            _ => true, // Unknown filter type - don't filter out
        }
    }
    
    /// Build Arrow RecordBatch from journal entries
    fn build_record_batch(&self, entries: Vec<JournalEntryData>) -> DataFusionResult<RecordBatch> {
        let num_entries = entries.len();
        
        // Build arrays for each column
        let timestamps: ArrayRef = Arc::new(TimestampMicrosecondArray::from(
            entries.iter().map(|e| e.timestamp as i64).collect::<Vec<_>>()
        ));
        
        let monotonic: ArrayRef = Arc::new(UInt64Array::from(
            entries.iter().map(|e| e.monotonic).collect::<Vec<_>>()
        ));
        
        let boot_ids: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.boot_id.clone()).collect::<Vec<_>>()
        ));
        
        let seqnums: ArrayRef = Arc::new(UInt64Array::from(
            entries.iter().map(|e| e.seqnum).collect::<Vec<_>>()
        ));
        
        let messages: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
        ));
        
        let priorities: ArrayRef = Arc::new(Int32Array::from(
            entries.iter().map(|e| e.priority).collect::<Vec<_>>()
        ));
        
        let pids: ArrayRef = Arc::new(UInt64Array::from(
            entries.iter().map(|e| e.pid).collect::<Vec<_>>()
        ));
        
        let uids: ArrayRef = Arc::new(UInt64Array::from(
            entries.iter().map(|e| e.uid).collect::<Vec<_>>()
        ));
        
        let gids: ArrayRef = Arc::new(UInt64Array::from(
            entries.iter().map(|e| e.gid).collect::<Vec<_>>()
        ));
        
        let comms: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.comm.clone()).collect::<Vec<_>>()
        ));
        
        let exes: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.exe.clone()).collect::<Vec<_>>()
        ));
        
        let cmdlines: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.cmdline.clone()).collect::<Vec<_>>()
        ));
        
        let systemd_units: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.systemd_unit.clone()).collect::<Vec<_>>()
        ));
        
        let systemd_user_units: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.systemd_user_unit.clone()).collect::<Vec<_>>()
        ));
        
        let systemd_slices: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.systemd_slice.clone()).collect::<Vec<_>>()
        ));
        
        let hostnames: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.hostname.clone()).collect::<Vec<_>>()
        ));
        
        let machine_ids: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.machine_id.clone()).collect::<Vec<_>>()
        ));
        
        let source_files: ArrayRef = Arc::new(StringArray::from(
            entries.iter().map(|e| e.source_file.clone()).collect::<Vec<_>>()
        ));
        
        let source_file_sizes: ArrayRef = Arc::new(UInt64Array::from(
            entries.iter().map(|e| e.source_file_size).collect::<Vec<_>>()
        ));
        
        let all_columns = vec![
            timestamps, monotonic, boot_ids, seqnums,
            messages, priorities, pids, uids, gids, comms, exes, cmdlines,
            systemd_units, systemd_user_units, systemd_slices,
            hostnames, machine_ids, source_files, source_file_sizes,
        ];
        
        // Apply projection if specified
        let columns = if let Some(projection) = &self.projection {
            projection.iter().map(|&i| all_columns[i].clone()).collect()
        } else {
            all_columns
        };
        
        let projected_schema = if let Some(projection) = &self.projection {
            let projected_fields: Vec<_> = projection.iter()
                .map(|&i| self.schema.field(i).clone())
                .collect();
            Arc::new(Schema::new(projected_fields))
        } else {
            self.schema.clone()
        };
        
        RecordBatch::try_new(projected_schema, columns)
            .map_err(|e| DataFusionError::ArrowError(Box::new(e), None))
    }
}

fn get_int_from_literal(lit: &ScalarValue) -> Option<i32> {
    match lit {
        ScalarValue::Int32(Some(val)) => Some(*val),
        ScalarValue::Int64(Some(val)) => Some(*val as i32),
        _ => None,
    }
}

/// Represents a parsed journal entry
#[derive(Debug, Clone)]
struct JournalEntryData {
    // Core fields
    timestamp: u64,
    monotonic: Option<u64>,
    boot_id: String,
    seqnum: u64,
    
    // Message fields
    message: Option<String>,
    priority: Option<i32>,
    
    // Process fields
    pid: Option<u64>,
    uid: Option<u64>,
    gid: Option<u64>,
    comm: Option<String>,
    exe: Option<String>,
    cmdline: Option<String>,
    
    // Systemd fields
    systemd_unit: Option<String>,
    systemd_user_unit: Option<String>,
    systemd_slice: Option<String>,
    
    // System fields
    hostname: Option<String>,
    machine_id: Option<String>,
    
    // Source information
    source_file: String,
    source_file_size: u64,
}

// ExecutionPlan implementation
use datafusion_physical_plan::{
    DisplayAs, DisplayFormatType, 
    PlanProperties,
    Partitioning,
    SendableRecordBatchStream,
    execution_plan::{Boundedness, EmissionType},
    memory::MemoryStream
};
use datafusion_physical_expr::EquivalenceProperties;
use datafusion_common::Result;
use datafusion_execution::TaskContext;
use std::fmt;

impl std::fmt::Debug for JournalEntriesExecutionPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JournalEntriesExecutionPlan: {} files", self.files.len())
    }
}

impl DisplayAs for JournalEntriesExecutionPlan {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "JournalEntriesScan: {} files", self.files.len())
    }
}

impl ExecutionPlan for JournalEntriesExecutionPlan {
    fn name(&self) -> &'static str {
        "JournalEntriesScan"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn properties(&self) -> &PlanProperties {
        &self.properties
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        Ok(self)
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let batch = self.parse_journal_entries()?;
        
        Ok(Box::pin(MemoryStream::try_new(
            vec![batch],
            self.schema(),
            None,
        )?))
    }
}