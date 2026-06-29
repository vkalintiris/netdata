//! Experimental DataFusion integration for SFST log-index files.
//!
//! Stage A (SOW-20260629-datafusion-sfst-stage-a): a custom [`SfstTable`]
//! `TableProvider` + leaf `ExecutionPlan` that exposes **one** SFST file as a
//! queryable table. Register it on a `SessionContext` and run SQL:
//!
//! ```no_run
//! # async fn run() -> datafusion::error::Result<()> {
//! use std::sync::Arc;
//! use datafusion::prelude::SessionContext;
//! use sfst_datafusion::SfstTable;
//!
//! let table = SfstTable::open_path("out.sfst").unwrap();
//! let ctx = SessionContext::new();
//! ctx.register_table("logs", Arc::new(table))?;
//! let df = ctx.sql("SELECT timestamp FROM logs LIMIT 10").await?;
//! df.show().await?;
//! # Ok(())
//! # }
//! ```
//!
//! Not wired into production; the v9 format and the ng-index seal path are
//! untouched. See the SOW for the recorded design forks (D1–D5) to revisit.

mod exec;
mod pushdown;
mod schema;
mod table;

pub use schema::{ColKind, ColumnSpec, SfstSchema, TS_COLUMN};
pub use table::SfstTable;
