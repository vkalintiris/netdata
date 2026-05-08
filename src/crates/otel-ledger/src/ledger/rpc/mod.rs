//! Function-call dispatch and supervisor request handling.
//!
//! Submodules:
//!
//! - `dispatch` — the run-loop's entry points: `handle_supervisor_req`,
//!   `handle_outbound_resp`, and the per-call `dispatch_function_call`
//!   that spawns handler tasks driven by the `bridge::function` engine.
//! - `handler` — `OtelLogsHandler`, the typed `FunctionHandler` impl,
//!   its declaration, and the otel-logs–specific args→payload shim.
//! - `types` — wire-format types: `OtelLogsRequest`, `OtelLogsResponse`,
//!   `Candidate`, etc.

mod dispatch;
mod handler;
mod types;

pub(crate) use handler::OtelLogsHandler;
