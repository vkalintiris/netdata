//! DataFusion TableProvider implementation for systemd journal logs

use anyhow::Result;
use async_trait::async_trait;
use datafusion::arrow::array::{
    ArrayRef, RecordBatch, StringBuilder, TimestampMicrosecondBuilder, UInt32Builder,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use datafusion::catalog::Session;
use datafusion::datasource::TableProvider;
use datafusion::error::{DataFusionError, Result as DataFusionResult};
use datafusion::execution::TaskContext;
use datafusion::logical_expr::TableType;
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use futures::stream::{self, StreamExt};
use journal_index::Direction;
use journal_function::indexing::{FileIndexRequest, IndexingService};
use journal_function::logs::{LogEntryData, LogQuery};
use journal_function::{Facets, FileIndexCache, FileIndexKey, Registry};
use std::any::Any;
use std::fmt;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// TableProvider that exposes systemd journal logs as a DataFusion table
pub struct JournalTableProvider {
    registry: Registry,
    file_index_cache: FileIndexCache,
    indexing_service: IndexingService,
    after: u32,
    before: u32,
    facets: Vec<String>,
    schema: SchemaRef,
}

impl fmt::Debug for JournalTableProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalTableProvider")
            .field("after", &self.after)
            .field("before", &self.before)
            .field("facets", &self.facets)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

// IndexingService doesn't have a shutdown method, workers run indefinitely
// This is fine for our use case - they'll be cleaned up when the process exits

impl JournalTableProvider {
    /// Create a new journal table provider
    pub fn new(
        registry: Registry,
        file_index_cache: FileIndexCache,
        indexing_service: IndexingService,
        after: u32,
        before: u32,
        facets: Vec<String>,
    ) -> Self {
        // Define the schema for the journal table
        // We'll expose a fixed set of common fields, plus dynamic fields
        let schema = Arc::new(Schema::new(vec![
            Field::new(
                "timestamp",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
            Field::new("priority", DataType::UInt32, true),
            Field::new("message", DataType::Utf8, true),
            Field::new("syslog_identifier", DataType::Utf8, true),
            Field::new("systemd_unit", DataType::Utf8, true),
            Field::new("hostname", DataType::Utf8, true),
            Field::new("pid", DataType::Utf8, true),
            Field::new("uid", DataType::Utf8, true),
            Field::new("gid", DataType::Utf8, true),
            Field::new("comm", DataType::Utf8, true),
            Field::new("exe", DataType::Utf8, true),
            Field::new("cmdline", DataType::Utf8, true),
            Field::new("boot_id", DataType::Utf8, true),
            Field::new("machine_id", DataType::Utf8, true),
        ]));

        Self {
            registry,
            file_index_cache,
            indexing_service,
            after,
            before,
            facets,
            schema,
        }
    }

    /// Query logs and convert to Arrow RecordBatch
    async fn query_logs(&self) -> Result<Vec<RecordBatch>> {
        info!(
            "Querying journal logs from {} to {}",
            self.after, self.before
        );

        // Find files in the time range
        let file_infos = self
            .registry
            .find_files_in_range(self.after, self.before)
            .map_err(|e| anyhow::anyhow!("Failed to find files: {}", e))?;

        info!("Found {} files in range", file_infos.len());

        if file_infos.is_empty() {
            info!("No files found in time range, returning empty result");
            return Ok(vec![]);
        }

        // Request indexing for all files in the range
        let facets_obj = Facets::new(&self.facets);

        info!("Requesting indexing for {} files", file_infos.len());
        for file_info in file_infos.iter() {
            let key = FileIndexKey::new(&file_info.file, &facets_obj);
            let source_timestamp_field = journal_index::FieldName::new_unchecked("_SOURCE_REALTIME_TIMESTAMP");
            let bucket_duration = 60; // 60 second buckets
            let request = FileIndexRequest::new(key, source_timestamp_field, bucket_duration);
            self.indexing_service.queue_indexing(request);
        }

        // Wait for indexing to complete (with timeout)
        let wait_start = std::time::Instant::now();
        let max_wait = std::time::Duration::from_secs(30);

        // Collect indexed files from cache
        let mut indexed_files = Vec::new();

        loop {
            indexed_files.clear();

            for file_info in file_infos.iter() {
                let key = FileIndexKey::new(&file_info.file, &facets_obj);
                match self.file_index_cache.get(&key).await {
                    Ok(Some(index)) => {
                        indexed_files.push(index);
                    }
                    Ok(None) => {
                        debug!("File not yet indexed: {:?}", file_info.file.path());
                    }
                    Err(e) => {
                        warn!("Failed to get index from cache: {}", e);
                    }
                }
            }

            if indexed_files.len() == file_infos.len() {
                info!("All files indexed successfully");
                break;
            }

            if wait_start.elapsed() > max_wait {
                warn!("Timeout waiting for indexing. {} of {} files indexed",
                      indexed_files.len(), file_infos.len());
                break;
            }

            // Wait a bit before checking again
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        }

        info!("Found {} indexed files in cache", indexed_files.len());

        if indexed_files.is_empty() {
            warn!("No indexed files available");
            return Ok(vec![]);
        }

        // Execute log query
        let anchor_usec = self.after as u64 * 1_000_000;
        let log_entries = LogQuery::new(&indexed_files)
            .with_direction(Direction::Forward)
            .with_anchor_usec(anchor_usec)
            .with_limit(100000) // Default limit, could be made configurable
            .execute()
            .map_err(|e| anyhow::anyhow!("Log query failed: {}", e))?;

        info!("Retrieved {} log entries", log_entries.len());

        // Convert to Arrow RecordBatch
        self.convert_to_record_batch(log_entries)
    }

    /// Convert log entries to Arrow RecordBatch
    fn convert_to_record_batch(&self, entries: Vec<LogEntryData>) -> Result<Vec<RecordBatch>> {
        if entries.is_empty() {
            return Ok(vec![]);
        }

        // Create builders for each column
        let mut timestamp_builder = TimestampMicrosecondBuilder::new();
        let mut priority_builder = UInt32Builder::new();
        let mut message_builder = StringBuilder::new();
        let mut syslog_identifier_builder = StringBuilder::new();
        let mut systemd_unit_builder = StringBuilder::new();
        let mut hostname_builder = StringBuilder::new();
        let mut pid_builder = StringBuilder::new();
        let mut uid_builder = StringBuilder::new();
        let mut gid_builder = StringBuilder::new();
        let mut comm_builder = StringBuilder::new();
        let mut exe_builder = StringBuilder::new();
        let mut cmdline_builder = StringBuilder::new();
        let mut boot_id_builder = StringBuilder::new();
        let mut machine_id_builder = StringBuilder::new();

        // Process each entry
        for entry in entries {
            // Timestamp (required field)
            timestamp_builder.append_value(entry.timestamp as i64);

            // Extract well-known fields
            let mut priority = None;
            let mut message = None;
            let mut syslog_identifier = None;
            let mut systemd_unit = None;
            let mut hostname = None;
            let mut pid = None;
            let mut uid = None;
            let mut gid = None;
            let mut comm = None;
            let mut exe = None;
            let mut cmdline = None;
            let mut boot_id = None;
            let mut machine_id = None;

            for field in &entry.fields {
                let name = field.field();
                let value = field.value();
                let name_str = name.as_ref();

                match name_str {
                    "PRIORITY" => {
                        priority = value.parse::<u32>().ok();
                    }
                    "MESSAGE" => {
                        message = Some(value);
                    }
                    "SYSLOG_IDENTIFIER" => {
                        syslog_identifier = Some(value);
                    }
                    "_SYSTEMD_UNIT" => {
                        systemd_unit = Some(value);
                    }
                    "_HOSTNAME" => {
                        hostname = Some(value);
                    }
                    "_PID" => {
                        pid = Some(value);
                    }
                    "_UID" => {
                        uid = Some(value);
                    }
                    "_GID" => {
                        gid = Some(value);
                    }
                    "_COMM" => {
                        comm = Some(value);
                    }
                    "_EXE" => {
                        exe = Some(value);
                    }
                    "_CMDLINE" => {
                        cmdline = Some(value);
                    }
                    "_BOOT_ID" => {
                        boot_id = Some(value);
                    }
                    "_MACHINE_ID" => {
                        machine_id = Some(value);
                    }
                    _ => {}
                }
            }

            // Append values to builders (None becomes NULL)
            if let Some(p) = priority {
                priority_builder.append_value(p);
            } else {
                priority_builder.append_null();
            }

            append_option_str(&mut message_builder, message);
            append_option_str(&mut syslog_identifier_builder, syslog_identifier);
            append_option_str(&mut systemd_unit_builder, systemd_unit);
            append_option_str(&mut hostname_builder, hostname);
            append_option_str(&mut pid_builder, pid);
            append_option_str(&mut uid_builder, uid);
            append_option_str(&mut gid_builder, gid);
            append_option_str(&mut comm_builder, comm);
            append_option_str(&mut exe_builder, exe);
            append_option_str(&mut cmdline_builder, cmdline);
            append_option_str(&mut boot_id_builder, boot_id);
            append_option_str(&mut machine_id_builder, machine_id);
        }

        // Finish builders and create arrays
        let columns: Vec<ArrayRef> = vec![
            Arc::new(timestamp_builder.finish()),
            Arc::new(priority_builder.finish()),
            Arc::new(message_builder.finish()),
            Arc::new(syslog_identifier_builder.finish()),
            Arc::new(systemd_unit_builder.finish()),
            Arc::new(hostname_builder.finish()),
            Arc::new(pid_builder.finish()),
            Arc::new(uid_builder.finish()),
            Arc::new(gid_builder.finish()),
            Arc::new(comm_builder.finish()),
            Arc::new(exe_builder.finish()),
            Arc::new(cmdline_builder.finish()),
            Arc::new(boot_id_builder.finish()),
            Arc::new(machine_id_builder.finish()),
        ];

        // Create RecordBatch
        let batch = RecordBatch::try_new(self.schema.clone(), columns)
            .map_err(|e| anyhow::anyhow!("Failed to create RecordBatch: {}", e))?;

        Ok(vec![batch])
    }
}

/// Helper to append Option<&str> to StringBuilder
fn append_option_str(builder: &mut StringBuilder, value: Option<&str>) {
    if let Some(v) = value {
        builder.append_value(v);
    } else {
        builder.append_null();
    }
}

#[async_trait]
impl TableProvider for JournalTableProvider {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        _filters: &[datafusion::prelude::Expr],
        _limit: Option<usize>,
    ) -> DataFusionResult<Arc<dyn ExecutionPlan>> {
        // Store the full schema for querying, but pass projection for output schema
        Ok(Arc::new(JournalExec::new(
            self.registry.clone(),
            self.file_index_cache.clone(),
            self.indexing_service.clone(),
            self.after,
            self.before,
            self.facets.clone(),
            self.schema.clone(), // Pass full schema - JournalExec will create projected schema
            projection.cloned(),
        )))
    }
}

/// ExecutionPlan for journal table scans
#[derive(Clone)]
struct JournalExec {
    registry: Registry,
    file_index_cache: FileIndexCache,
    indexing_service: IndexingService,
    after: u32,
    before: u32,
    facets: Vec<String>,
    schema: SchemaRef,
    projection: Option<Vec<usize>>,
}

impl fmt::Debug for JournalExec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournalExec")
            .field("after", &self.after)
            .field("before", &self.before)
            .field("facets", &self.facets)
            .field("schema", &self.schema)
            .finish_non_exhaustive()
    }
}

