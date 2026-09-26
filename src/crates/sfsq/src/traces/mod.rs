//! Multi-source trace-query subsystem: cross-source trace-by-id and the span
//! explorer ([`explore`]).
//!
//! Same philosophy as [`logs`](crate::logs): neutral, transport-free —
//! plain Rust data in and out, no wire concerns; each consumer (the CLI,
//! a future Netdata UI) maps its own request/response format onto it.
//! The combiner, status, and identity contracts implemented here are
//! pinned by the integration suites under `tests/`.
//!
//! Two deliberate differences from the logs engine:
//!
//! - **No silent degradation.** A source that fails to map or decode is
//!   reported through the query-level [`QueryStatus`] (a
//!   [`SourceFailure`](PartialReason::SourceFailure) reason), and one
//!   whose bytes could not be obtained at all ([`TraceSource::Unavailable`])
//!   as [`RemoteUnavailable`](PartialReason::RemoteUnavailable); a source
//!   the caller could not make queryable ([`TraceSource::Failed`], e.g. a
//!   WAL whose chunks failed to build) is a `SourceFailure` too. None is
//!   silently skipped: a trace is an exact object, and "some spans
//!   were quietly missing" is corruption from the consumer's point of view.
//! - **Validated source identity.** Every source carries a
//!   caller-supplied opaque [`SourceId`]; WAL-derived sources also carry
//!   [`WalCoverage`]. Duplicates and overlapping WAL ranges are rejected
//!   up front — a duplicated source would double UNSET-span-id spans,
//!   which deliberately never deduplicate.

mod by_id;
pub mod duration_hist;
pub mod explore;
mod sources;
mod wal_scan;
mod window;

pub use crate::status::{PartialReason, QueryStatus, ReasonCount, StatusBuilder};
pub use by_id::{DEFAULT_SPAN_CAP, FieldKinds, TraceData, TraceQuery, TraceRequestError, trace_by_id};
pub use sources::{
    SourceId, SourceSetError, TraceFailed, TraceSfstCandidate, TraceSource, TraceUnavailable,
    TraceWalTail, WalCoverage, validate_sources,
};
pub use wal_scan::{TraceScanError, TraceWalScan};
pub use window::{TimeWindow, WindowError};
