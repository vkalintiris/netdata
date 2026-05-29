//! `OtelLogsHandler` — typed `FunctionHandler` implementation.
//!
//! A thin adapter over the [`sfsq::logs`] query engine. Holds a
//! shared, read-only handle to the tenant registries: the run-loop's
//! mutators take brief write locks; this handler takes a read lock
//! just long enough to enumerate the SFST candidates whose time range
//! overlaps the request window, then drops it and runs the (sync)
//! query off the runtime thread via `spawn_blocking`.
//!
//! All query logic — filter, facets, histogram, pagination, row
//! materialization, and the wire envelope — lives in [`sfsq::logs`].
//! What stays here is the netdata-plugin glue: the `FunctionHandler`
//! impl, the capability declaration, and the rt-level args→payload
//! shim.

use std::sync::Arc;

use async_trait::async_trait;
use bridge::function::{FunctionCallContext, FunctionHandler};
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_types::HttpAccess;
use tokio::sync::RwLock;

use sfsq::logs::{
    InfoResponse, LogsResult, LogsRequest, LogsResponse, effective_window, run,
};

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
    type Request = LogsRequest;
    type Response = LogsResponse;

    async fn on_call(
        &self,
        _ctx: FunctionCallContext,
        mut req: Self::Request,
    ) -> netdata_plugin_error::Result<Self::Response> {
        if req.info {
            return Ok(LogsResponse::Info(InfoResponse::default()));
        }

        // Resolve the (possibly defaulted) request window, then
        // enumerate the SFST candidates overlapping it under a brief
        // read lock — dropped before any I/O.
        (req.after, req.before) = effective_window(req.after, req.before);
        let candidates = {
            let guard = self.registries.read().await;
            let query = file_registry::Query {
                time_range: req.after..req.before,
                stream: None,
            };
            guard.sfst_candidates(&query)
        };

        // The query is synchronous and CPU/IO-bound (opens + decompresses
        // SFSTs); run it off the runtime thread.
        let (after, before, last) = (req.after, req.before, req.last);
        let response = tokio::task::spawn_blocking(move || run(candidates, req))
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("otel-logs blocking task failed: {e}");
                LogsResult::empty_stub(after, before, last)
            });

        Ok(LogsResponse::Logs(response))
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
