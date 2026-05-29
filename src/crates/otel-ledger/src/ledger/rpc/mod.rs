//! Function-call dispatch and supervisor request handling.
//!
//! Submodules:
//!
//! - `dispatch` — the run-loop's entry points: `handle_supervisor_req`,
//!   `handle_outbound_resp`, and the per-call `dispatch_function_call`
//!   that spawns handler tasks driven by the `bridge::function` engine.
//! - `handler` — `OtelLogsHandler`, the typed `FunctionHandler` impl,
//!   its declaration, and the otel-logs–specific args→payload shim.
//!
//! The query subsystem it drives (request/response types, the wire
//! envelope, SFST→UI adapters, the cursor codec, and the multi-file
//! query engine) lives in the [`sfsq::logs`] crate module.

mod dispatch;
mod handler;

pub(crate) use handler::OtelLogsHandler;
