//! Multi-file log-query subsystem over SFST indexes.
//!
//! Extracted from `otel-ledger`'s function handler: the filter →
//! facets / histogram → pagination → row-materialization → UI-envelope
//! pipeline that turns a set of overlapping SFST files plus a request
//! into a single wire response. The ledger keeps only the thin
//! `FunctionHandler` glue (registry access, async, capability
//! declaration) and calls into here.
//!
//! Distinct from the crate-root single-file [`crate::LogQuery`] API,
//! which this will eventually be unified with (the single-file query
//! is the natural per-file primitive under the multi-file engine).

pub mod adapter;
pub mod cursor;
pub mod engine;
pub mod types;
pub mod wire;

pub use engine::{SfstCandidate, effective_window, run};