impl JournalExec {
    fn new(
        registry: Registry,
        file_index_cache: FileIndexCache,
        indexing_service: IndexingService,
        after: u32,
        before: u32,
        facets: Vec<String>,
        schema: SchemaRef,
        projection: Option<Vec<usize>>,
    ) -> Self {
        // Apply projection to schema if provided
        let projected_schema = match &projection {
            Some(proj) => {
                // Even if proj is empty (for COUNT(*)), create the projected schema
                let fields: Vec<_> = proj.iter().map(|i| schema.field(*i).clone()).collect();
                Arc::new(Schema::new(fields))
            }
            None => schema.clone(),
        };

        Self {
            registry,
            file_index_cache,
            indexing_service,
            after,
            before,
            facets,
            schema: projected_schema,
            projection,
        }
    }

    /// Query logs (similar to JournalTableProvider::query_logs but as a standalone function)
    async fn query_logs_internal(
        registry: &Registry,
        file_index_cache: &FileIndexCache,
        indexing_service: &IndexingService,
        after: u32,
        before: u32,
        facets: &[String],
        full_schema: SchemaRef,
        projection: &Option<Vec<usize>>,
    ) -> Result<Vec<RecordBatch>> {
        // Query with full schema first
        let provider = JournalTableProvider {
            registry: registry.clone(),
            file_index_cache: file_index_cache.clone(),
            indexing_service: indexing_service.clone(),
            after,
            before,
            facets: facets.to_vec(),
            schema: full_schema.clone(),
        };
        let batches = provider.query_logs().await?;

        // Apply projection if needed
        match projection {
            Some(proj) if proj.is_empty() => {
                // Special case for COUNT(*) - create empty schema batches with row counts
                let projected_batches: Result<Vec<_>> = batches
                    .into_iter()
                    .map(|batch| {
                        let row_count = batch.num_rows();
                        let empty_schema = Arc::new(Schema::empty());
                        RecordBatch::try_new_with_options(
                            empty_schema,
                            vec![],
                            &datafusion::arrow::array::RecordBatchOptions::new()
                                .with_row_count(Some(row_count)),
                        )
                        .map_err(|e| anyhow::anyhow!("Failed to create empty batch: {}", e))
                    })
                    .collect();
                projected_batches
            }
            Some(proj) => {
                // Project the columns
                let projected_batches: Result<Vec<_>> = batches
                    .into_iter()
                    .map(|batch| {
                        let columns: Vec<ArrayRef> =
                            proj.iter().map(|i| batch.column(*i).clone()).collect();
                        let fields: Vec<Field> =
                            proj.iter().map(|i| full_schema.field(*i).clone()).collect();
                        let projected_schema = Arc::new(Schema::new(fields));
                        RecordBatch::try_new(projected_schema, columns)
                            .map_err(|e| anyhow::anyhow!("Failed to create projected batch: {}", e))
                    })
                    .collect();
                projected_batches
            }
            None => Ok(batches),
        }
    }
}

