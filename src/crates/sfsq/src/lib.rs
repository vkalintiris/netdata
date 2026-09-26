//! Query layer over SFST indexes.
//!
//! [`logs`] is the multi-file log-query engine: given a set of
//! overlapping SFST files and a [`logs::LogsQuery`], it produces a single
//! [`logs::LogsData`] — facets, a histogram, and a paginated,
//! materialized page of log rows. The API is wire-neutral (plain Rust in
//! and out); a consumer maps its own request/response format onto it.
//! See [`logs::run`] for the entry point.
//!
//! [`traces`] is the trace-query engine over the same source kinds
//! (sealed SFSTs, in-memory chunk SFSTs, WAL tails): cross-source
//! trace-by-id through one shared combiner. Same wire-neutral philosophy;
//! design authority: the phase-4 design record in the traces plan repo.
//!
//! [`Source`] (bytes provenance: sealed file or in-memory chunk image) and
//! the result status ([`QueryStatus`]) are shared by both engines.

pub mod logs;
pub mod traces;

pub(crate) mod merge;

mod source;
mod status;

pub use source::Source;
pub use status::{PartialReason, QueryStatus, ReasonCount, StatusBuilder};
