//! The `TableProvider` that exposes one SFST file as a DataFusion table.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::{Session, TableProvider};
use datafusion::common::stats::Precision;
use datafusion::common::{ColumnStatistics, ScalarValue, Statistics};
use datafusion::error::Result;
use datafusion::logical_expr::{Expr, TableProviderFilterPushDown, TableType};
use datafusion::physical_plan::ExecutionPlan;

use crate::exec::SfstExec;
use crate::schema::SfstSchema;

/// A single SFST file presented as a queryable table. Owns the file bytes; the
/// borrowed `IndexReader` is reopened per scan inside the execution plan.
pub struct SfstTable {
    data: Arc<Vec<u8>>,
    schema: Arc<SfstSchema>,
    record_count: u32,
    /// Chronological min/max timestamp (ns) for the `timestamp` column stats.
    min_ts_ns: i64,
    max_ts_ns: i64,
}

impl fmt::Debug for SfstTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SfstTable")
            .field("record_count", &self.record_count)
            .field("columns", &self.schema.schema.fields().len())
            .finish()
    }
}

impl SfstTable {
    /// Open an SFST file from its raw bytes.
    pub fn open(data: Vec<u8>) -> std::result::Result<Self, sfst::Error> {
        let reader = sfst::IndexReader::open(&data)?;
        let schema = SfstSchema::build(&reader);
        let record_count = reader.summary().record_count;
        // Timestamps are chronological, so the head/tail are the min/max.
        let ts = reader.load_timestamps()?;
        let (min_ts_ns, max_ts_ns) = if ts.is_empty() {
            (0, 0)
        } else {
            (
                ts.at(0).unwrap_or(0),
                ts.at(record_count.saturating_sub(1)).unwrap_or(0),
            )
        };
        // Reader borrows `data`; drop it before moving the bytes into the Arc.
        drop(reader);
        Ok(Self {
            data: Arc::new(data),
            schema: Arc::new(schema),
            record_count,
            min_ts_ns,
            max_ts_ns,
        })
    }

    /// Open an SFST file from a path.
    pub fn open_path(path: impl AsRef<std::path::Path>) -> std::result::Result<Self, sfst::Error> {
        Self::open(std::fs::read(path)?)
    }

    /// The file bytes (shared); used by the aggregate-pushdown plan.
    pub(crate) fn data(&self) -> Arc<Vec<u8>> {
        self.data.clone()
    }

    /// The table's schema + per-column specs (kind + tier).
    pub(crate) fn sfst_schema(&self) -> &SfstSchema {
        &self.schema
    }
}

#[async_trait]
impl TableProvider for SfstTable {
    fn schema(&self) -> SchemaRef {
        self.schema.schema.clone()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        _state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let (filter, window) = crate::pushdown::plan(filters, &self.schema);
        let exec = SfstExec::new(
            self.data.clone(),
            self.schema.clone(),
            projection.cloned(),
            filter,
            window,
            limit,
        )?;
        Ok(Arc::new(exec))
    }

    fn supports_filters_pushdown(
        &self,
        filters: &[&Expr],
    ) -> Result<Vec<TableProviderFilterPushDown>> {
        Ok(crate::pushdown::verdicts(filters, &self.schema))
    }

    fn statistics(&self) -> Option<Statistics> {
        let ncols = self.schema.schema.fields().len();
        let mut column_statistics = vec![ColumnStatistics::new_unknown(); ncols];
        // Column 0 is `timestamp`: exact, non-null, with known bounds.
        if let Some(ts) = column_statistics.first_mut() {
            ts.null_count = Precision::Exact(0);
            ts.min_value =
                Precision::Exact(ScalarValue::TimestampNanosecond(Some(self.min_ts_ns), None));
            ts.max_value =
                Precision::Exact(ScalarValue::TimestampNanosecond(Some(self.max_ts_ns), None));
        }
        Some(Statistics {
            num_rows: Precision::Exact(self.record_count as usize),
            total_byte_size: Precision::Absent,
            column_statistics,
        })
    }
}
