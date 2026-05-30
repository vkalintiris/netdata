//! Multi-file log-query subsystem over SFST indexes.
//!
//! The pipeline that turns a set of overlapping SFST files plus a
//! request into a single query response: filter → facets / histogram →
//! pagination → row materialization → response envelope. [`run`] is the
//! entry point — it takes the candidate files and a [`LogsRequest`] and
//! returns a [`LogsResult`].
//!
//! The query itself is pure and synchronous; opening and decompressing
//! the SFST files is its only I/O. Resolving which files overlap a
//! request window, and scheduling the work off an async runtime thread,
//! is left to the caller.

mod adapter;
mod cursor;
mod engine;
mod types;
mod wire;

// The crate's public log-query API. Internals (the SFST→UI adapters,
// the cursor codec, the wire sub-structs) stay module-private; only
// these are re-exported.
pub use engine::{PreparedQuery, SfstCandidate, run};
pub use types::{InfoResponse, LogsRequest, LogsResponse};
pub use wire::LogsResult;
