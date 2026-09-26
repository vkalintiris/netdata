//! `OtelTracesHandler` — typed `FunctionHandler` implementation for the
//! `otel-traces` Function.
//!
//! The modes: `info` (capability discovery), `explore` (the span explorer)
//! and `values` (its value suggestions), and `trace` (exact single-trace
//! fetch). Mode selection and every request-SHAPE validation happen during
//! deserialization (the wire's typed request — shape errors are transport
//! 400s); this handler owns only the semantic validation (trace-id shape,
//! span cap, bounds). The wire contract lives in [`super::wire`], the engine
//! mapping in [`super::adapter`], and source resolution in
//! [`super::sources`].
//!
//! Netdata-plugin glue only, like the logs handler: the engine
//! ([`sfsq::traces`]) stays wire-neutral; the bridge's `HandlerAdapter`
//! owns the JSON round-trip, progress ticker, and cancellation.

use std::sync::Arc;

use async_trait::async_trait;
use bridge::function::{FunctionCallContext, FunctionHandler};
use file_registry::TenantId;
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_types::HttpAccess;
use tokio::sync::{OwnedSemaphorePermit, RwLock, Semaphore};

use file_lifecycle::chunk::ChunkCache;
use file_lifecycle::registry::TenantRegistries;

use sfsq::traces::{TraceQuery, TraceRequestError, trace_by_id};

use super::adapter::{
    parse_trace_id, to_explore_query, to_explore_response, to_trace_result, to_values_query,
    to_values_response, validate_trace_bounds,
};
use super::sources::{Capture, CaptureError, TracesSourceSupplier};
use super::wire::{
    CoverageWire, ExploreParams, OtelTracesRequest, OtelTracesResponse, TraceParams, TracesMode,
    ValuesParams,
};
use file_lifecycle::remote_read::RemoteRead;

/// Shorthand for the handler-level error every failure path maps to.
fn handler_err(message: String) -> netdata_plugin_error::NetdataPluginError {
    netdata_plugin_error::NetdataPluginError::FunctionHandler { message }
}

/// A capture that failed as a whole: a hard error the caller can act on.
fn capture_error(e: CaptureError) -> netdata_plugin_error::NetdataPluginError {
    let size = |bytes: u64| bytesize::ByteSize::b(bytes).display().si().to_string();
    match e {
        CaptureError::TooLarge { at_least, capacity } => handler_err(format!(
            "this query needs more than {} of remote trace data (at least {}), more than the \
             download cache holds; narrow the time range or raise \
             `remote_storage.read_cache_max_size`",
            size(capacity),
            size(at_least)
        )),
        CaptureError::EvictionFailed => handler_err(
            "the remote-read download cache directory is unwritable (eviction failed); check \
             its permissions and free space"
                .to_string(),
        ),
        CaptureError::Planning(e) => {
            handler_err(format!("otel-traces remote planning task failed: {e}"))
        }
    }
}

/// Explorer, value and trace requests evaluated at once; the rest wait for a
/// turn, so a burst of refreshes cannot take every core and all the memory.
const ADMITTED_REQUESTS: usize = 2;

pub(crate) struct OtelTracesHandler {
    /// Live source resolution (registries snapshot + WAL chunk builds).
    supplier: TracesSourceSupplier,
    /// Turns for the requests [`ADMITTED_REQUESTS`] bounds.
    admission: Arc<Semaphore>,
}

impl OtelTracesHandler {
    /// `remote` reads files that local retention evicted back from remote
    /// storage; `None` when remote storage is disabled.
    pub(crate) fn new(
        registries: Arc<RwLock<TenantRegistries>>,
        chunk_cache: Arc<ChunkCache>,
        min_entries: u64,
        remote: Option<RemoteRead>,
    ) -> Self {
        Self::with_admission(
            registries,
            chunk_cache,
            min_entries,
            remote,
            ADMITTED_REQUESTS,
        )
    }

    fn with_admission(
        registries: Arc<RwLock<TenantRegistries>>,
        chunk_cache: Arc<ChunkCache>,
        min_entries: u64,
        remote: Option<RemoteRead>,
        admitted: usize,
    ) -> Self {
        Self {
            supplier: TracesSourceSupplier::new(registries, chunk_cache, min_entries, remote),
            admission: Arc::new(Semaphore::new(admitted)),
        }
    }

    /// Wait for a turn, or until the request is cancelled: then `None`, and
    /// the request goes on to its usual cancelled answer (the capture and the
    /// engine see the same token). The caller moves the permit into its
    /// blocking task, so the turn lasts as long as the work, even when the
    /// caller stops waiting for it.
    async fn admit(&self, ctx: &FunctionCallContext) -> Option<OwnedSemaphorePermit> {
        tokio::select! {
            permit = Arc::clone(&self.admission).acquire_owned() => permit.ok(),
            () = ctx.cancellation.cancelled() => None,
        }
    }

