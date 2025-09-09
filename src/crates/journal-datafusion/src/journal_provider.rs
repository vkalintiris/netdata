use std::any::Any;
use std::sync::Arc;
use std::time::SystemTime;

use arrow::array::{StringArray, UInt64Array, ArrayRef};
use arrow::datatypes::{DataType, Field, Schema, SchemaRef, TimeUnit};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;

use datafusion_catalog::{Session, TableProvider};
use datafusion_common::{Result as DataFusionResult, DataFusionError};
use datafusion_expr::TableType;
use datafusion_expr::{Expr, TableProviderFilterPushDown};
use datafusion_common::{Column, ScalarValue};
// ExecutionPlan imported below

use journal_registry::{JournalRegistry, RegistryFile, SourceType};

/// A DataFusion TableProvider that enables SQL queries over journal files
pub struct JournalTableProvider {
    registry: Arc<JournalRegistry>,
    schema: SchemaRef,
}

impl std::fmt::Debug for JournalTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JournalTableProvider")
            .field("schema", &self.schema)
            .finish()
    }
}

impl JournalTableProvider {
    /// Create a new JournalTableProvider
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
        
        // Define schema for journal metadata table
        let schema = Arc::new(Schema::new(vec![
            Field::new("path", DataType::Utf8, false),
            Field::new("size", DataType::UInt64, false),
            Field::new("modified_timestamp", DataType::Timestamp(TimeUnit::Millisecond, None), false),
            Field::new("source_type", DataType::Utf8, false),
            Field::new("machine_id", DataType::Utf8, true),
            Field::new("sequence_number", DataType::UInt64, true),
            Field::new("first_timestamp", DataType::UInt64, true),
        ]));
        
