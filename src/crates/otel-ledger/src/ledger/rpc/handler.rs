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

use sfsq::logs::{SfstCandidate, Source, WalTail, run};

use super::adapter::{to_result, window_secs};
use super::wire::{InfoResponse, LogsResult, OtelLogsRequest, OtelLogsResponse};
use crate::chunk::{ChunkCache, chunk_boundaries, tail_start};
use crate::registry::{TenantRegistries, WalDesc};

pub(crate) struct OtelLogsHandler {
    registries: Arc<RwLock<TenantRegistries>>,
    /// Shared with the ledger (which drops a WAL's chunks on rotation).
    chunk_cache: Arc<ChunkCache>,
    /// Minimum records per chunk when indexing an active WAL's prefix.
    min_entries: u64,
}

impl OtelLogsHandler {
    pub(crate) fn new(
        registries: Arc<RwLock<TenantRegistries>>,
        chunk_cache: Arc<ChunkCache>,
        min_entries: u64,
    ) -> Self {
        Self {
            registries,
            chunk_cache,
            min_entries,
        }
    }

    /// Resolve one active-WAL descriptor into in-memory chunk SFST
    /// candidates plus the sub-chunk tail. Off the registry lock.
    ///
    /// Scans the durable prefix's frame headers, groups them into chunks
    /// at `min_entries`, and builds each missing chunk through the cache
    /// (singleflight). A chunk that fails to build or whose record count
    /// doesn't match the scan is logged and skipped — its data is absent
    /// from this response and reappears once it (or the whole WAL's
    /// SFST) is available, mirroring the engine's per-source degrade.
    async fn resolve_wal(&self, wal: WalDesc) -> (Vec<SfstCandidate>, Option<WalTail>) {
        let header = wal::HEADER_SIZE as u64;
        let scan_path = wal.path.clone();
        let valid_up_to = wal.valid_up_to;
        let frames = match tokio::task::spawn_blocking(move || {
            wal::scan_frame_boundaries(&scan_path, header, valid_up_to)
        })
        .await
        {
            Ok(Ok(frames)) => frames,
            Ok(Err(e)) => {
                tracing::warn!(seq = wal.seq, "WAL boundary scan failed: {e}");
                return (Vec::new(), None);
            }
            Err(e) => {
                tracing::warn!(seq = wal.seq, "WAL boundary scan task failed: {e}");
                return (Vec::new(), None);
            }
        };

        let chunks = chunk_boundaries(&frames, header, self.min_entries);
        let mut candidates = Vec::new();
        for chunk in &chunks {
            let seq = wal.seq;
            let path = wal.path.clone();
            let (start, end, expected) = (chunk.start, chunk.end, chunk.entry_count);
            // The build future: index the byte range on a blocking
            // thread and cross-check the record count (the truncation
            // check open_range defers). Runs once per (seq, index) under
            // singleflight; skipped entirely on a cache hit.
            let init = async move {
                match tokio::task::spawn_blocking(move || sfst::index_range(&path, start, end))
                    .await
                {
                    Ok(Ok((summary, bytes))) => {
                        if u64::from(summary.total_logs) != expected {
                            Err(format!(
                                "chunk record count {} != expected {expected}",
                                summary.total_logs
                            ))
                        } else {
                            Ok(Arc::new(bytes))
                        }
                    }
                    Ok(Err(e)) => Err(format!("index_range: {e}")),
                    Err(e) => Err(format!("build task: {e}")),
                }
            };

            match self.chunk_cache.get_or_build(seq, chunk.index, init).await {
                Ok(bytes) => match sfst::IndexReader::open(&bytes[..]) {
                    Ok(reader) => candidates.push(SfstCandidate {
                        summary: reader.summary().clone(),
                        seq,
                        source: Source::Memory(bytes),
                    }),
                    Err(e) => {
                        tracing::warn!(seq, index = chunk.index, "chunk parse failed: {e}")
                    }
                },
                Err(e) => tracing::warn!(seq, index = chunk.index, "chunk build skipped: {e}"),
            }
        }

        let tail = WalTail {
            seq: wal.seq,
            path: wal.path,
            start: tail_start(&chunks, header),
            end: wal.valid_up_to,
        };
        (candidates, Some(tail))
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
        // A malformed free-text `query` regex is a clean request error.
        let query = req.into_query().map_err(|e| {
            netdata_plugin_error::NetdataPluginError::FunctionHandler {
                message: format!("invalid query: {e}"),
            }
        })?;
        let time_range = window_secs(&query.grid());
        // Snapshot the candidate set under a brief read lock: on-disk
        // SFSTs plus the unindexed WALs overlapping the window, owned so
        // the lock drops before any I/O. `valid_up_to` is captured here,
        // once — every chunk and tail derives from this single value, so
        // the whole query sees one consistent durable prefix even as
        // ingestion advances it.
        let (mut sfst_candidates, wal_descs) = {
            let guard = self.registries.read().await;
            let q = file_registry::Query {
                time_range: time_range.clone(),
                stream: None,
            };
            guard.query_snapshot(&q)
        };

        // Resolve each WAL into in-memory chunk SFSTs + a tail (off the
        // lock; chunk builds are singleflighted through the cache).
        let mut wal_tails: Vec<WalTail> = Vec::new();
        for wal in wal_descs {
            let (chunks, tail) = self.resolve_wal(wal).await;
            sfst_candidates.extend(chunks);
            if let Some(tail) = tail {
                wal_tails.push(tail);
            }
        }

        let (after, before) = (time_range.start, time_range.end);
        if sfst_candidates.is_empty() && wal_tails.is_empty() {
            return Ok(OtelLogsResponse::Logs(LogsResult::empty_stub(
                after, before, last,
            )));
        }

        // The query is synchronous and CPU/IO-bound (opens + decompresses
        // SFSTs, row-scans the tails); run it and shape the neutral
        // result into the wire envelope off the runtime thread.
        let result = match tokio::task::spawn_blocking(move || {
            to_result(run(sfst_candidates, wal_tails, query), last)
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