    /// The `trace` mode: exact single-trace fetch via the engine's
    /// cross-source `trace_by_id`.
    ///
    /// Ignores the ENVELOPE window; assembly bounds live in the `trace`
    /// sub-object. Absent bounds capture the FULL range, remote history
    /// included (so the lookup fails as too large once that history
    /// exceeds the download cache) — a trace is an exact object whose spans
    /// straddle files (WAL rotation is content-agnostic), and only the
    /// caller knows how much slack its anchor deserves. Present bounds
    /// prune the capture file-granularly
    /// (a file overlapping the bounds is probed whole). Either way the
    /// response DECLARES the range used (`coverage`) — spans beyond it
    /// are unknown, never silently dropped: the declaration is the
    /// honesty. An absent id is a Complete empty trace, not an error.
    async fn trace(
        &self,
        ctx: &FunctionCallContext,
        params: &TraceParams,
        tenant: Option<&str>,
    ) -> netdata_plugin_error::Result<OtelTracesResponse> {
        let trace_id = parse_trace_id(&params.id)
            .map_err(|e| handler_err(format!("invalid otel-traces request: {e}")))?;
        // Pre-capture twin of the engine's UnsetTraceId check (the
        // parser deliberately lets the sentinel through).
        if trace_id.is_unset() {
            return Err(handler_err(
                "invalid otel-traces request: the all-zero (unset) trace id cannot be looked up"
                    .to_string(),
            ));
        }
        let mut query = TraceQuery::new(trace_id);
        if let Some(cap) = params.span_cap {
            // Pre-capture twin of the engine's ZeroSpanCap check.
            if cap == 0 {
                return Err(handler_err(
                    "invalid otel-traces request: a zero span cap would return nothing; \
                     omit the cap or raise it"
                        .to_string(),
                ));
            }
            // The wire may TIGHTEN the engine's runaway-merge bound,
            // never loosen it — an oversized cap would defeat the
            // default's documented purpose (see DEFAULT_SPAN_CAP).
            if cap > sfsq::traces::DEFAULT_SPAN_CAP {
                return Err(handler_err(format!(
                    "invalid otel-traces request: 'span_cap' {cap} exceeds the maximum {}",
                    sfsq::traces::DEFAULT_SPAN_CAP
                )));
            }
            query = query.span_cap(cap);
        }

        let bounds = validate_trace_bounds(params.after, params.before)
            .map_err(|e| handler_err(format!("invalid otel-traces request: {e}")))?;
        let capture_range = bounds.unwrap_or(0..u32::MAX);
        let coverage = CoverageWire {
            after: capture_range.start,
            before: capture_range.end,
        };

        let tenant = TenantId::resolve_query(tenant);
        let permit = self.admit(ctx).await;
        // A cancelled capture returns NO sources; the empty vector flows
        // into the engine, which polls the same token up front and
        // reports the Cancelled partial — one consistent cancel path.
        // The capture sets the progress total; the engine ticks the done
        // counter once per source and the bridge's ticker renders it.
        let Capture { sources, pins } = self
            .supplier
            .capture(&tenant, capture_range, &ctx.cancellation, &ctx.progress)
            .await
            .map_err(capture_error)?;
        let done = ctx.progress.done_counter();
        let cancel = ctx.cancellation.clone();

        // Sync engine (maps + decompresses files) — off the runtime
        // thread. Engine request errors (unset id, zero cap) are clean
        // client errors; a rejected SOURCE SET is the supplier's
        // inconsistency (duplicate ids / overlapping WAL coverage), not
        // the client's — framed as internal so a debugger looks at the
        // right side. A panicked task is a handler failure.
        let data = match tokio::task::spawn_blocking(move || {
            let _pins = pins;
            let _permit = permit;
            trace_by_id(sources, query, cancel, done)
        })
        .await
        {
            Ok(Ok(data)) => data,
            Ok(Err(TraceRequestError::SourceSet(e))) => {
                return Err(handler_err(format!(
                    "otel-traces internal error: captured source set is inconsistent: {e}"
                )));
            }
            Ok(Err(e)) => {
                return Err(handler_err(format!("invalid otel-traces request: {e}")));
            }
            Err(e) => {
                return Err(handler_err(format!("otel-traces trace task failed: {e}")));
            }
        };

        Ok(OtelTracesResponse::Trace(Box::new(to_trace_result(
            &trace_id, data, coverage,
        ))))
    }
}

