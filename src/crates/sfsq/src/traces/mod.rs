//! Multi-source trace-query subsystem: cross-source trace-by-id, search,
//! the trace-level overview, and the span explorer ([`explore`]).
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
mod gate;
mod overview;
mod predicate;
mod fold;
mod rollup;
mod search;
mod sources;
mod status;
mod vocab;
mod wal_scan;
mod window;

pub use by_id::{DEFAULT_SPAN_CAP, FieldKinds, TraceData, TraceQuery, TraceRequestError, trace_by_id};
pub use overview::{
    DURATION_BIN_COUNT, DURATION_BIN_LABELS, DurationPercentiles, FACET_TOP_K, FacetList,
    OverviewData, OverviewQuery, OverviewRequestError, RootFacets, overview,
};
pub use rollup::{
    TraceAggregate, TraceRootInfo, sealed_trace_aggregates, sealed_trace_envelopes,
    tail_trace_aggregates,
};
pub use predicate::{
    CompareOp, Condition, Predicate, PredicateError, PredicateTarget, PredicateValue,
    span_matches,
};
pub use search::{
    DEFAULT_SEARCH_LIMIT, DEFAULT_SPANS_PER_TRACE, SPANS_PER_TRACE_MAX, SearchData, SearchQuery, SearchRequestError,
    SearchSources, TraceSummary, search,
};
pub use sources::{
    SourceId, SourceSetError, TraceFailed, TraceSfstCandidate, TraceSource, TraceUnavailable,
    TraceWalTail, WalCoverage, validate_sources,
};
pub use status::{PartialReason, QueryStatus, ReasonCount, StatusBuilder};
pub use vocab::{AttributeOwner, BuiltinField};
pub use wal_scan::{TraceScanError, TraceWalScan};
pub use window::{TimeWindow, WindowError};
