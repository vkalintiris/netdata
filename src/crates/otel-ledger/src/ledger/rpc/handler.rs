//! `OtelLogsHandler` — typed `FunctionHandler` implementation.
//!
//! A thin adapter over the [`sfsq::logs`] query engine. Holds a shared,
//! read-only handle to the tenant registries: the run-loop's mutators
//! take brief write locks; this handler takes a read lock just long
//! enough to enumerate the SFST candidates whose time range overlaps the
//! request window, then drops it and runs the (sync) query off the
//! runtime thread via `spawn_blocking`.
//!
//! The engine is wire-neutral: it consumes a [`sfsq::logs::LogsQuery`]
//! and produces a [`sfsq::logs::LogsData`]. The netdata function wire
//! shape — the request/response types and the response envelope — lives
//! in [`super::wire`], and the mapping to and from the engine in
//! [`super::adapter`]. What stays here is the netdata-plugin glue: the
//! `FunctionHandler` impl, the capability declaration, the rt-level
//! args→payload shim, and the lock/scheduling dance.

use std::sync::Arc;

use async_trait::async_trait;
use bridge::function::{FunctionCallContext, FunctionHandler};
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_types::HttpAccess;
use tokio::sync::RwLock;

use sfsq::logs::run;

use super::adapter::{to_result, window_secs};
use super::wire::{InfoResponse, LogsResult, OtelLogsRequest, OtelLogsResponse};
use crate::registry::TenantRegistries;

pub(crate) struct OtelLogsHandler {
    registries: Arc<RwLock<TenantRegistries>>,
}

impl OtelLogsHandler {
    pub(crate) fn new(registries: Arc<RwLock<TenantRegistries>>) -> Self {
        Self { registries }
    }
}

#[async_trait]
impl FunctionHandler for OtelLogsHandler {
    type Request = OtelLogsRequest;
    type Response = OtelLogsResponse;

    async fn on_call(
        &self,
        _ctx: FunctionCallContext,
        req: Self::Request,
    ) -> netdata_plugin_error::Result<Self::Response> {
        if req.info {
            return Ok(OtelLogsResponse::Info(InfoResponse::default()));
        }

        // Canonicalize the wire request into the neutral query (defaulting
        // + bucket alignment + grid), then enumerate the SFST candidates
        // overlapping the grid's window under a brief read lock — dropped
        // before any I/O.
        let last = req.last;
        let query = req.into_query();
        let time_range = window_secs(&query.grid);
        let candidates = {
            let guard = self.registries.read().await;
            let q = file_registry::Query {
                time_range: time_range.clone(),
                stream: None,
            };
            guard.sfst_candidates(&q)
        };

        let (after, before) = (time_range.start, time_range.end);
        if candidates.is_empty() {
            return Ok(OtelLogsResponse::Logs(LogsResult::empty_stub(
                after, before, last,
            )));
        }

        // The query is synchronous and CPU/IO-bound (opens + decompresses
        // SFSTs); run it and shape the neutral result into the wire
        // envelope off the runtime thread.
        let result = match tokio::task::spawn_blocking(move || {
            to_result(run(candidates, query), last)
        })
        .await
        {
            Ok(result) => result,
            Err(e) => {
                tracing::warn!("otel-logs blocking task failed: {e}");
                LogsResult::empty_stub(after, before, last)
            }
        };

        Ok(OtelLogsResponse::Logs(result))
    }

    fn declaration(&self) -> FunctionDeclaration {
        let mut d = FunctionDeclaration::new("otel-logs", "Query OpenTelemetry logs");
        d.global = true;
        d.tags = Some("logs".to_string());
        d.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        d
    }
}

/// Replicate the rt-level GET shim (`netdata-plugin/rt/src/lib.rs`):
/// when args carry `after:N` / `before:N` tokens, synthesize a JSON
/// payload with the parsed window plus an `info` flag determined by
/// whether the literal `info` token is in the args. Returns `None`
/// when no synthesis happened (no args, or the upstream rt shim
/// already produced a payload), in which case the caller falls back
/// to the original payload.
pub(super) fn patch_args_into_payload(args: &[String], payload: Option<&[u8]>) -> Option<Vec<u8>> {
    if args.is_empty() || payload.is_some() {
        return None;
    }

    let info = args.iter().any(|a| a == "info");
    let mut map = serde_json::Map::new();
    map.insert("info".into(), serde_json::json!(info));

    for arg in args {
        if let Some(rest) = arg.strip_prefix("after:") {
            if let Ok(v) = rest.parse::<u64>() {
                map.insert("after".into(), serde_json::json!(v));
            }
        } else if let Some(rest) = arg.strip_prefix("before:") {
            if let Ok(v) = rest.parse::<u64>() {
                map.insert("before".into(), serde_json::json!(v));
            }
        }
    }

    serde_json::to_vec(&serde_json::Value::Object(map)).ok()
}

#[cfg(test)]
mod tests;
