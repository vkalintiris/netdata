//! The leaf `ExecutionPlan` that scans one SFST file into Arrow batches.
//!
//! Stage A (decision D3=A): rows are reconstructed via `IndexReader`'s existing
//! `materialize_rows` and pivoted into projected Arrow columns. This proves the
//! integration contract; the column-direct read (D3=B) is the recorded revisit.

use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use datafusion::arrow::array::{
    ArrayRef, BooleanBuilder, Float64Builder, Int64Builder, ListBuilder, StringBuilder,
    TimestampNanosecondBuilder,
};
use datafusion::arrow::compute::SortOptions;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::arrow::record_batch::{RecordBatch, RecordBatchOptions};
use datafusion::common::{project_schema, DataFusionError};
use datafusion::error::Result;
use datafusion::execution::TaskContext;
use datafusion::physical_expr::expressions::Column;
use datafusion::physical_expr::{EquivalenceProperties, PhysicalSortExpr};
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::memory::MemoryStream;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use sfst::{Filter, IndexReader};

use crate::schema::{ColKind, ColumnSpec, SfstSchema};

/// Map any sfst-side error into a DataFusion execution error.
fn exec_err(e: impl fmt::Display) -> DataFusionError {
    DataFusionError::Execution(format!("sfst: {e}"))
}

pub struct SfstExec {
    /// Owned file bytes; the borrowed `IndexReader` is opened per `execute`.
    data: Arc<Vec<u8>>,
    /// Full table schema + pivot specs (shared with the table provider).
    table: Arc<SfstSchema>,
    /// Column indices into the full schema, or `None` for all columns.
    projection: Option<Vec<usize>>,
    /// Projected output schema (what `execute` emits).
    projected_schema: SchemaRef,
    /// Pushed-down equality predicates (empty = match all).
    filter: Filter,
    /// Pushed-down half-open time window (`i64::MIN..i64::MAX` = unbounded).
    window: Range<i64>,
    /// Pushed-down row cap, applied to the time-sorted positions.
    limit: Option<usize>,
    cache: Arc<PlanProperties>,
}