impl DisplayAs for JournalExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "JournalExec")
    }
}

impl ExecutionPlan for JournalExec {
    fn name(&self) -> &str {
        "JournalExec"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.schema.clone()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![]
    }

    fn with_new_children(
        self: Arc<Self>,
        _children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> DataFusionResult<Arc<dyn ExecutionPlan>> {
        Ok(self)
    }

    fn execute(
        &self,
        _partition: usize,
        _context: Arc<TaskContext>,
    ) -> DataFusionResult<SendableRecordBatchStream> {
        let registry = self.registry.clone();
        let file_index_cache = self.file_index_cache.clone();
        let indexing_service = self.indexing_service.clone();
        let after = self.after;
        let before = self.before;
        let facets = self.facets.clone();

        // We need to get the full schema to query with, but self.schema is already projected
        // So we need to store both full and projected schema
        // For now, let's get the full schema from the provider
        let full_schema = Arc::new(Schema::new(vec![
            Field::new(
                "timestamp",
                DataType::Timestamp(TimeUnit::Microsecond, None),
                false,
            ),
            Field::new("priority", DataType::UInt32, true),
            Field::new("message", DataType::Utf8, true),
            Field::new("syslog_identifier", DataType::Utf8, true),
            Field::new("systemd_unit", DataType::Utf8, true),
            Field::new("hostname", DataType::Utf8, true),
            Field::new("pid", DataType::Utf8, true),
            Field::new("uid", DataType::Utf8, true),
            Field::new("gid", DataType::Utf8, true),
            Field::new("comm", DataType::Utf8, true),
            Field::new("exe", DataType::Utf8, true),
            Field::new("cmdline", DataType::Utf8, true),
            Field::new("boot_id", DataType::Utf8, true),
            Field::new("machine_id", DataType::Utf8, true),
        ]));

        let projection = self.projection.clone();
        let output_schema = self.schema.clone();

        // Create an async stream that will query logs when polled
        let stream = stream::once(async move {
            Self::query_logs_internal(
                &registry,
                &file_index_cache,
                &indexing_service,
                after,
                before,
                &facets,
                full_schema,
                &projection,
            )
            .await
            .map_err(|e| DataFusionError::Execution(format!("Failed to query logs: {}", e)))
        });

        let stream = stream.flat_map(|result| {
            stream::iter(match result {
                Ok(batches) => batches.into_iter().map(Ok).collect(),
                Err(e) => vec![Err(e)],
            })
        });

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            output_schema,
            stream,
        )))
    }

    fn statistics(&self) -> DataFusionResult<datafusion::physical_plan::Statistics> {
        Ok(datafusion::physical_plan::Statistics::new_unknown(
            self.schema.as_ref(),
        ))
    }

    fn properties(&self) -> &PlanProperties {
        // Create a static PlanProperties instance
        use std::sync::OnceLock;

        static PROPERTIES: OnceLock<PlanProperties> = OnceLock::new();
        PROPERTIES.get_or_init(|| {
            PlanProperties::new(
                EquivalenceProperties::new(self.schema.clone()),
                Partitioning::UnknownPartitioning(1),
                EmissionType::Incremental,
                Boundedness::Bounded,
            )
        })
    }
}
