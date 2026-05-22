//! Plan IR — the post-lowering representation that the evaluator
//! consumes.
//!
//! Variants land progressively:
//! - SOW-D2 introduces `Plan::Log` (log query path).
//! - SOW-D3 introduces `Plan::Metric` (range/vector aggs, binops,
//!   `label_replace`, `vector(N)`).
//!
//! Until then this is a unit-struct placeholder so the public
//! `lower()` signature can compile.

/// Lowered query plan. Always reached via [`crate::lower`].
#[derive(Debug, Clone, PartialEq)]
pub struct Plan;