/// The wall clock as unix seconds, saturating at the u32 horizon (the
/// registry's second-granular time type).
fn unix_now_s() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(u64::from(u32::MAX)) as u32
}

impl OtelTracesHandler {
    /// The traces explorer: one capture over the aligned window, the
    /// engine off the async runtime, the answer in the Functions envelope.
    async fn explore(
        &self,
        ctx: &FunctionCallContext,
        params: &ExploreParams,
        tenant: Option<&str>,
    ) -> netdata_plugin_error::Result<OtelTracesResponse> {
        let (query, after, before) = to_explore_query(params, unix_now_s());
        let grid = query.grid;
        let tenant = TenantId::resolve_query(tenant);
        let permit = self.admit(ctx).await;
        let Capture { sources, pins } = self
            .supplier
            .capture(&tenant, after..before, &ctx.cancellation, &ctx.progress)
            .await
            .map_err(capture_error)?;
        if params.groups {
            // Groups read every source a second time, ticking once more each.
            let (_, total) = ctx.progress.load();
            ctx.progress.set_total(total + sources.len());
        }
        let done = ctx.progress.done_counter();
        let cancel = ctx.cancellation.clone();

        match tokio::task::spawn_blocking(move || {
            let _pins = pins;
            let _permit = permit;
            sfsq::traces::explore::explore(
                sources,
                query,
                sfsq::traces::explore::ExploreOptions::default(),
                cancel,
                done,
            )
        })
        .await
        {
            Ok(Ok(data)) => Ok(OtelTracesResponse::Explore(Box::new(to_explore_response(
                data, grid, after, before,
            )))),
            Ok(Err(sfsq::traces::explore::ExploreRequestError::SourceSet(e))) => {
                Err(handler_err(format!(
                    "otel-traces internal error: captured source set is inconsistent: {e}"
                )))
            }
            Ok(Err(e)) => Err(handler_err(format!("invalid otel-traces request: {e}"))),
            Err(e) => Err(handler_err(format!("otel-traces explore task failed: {e}"))),
        }
    }

    /// Value suggestions: one capture over the window, the engine off the
    /// async runtime.
    async fn values(
        &self,
        ctx: &FunctionCallContext,
        params: &ValuesParams,
        tenant: Option<&str>,
    ) -> netdata_plugin_error::Result<OtelTracesResponse> {
        let (query, after, before) = to_values_query(params, unix_now_s());
        let tenant = TenantId::resolve_query(tenant);
        let permit = self.admit(ctx).await;
        let Capture { sources, pins } = self
            .supplier
            .capture(&tenant, after..before, &ctx.cancellation, &ctx.progress)
            .await
            .map_err(capture_error)?;
        let done = ctx.progress.done_counter();
        let cancel = ctx.cancellation.clone();
        let field = params.field.clone();

        match tokio::task::spawn_blocking(move || {
            let _pins = pins;
            let _permit = permit;
            sfsq::traces::explore::field_values(
                sources,
                query,
                sfsq::traces::explore::ExploreOptions::default(),
                cancel,
                done,
            )
        })
        .await
        {
            Ok(Ok(data)) => Ok(OtelTracesResponse::Values(to_values_response(data, field))),
            Ok(Err(sfsq::traces::explore::ExploreRequestError::SourceSet(e))) => Err(handler_err(
                format!("otel-traces internal error: captured source set is inconsistent: {e}"),
            )),
            Ok(Err(e)) => Err(handler_err(format!("invalid otel-traces request: {e}"))),
            Err(e) => Err(handler_err(format!("otel-traces values task failed: {e}"))),
        }
    }
}

#[async_trait]
impl FunctionHandler for OtelTracesHandler {
    type Request = OtelTracesRequest;
    type Response = OtelTracesResponse;

    async fn on_call(
        &self,
        ctx: FunctionCallContext,
        req: Self::Request,
    ) -> netdata_plugin_error::Result<Self::Response> {
        let tenant = req.tenant.as_deref();
        match &req.mode {
            TracesMode::Info => Ok(OtelTracesResponse::Info(Box::default())),
            TracesMode::Explore(params) => self.explore(&ctx, params, tenant).await,
            TracesMode::Values(params) => self.values(&ctx, params, tenant).await,
            TracesMode::Trace(params) => self.trace(&ctx, params, tenant).await,
        }
    }

    fn declaration(&self) -> FunctionDeclaration {
        let mut d = FunctionDeclaration::new("otel-traces", "Query OpenTelemetry traces");
        d.global = true;
        d.tags = Some("traces".to_string());
        d.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        d
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod remote_tests;

#[cfg(test)]
mod oracle_tier2_tests;

#[cfg(test)]
mod schema_tests;
