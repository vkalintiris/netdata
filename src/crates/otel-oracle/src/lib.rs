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
//! fed; [`model`] rebuilds the stored rows from them; [`calc`] computes the
//! numbers. On live data the spans come from [`capture`], written by the
//! `otel-tee` binary in front of the lab agent, [`ingest`] replays which of
//! them the agent's ingestion window kept, and [`membership`] reads which rows
//! the agent's store holds and in which unit (the only module allowed to read
//! the store, and only its id, time and duration columns); [`matching`] pairs
//! the two and says which windows can be judged.

pub mod calc;
pub mod capture;
pub mod corpus;
pub mod ingest;
pub mod matching;
pub mod membership;
pub mod model;
