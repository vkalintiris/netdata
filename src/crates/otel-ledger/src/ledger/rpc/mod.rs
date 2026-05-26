//! Function-call dispatch and supervisor request handling.
//!
//! Submodules:
//!
//! - `dispatch` — the run-loop's entry points: `handle_supervisor_req`,
//!   `handle_outbound_resp`, and the per-call `dispatch_function_call`
//!   that spawns handler tasks driven by the `bridge::function` engine.
//! - `handler` — `OtelLogsHandler`, the typed `FunctionHandler` impl,
//!   its declaration, and the otel-logs–specific args→payload shim.
//! - `types` — request shape and top-level response enum.
//! - `wire` — Netdata UI response envelope (facets, histogram, items, …).
//! - `adapter` — SFST query results → UI envelope conversions.

mod adapter;
mod dispatch;
mod handler;
mod types;
mod wire;

pub(crate) use handler::OtelLogsHandler;
