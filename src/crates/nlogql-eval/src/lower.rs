//! AST → Plan lowering.
//!
//! Translates an [`nlogql::ast::Expr`] into a [`crate::plan::Plan`],
//! performing the semantic checks that the parser intentionally
//! defers (see `EXPECTED_FAILS.md` in the `nlogql` crate):
//!
//! - `topk` / `bottomk` / `approx_topk` require a numeric first arg.
//! - Other vector ops reject a numeric first arg.
//! - `quantile_over_time` requires its quantile in `[0, 1]`.
//!
//! Stub today — every input yields `Err(LowerError::Unimplemented)`.
//! Productions land in SOW-D2 and SOW-D3.

use nlogql::ast::Expr;

use crate::error::LowerError;
use crate::plan::Plan;

/// Lower a parsed LogQL AST into an executable [`Plan`].
pub fn lower(_expr: &Expr) -> Result<Plan, LowerError> {
    Err(LowerError::Unimplemented {
        what: "lower() — pending SOW-D2 (log path) and SOW-D3 (metric path)",
    })
}
