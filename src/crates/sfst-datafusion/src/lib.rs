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

mod aggregate;
mod exec;
mod pushdown;
mod schema;
mod table;

use std::sync::Arc;

pub use schema::{ColKind, ColumnSpec, SfstSchema, TS_COLUMN};
pub use table::SfstTable;

/// A `SessionContext` wired for SFST: the facet aggregation-pushdown optimizer
/// rule plus the query planner that plans its node. Use this instead of
/// `SessionContext::new()` to get `COUNT(*) GROUP BY <field>` answered from
/// facet bitmaps; without it, queries still run correctly via the normal plan.
///
/// `information_schema` is enabled, so `SHOW TABLES`, `SHOW COLUMNS FROM <t>`,
/// and `SELECT … FROM information_schema.columns` work (alongside `DESCRIBE`).
pub fn session_context() -> datafusion::prelude::SessionContext {
    use datafusion::execution::session_state::SessionStateBuilder;
    use datafusion::prelude::{SessionConfig, SessionContext};

    let config = SessionConfig::new().with_information_schema(true);
    let state = SessionStateBuilder::new()
        .with_config(config)
        .with_default_features()
        .with_query_planner(Arc::new(aggregate::SfstQueryPlanner))
        .build();
    let ctx = SessionContext::new_with_state(state);
    ctx.add_optimizer_rule(Arc::new(aggregate::SfstFacetRule));
    ctx
}