        Ok(Self { registry, schema })
    }

    /// Check if a filter can be pushed down to the journal registry
    fn can_push_filter(&self, expr: &Expr) -> TableProviderFilterPushDown {
        use datafusion_expr::{BinaryExpr, Operator};
        
        match expr {
            // Support equality filters on string columns
            Expr::BinaryExpr(BinaryExpr { left, op: Operator::Eq, right }) => {
                if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                    match col.name.as_str() {
                        "source_type" => {
                            if let Some(_val) = lit.get_str_value() {
                                return TableProviderFilterPushDown::Exact;
                            }
                        }
                        "machine_id" => {
                            if let Some(_val) = lit.get_str_value() {
                                return TableProviderFilterPushDown::Exact;
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Support range filters on numeric columns  
            Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
                if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                    match (col.name.as_str(), op) {
                        ("size", Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq) => {
                            if lit.get_u64_value().is_some() {
                                return TableProviderFilterPushDown::Exact;
                            }
                        }
                        ("modified_timestamp", Operator::Gt | Operator::GtEq | Operator::Lt | Operator::LtEq) => {
                            // Support timestamp range queries
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
        
        // Apply pushed down filters to registry query
        for filter in filters {
            match apply_filter_to_query(query, filter) {
                Ok(new_query) => query = new_query,
                Err(e) => tracing::warn!("Failed to apply filter pushdown: {}", e),
            }
        }
        
        // Apply limit if specified
        if let Some(limit_value) = limit {
            query = query.limit(limit_value);
        }
        
        // Execute the query to get filtered registry files
        let registry_files = query.execute();
        
        // Create execution plan
        Ok(Arc::new(JournalExecutionPlan::new(
            registry_files,
            self.schema(),
            projection.cloned(),
        )))
    }
}

/// Apply a DataFusion filter to a JournalRegistry query for pushdown optimization
fn apply_filter_to_query(
    mut query: journal_registry::RegistryQuery,
    filter: &Expr
) -> anyhow::Result<journal_registry::RegistryQuery> {
    use datafusion_expr::{BinaryExpr, Operator};
    
    match filter {
        Expr::BinaryExpr(BinaryExpr { left, op: Operator::Eq, right }) => {
            if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                match col.name.as_str() {
                    "source_type" => {
                        if let Some(val) = lit.get_str_value() {
                            let source_type = match val {
                                "system" => SourceType::System,
                                "user" => SourceType::User,
                                "remote" => SourceType::Remote,
                                "namespace" => SourceType::Namespace,
                                "other" => SourceType::Other,
                                _ => return Ok(query), // Invalid source type, skip
                            };
                            query = query.source(source_type);
                        }
                    }
                    "machine_id" => {
                        if let Some(val) = lit.get_str_value() {
                            query = query.machine(val);
                        }
                    }
                    _ => {}
                }
            }
        }
        Expr::BinaryExpr(BinaryExpr { left, op, right }) => {
            if let (Ok(col), Some(lit)) = (left.try_as_col(), right.as_literal()) {
                match col.name.as_str() {
                    "size" => {
                        if let Some(val) = lit.get_u64_value() {
                            match op {
                                Operator::Gt => query = query.min_size(val + 1),
                                Operator::GtEq => query = query.min_size(val),
                                Operator::Lt => query = query.max_size(val - 1),
                                Operator::LtEq => query = query.max_size(val),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
    
    Ok(query)
}

/// Custom ExecutionPlan for journal metadata
pub struct JournalExecutionPlan {
    files: Vec<RegistryFile>,
    schema: SchemaRef,
    projection: Option<Vec<usize>>,
    properties: PlanProperties,
}

impl JournalExecutionPlan {
    pub fn new(
        files: Vec<RegistryFile>,
        schema: SchemaRef,
        projection: Option<Vec<usize>>,
    ) -> Self {
        let eq_properties = EquivalenceProperties::new(schema.clone());
        let properties = PlanProperties::new(
            eq_properties,
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        );
        
        Self {
            files,
            schema,
            projection,
            properties,
        }
    }

    fn build_record_batch(&self) -> DataFusionResult<RecordBatch> {
        let num_files = self.files.len();
        
        // Build arrays for each column
        let paths: ArrayRef = Arc::new(StringArray::from(
            self.files.iter()
                .map(|f| f.path.to_string_lossy().to_string())
                .collect::<Vec<_>>()
        ));
        
        let sizes: ArrayRef = Arc::new(UInt64Array::from(
            self.files.iter().map(|f| f.size).collect::<Vec<_>>()
        ));
        
        let modified_timestamps: ArrayRef = Arc::new(
            arrow::array::TimestampMillisecondArray::from(
                self.files.iter()
                    .map(|f| {
                        f.modified
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as i64
                    })
                    .collect::<Vec<_>>()
            )
        );
        
        let source_types: ArrayRef = Arc::new(StringArray::from(
            self.files.iter()
                .map(|f| f.source_type.to_string())
                .collect::<Vec<_>>()
        ));
        
        let machine_ids: ArrayRef = Arc::new(StringArray::from(
            self.files.iter()
                .map(|f| f.machine_id.clone())
                .collect::<Vec<_>>()
        ));
        
        let sequence_numbers: ArrayRef = Arc::new(UInt64Array::from(
            self.files.iter()
                .map(|f| f.sequence_number)
                .collect::<Vec<_>>()
        ));
        
        let first_timestamps: ArrayRef = Arc::new(UInt64Array::from(
            self.files.iter()
                .map(|f| f.first_timestamp)
                .collect::<Vec<_>>()
        ));
        
        let all_columns = vec![
            paths,
            sizes, 
            modified_timestamps,
            source_types,
            machine_ids,
            sequence_numbers,
            first_timestamps,
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

use datafusion_physical_plan::{
    DisplayAs, DisplayFormatType, 
    ExecutionPlan, 
    PlanProperties,
    Partitioning,
    SendableRecordBatchStream,
    execution_plan::{Boundedness, EmissionType},
    memory::MemoryStream
};
use datafusion_physical_expr::EquivalenceProperties;
use datafusion_common::Result;
use std::fmt;

impl std::fmt::Debug for JournalExecutionPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "JournalExecutionPlan: {} files", self.files.len())
    }
}

impl DisplayAs for JournalExecutionPlan {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "JournalScan: {} files", self.files.len())
    }
}

impl ExecutionPlan for JournalExecutionPlan {
    fn name(&self) -> &'static str {
        "JournalScan"
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
        _context: Arc<datafusion_execution::TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let batch = self.build_record_batch()?;
        
        Ok(Box::pin(MemoryStream::try_new(
            vec![batch],
            self.schema(),
            None,
        )?))
    }
}