impl SfstExec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        data: Arc<Vec<u8>>,
        table: Arc<SfstSchema>,
        projection: Option<Vec<usize>>,
        filter: Filter,
        window: Range<i64>,
        limit: Option<usize>,
    ) -> Result<Self> {
        let projected_schema = project_schema(&table.schema, projection.as_ref())?;

        // SFST rows are stored time-sorted, so advertise ascending `timestamp`
        // order when that column is projected — `ORDER BY timestamp` then needs
        // no SortExec.
        let mut eq = EquivalenceProperties::new(projected_schema.clone());
        if let Ok(idx) = projected_schema.index_of(crate::schema::TS_COLUMN) {
            eq.add_ordering([PhysicalSortExpr::new(
                Arc::new(Column::new(crate::schema::TS_COLUMN, idx)),
                SortOptions {
                    descending: false,
                    nulls_first: false,
                },
            )]);
        }

        let cache = Arc::new(PlanProperties::new(
            eq,
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Ok(Self {
            data,
            table,
            projection,
            projected_schema,
            filter,
            window,
            limit,
            cache,
        })
    }

    /// Open the reader, select positions, build only the projected columns.
    fn build_batch(&self) -> Result<RecordBatch> {
        let reader = IndexReader::open(&self.data).map_err(exec_err)?;

        // Pushed-down equality predicates + time window (decision D5); an empty
        // filter and an unbounded window degrade to a full scan.
        let compiled = reader.compile_filter(&self.filter, None).map_err(exec_err)?;
        let mut positions = reader
            .matched_positions(&compiled, self.window.clone())
            .map_err(exec_err)?;

        // Positions are ascending (time order); a pushed LIMIT takes the head.
        if let Some(limit) = self.limit {
            positions.truncate(limit);
        }

        let indices: Vec<usize> = match &self.projection {
            Some(p) => p.clone(),
            None => (0..self.table.schema.fields().len()).collect(),
        };

        // Projection pushdown is inherent: each projected attribute column is
        // resolved column-direct (only that field's chunk is decoded), and a
        // COUNT(*) / timestamp-only scan touches no attribute chunk at all.
        let timestamps = reader.load_timestamps().map_err(exec_err)?;

        // Resolve every projected attribute column in one shared pass — the
        // high-card fields among them then share a single stream-batch scan.
        let attr_names: Vec<&str> = indices
            .iter()
            .filter(|&&i| i > 0)
            .map(|&i| self.table.specs[i - 1].name.as_str())
            .collect();
        let attr_values = if attr_names.is_empty() {
            Vec::new()
        } else {
            reader
                .materialize_fields(&attr_names, &positions)
                .map_err(exec_err)?
        };

        let mut columns: Vec<ArrayRef> = Vec::with_capacity(indices.len());
        let mut attr_cursor = 0usize;
        for &i in &indices {
            if i == 0 {
                // The timestamp column, sourced directly from TIMS by position.
                let mut b = TimestampNanosecondBuilder::with_capacity(positions.len());
                for &p in &positions {
                    match timestamps.at(p) {
                        Some(ts) => b.append_value(ts),
                        // A selected position with no timestamp means corrupt chunks.
                        None => return Err(exec_err(format!("position {p} has no timestamp"))),
                    }
                }
                columns.push(Arc::new(b.finish()));
            } else {
                let spec = &self.table.specs[i - 1];
                columns.push(build_attr_column(spec, &attr_values[attr_cursor]));
                attr_cursor += 1;
            }
        }

        // A zero-column projection (COUNT(*)) needs an explicit row count.
        let options = RecordBatchOptions::new().with_row_count(Some(positions.len()));
        RecordBatch::try_new_with_options(self.projected_schema.clone(), columns, &options)
            .map_err(Into::into)
    }
}

/// Build one Arrow column from a field's per-position values (the output of
/// `IndexReader::materialize_field`: `values[p]` is the list of that field's
/// values at output row `p`). Scalar columns take the single value (last wins
/// defensively); list columns take all; absence → null.
fn build_attr_column(spec: &ColumnSpec, values: &[Vec<String>]) -> ArrayRef {
    match spec.kind {
        ColKind::Str => {
            let mut b = StringBuilder::new();
            for v in values {
                match v.last() {
                    Some(s) => b.append_value(s),
                    None => b.append_null(),
                }
            }
            Arc::new(b.finish())
        }
        ColKind::Int => {
            let mut b = Int64Builder::with_capacity(values.len());
            for v in values {
                // Absent or non-numeric → null (Stage A: no error on bad parse).
                b.append_option(v.last().and_then(|s| s.parse::<i64>().ok()));
            }
            Arc::new(b.finish())
        }
        ColKind::Double => {
            let mut b = Float64Builder::with_capacity(values.len());
            for v in values {
                b.append_option(v.last().and_then(|s| s.parse::<f64>().ok()));
            }
            Arc::new(b.finish())
        }
        ColKind::Bool => {
            let mut b = BooleanBuilder::with_capacity(values.len());
            for v in values {
                b.append_option(v.last().and_then(|s| s.parse::<bool>().ok()));
            }
            Arc::new(b.finish())
        }
        ColKind::List => {
            let mut b = ListBuilder::new(StringBuilder::new());
            for v in values {
                for s in v {
                    b.values().append_value(s);
                }
                // empty → a null list slot (field absent at this position).
                b.append(!v.is_empty());
            }
            Arc::new(b.finish())
        }
    }
}

impl fmt::Debug for SfstExec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SfstExec")
    }
}

impl DisplayAs for SfstExec {
    fn fmt_as(&self, _t: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        let cols = self.projected_schema.fields().len();
        match self.limit {
            Some(l) => write!(f, "SfstExec: projection={cols} cols, limit={l}"),
            None => write!(f, "SfstExec: projection={cols} cols"),
        }
    }
}

impl ExecutionPlan for SfstExec {
    fn name(&self) -> &str {
        "SfstExec"
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.cache
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
        let batch = self.build_batch()?;
        Ok(Box::pin(MemoryStream::try_new(
            vec![batch],
            self.projected_schema.clone(),
            None,
        )?))
    }
}
