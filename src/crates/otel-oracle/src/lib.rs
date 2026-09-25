//! Reference calculator for the traces explorer.
//!
//! Recomputes, from the raw OTLP spans, the numbers the `otel-traces` Function
//! returns, so tests can compare the two automatically. It is only useful while
//! it stays independent of the code it judges:
//!
//! - it never depends on `sfsq`, `otel-ledger`, `ng-index` or the query side of
//!   `sfst` (the `independence` test fails the build otherwise);
//! - its calculations are naive on purpose: full scans over an in-memory span
//!   list, ordered maps, full sorts, exact integer arithmetic.
//!
//! [`corpus`] generates the deterministic multi-service traces both sides are
//! fed.

pub mod corpus;
