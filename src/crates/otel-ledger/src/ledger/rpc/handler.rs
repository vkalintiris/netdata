//! `OtelLogsHandler` — typed `FunctionHandler` implementation.
//!
//! Holds a shared, read-only handle to the tenant registries. The
//! run-loop's mutators take brief write locks; this handler takes a
//! read lock just long enough to enumerate the SFST candidates whose
//! time range overlaps the request window, then drops it before doing
//! any I/O.
//!
//! Non-info requests open every overlapping SFST across all tenants,
//! run [`sfst::IndexReader::evaluate`] + [`sfst::IndexReader::facets`]
//! + [`sfst::IndexReader::timeline`] per file with a shared
//! request-aligned bucket grid, then merge the per-file results into
//! a single [`LogsResponse`]. WAL and remote-catalog candidates are
//! out of scope. The log-row table (`data` / `columns`) stays empty —
//! log materialization is a later phase.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use bridge::function::{FunctionCallContext, FunctionHandler};
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_types::HttpAccess;
use roaring::RoaringBitmap;
use tokio::sync::RwLock;

use super::adapter::{
    available_histograms_from_fields, facet_from_sfst, histogram_from_sfst,
    merge_facet_results, merge_timelines, union_field_tables,
};
use super::types::{ACCEPTED_PARAMS, InfoResponse, OtelLogsRequest, OtelLogsResponse};
use super::wire::{Items, LogsResponse, Pagination, Version};
use crate::registry::TenantRegistries;

/// Default histogram dimension when the request doesn't specify one.
/// Always `severity_text` — it's the OTel canonical log-level field,
/// and what makes a meaningful chart is the producer's responsibility
/// (set it, populate it with varied values). The UI exposes the full
/// `available_histograms` list for users to pick something else.
const DEFAULT_HISTOGRAM_FIELD: &str = "severity_text";

/// Aim for this many time buckets across the request window. Drives
/// `bucket_width_ns` when the caller doesn't specify one. At the UI
/// default 15-minute window this yields 15-second buckets.
const TARGET_BUCKETS: u32 = 60;

/// Default request window in seconds when the caller doesn't specify
/// `after`/`before`. Matches the cloud-frontend default time range.
const DEFAULT_WINDOW_SECS: u32 = 15 * 60;

/// Nanoseconds per second — used for `u32` seconds → `i64` ns
/// conversions when aligning bucket grids to the request window.
const NS_PER_S: i64 = 1_000_000_000;

/// Maximum value-cardinality per facet in the default facet set.
/// Fields with more distinct values than this aren't useful as
/// filter facets (the user can't reasonably pick from them) and
/// inflate the response. Explicit `req.facets` selections bypass
/// this cap.
const MAX_FACET_OPTIONS_PER_FIELD: u32 = 30;

/// Maximum facets in the default facet set. Caps the response when
/// an ingestor flattens array attributes (e.g. cert SAN arrays as
/// `log.body.data.leaf_cert.all_domains.{N}` producing one field per
/// index), which otherwise yields hundreds of facet fields.
const MAX_FACET_FIELDS: usize = 16;

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

        // The candidate planner filters by time-range overlap with
        // each file's summary. `stream: None` because we don't yet
        // filter by service identity at the planner level.
        let query = file_registry::Query {
            time_range: req.after..req.before,
            stream: None,
        };

        // Take the read lock just long enough to enumerate matching
        // SFSTs; release before any file I/O.
        let candidates = {
            let guard = self.registries.read().await;
            guard.sfst_candidates(&query)
        };

        if candidates.is_empty() {
            // No overlapping SFSTs (cold start, or window outside
            // any file) — honest empty envelope aligned to the
            // request window.
            return Ok(OtelLogsResponse::Logs(LogsResponse::empty_stub(
                req.after, req.before, req.last,
            )));
        }

        // Request-aligned bucket grid — anchored at `after`, sized
        // to span `[after, before)`. All per-file timelines are
        // computed against this grid so they're directly mergeable.
        let bucket_width_ns = pick_bucket_width_ns(req.after, req.before);
        let bucket_start_ns = (req.after as i64) * NS_PER_S;
        let num_buckets = num_buckets_for_window(req.after, req.before, bucket_width_ns);

        // Capture primitives needed for the error fallback before
        // `req` moves into the blocking task.
        let req_after = req.after;
        let req_before = req.before;
        let req_last = req.last;

        let response = tokio::task::spawn_blocking(move || {
            build_merged_logs_response(
                candidates,
                req,
                bucket_start_ns,
                bucket_width_ns,
                num_buckets,
            )
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("otel-logs blocking task failed: {e}");
            LogsResponse::empty_stub(req_after, req_before, req_last)
        });

        Ok(OtelLogsResponse::Logs(response))
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

/// Open every SFST candidate, run the three queries per file against
/// a shared request-aligned bucket grid, merge the per-file results,
/// and assemble the wire envelope. Pure sync — no awaits — so the
/// caller wraps this in `tokio::task::spawn_blocking`.
///
/// Per-file errors (corrupt file, missing field, etc.) are logged
/// and that file is skipped — other files still contribute to the
/// response. If *every* file errors we fall through to an empty
/// stub.
fn build_merged_logs_response(
    candidates: Vec<(sfst::Summary, PathBuf)>,
    req: OtelLogsRequest,
    bucket_start_ns: i64,
    bucket_width_ns: i64,
    num_buckets: usize,
) -> LogsResponse {
    let filter = build_filter(&req.selections);
    let histogram_field = pick_histogram_field(&req.histogram);

    // Open every candidate; on per-file error, log + skip. The
    // `IndexReader` borrows from the file's bytes, so we hold the
    // owned `Vec<u8>` alongside the reader for the duration of the
    // per-file work.
    let mut file_bytes: Vec<Vec<u8>> = Vec::with_capacity(candidates.len());
    let mut field_tables: Vec<Vec<sfst::FieldEntry>> = Vec::new();
    let mut readable_indices: Vec<usize> = Vec::new();

    for (i, (_summary, path)) in candidates.iter().enumerate() {
        match std::fs::read(path) {
            Ok(bytes) => {
                file_bytes.push(bytes);
                readable_indices.push(i);
            }
            Err(e) => {
                tracing::warn!("otel-logs: failed to read {}: {e}", path.display());
            }
        }
    }

    // Open readers + collect field tables; skip on open failure.
    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut reader_paths: Vec<&PathBuf> = Vec::new();
    for (slot, &cand_i) in readable_indices.iter().enumerate() {
        match sfst::IndexReader::open(&file_bytes[slot]) {
            Ok(reader) => {
                field_tables.push(reader.field_table().to_vec());
                reader_paths.push(&candidates[cand_i].1);
                readers.push(reader);
            }
            Err(e) => {
                tracing::warn!(
                    "otel-logs: failed to open {}: {e}",
                    candidates[cand_i].1.display()
                );
            }
        }
    }

    if readers.is_empty() {
        return LogsResponse::empty_stub(req.after, req.before, req.last);
    }

    // Picked facet field set against the unioned table — gives the
    // UI a consistent sidebar across files.
    let table_refs: Vec<&[sfst::FieldEntry]> =
        field_tables.iter().map(|t| t.as_slice()).collect();
    let unioned = union_field_tables(&table_refs);
    let facet_fields = pick_facet_fields(&req.facets, &unioned);

    let mut matched_total: u64 = 0;
    let mut per_file_facets: Vec<Vec<sfst::FacetResult>> = Vec::new();
    let mut per_file_timelines: Vec<sfst::Timeline> = Vec::new();

    for (reader, path) in readers.iter().zip(reader_paths.iter()) {
        // matched: filter-matching logs restricted to the request
        // window. `evaluate` returns positions across the file's
        // full range; intersect with the per-file range bitmap built
        // from the file's timestamps.
        match per_file_matched(reader, &filter, req.after, req.before) {
            Ok(m) => matched_total += m,
            Err(e) => tracing::warn!(
                "otel-logs: matched count failed for {}: {e}",
                path.display()
            ),
        }

        // Facets: filter the picked set to fields that exist in
        // this file. Unknown fields would make `facets()` error and
        // cost us the whole file.
        let file_facet_fields: Vec<String> = facet_fields
            .iter()
            .filter(|name| {
                reader
                    .field_table()
                    .iter()
                    .any(|f| f.name == **name)
            })
            .cloned()
            .collect();
        match reader.facets(&file_facet_fields, &filter) {
            Ok(facets) => per_file_facets.push(facets),
            Err(e) => tracing::warn!(
                "otel-logs: facets failed for {}: {e}",
                path.display()
            ),
        }

        // Histogram: a file that lacks the histogram field
        // contributes neither dimensions nor unset. (Minor:
        // filter-matching logs in this file don't reach `unset` in
        // the merged timeline. Acceptable for MVP — see the plan's
        // verification section.)
        let has_histogram_field = reader
            .field_table()
            .iter()
            .any(|f| f.name == histogram_field);
        if has_histogram_field {
            match reader.timeline(
                &histogram_field,
                &filter,
                bucket_start_ns,
                bucket_width_ns,
                num_buckets,
            ) {
                Ok(t) => per_file_timelines.push(t),
                Err(e) => tracing::warn!(
                    "otel-logs: timeline failed for {}: {e}",
                    path.display()
                ),
            }
        }
    }

    let merged_facets = merge_facet_results(per_file_facets);
    let wire_facets = merged_facets
        .iter()
        .enumerate()
        .map(|(i, f)| facet_from_sfst(i, f))
        .collect();

    // If no file contributed a timeline (histogram field absent
    // everywhere, or all timelines errored), synthesize an empty one
    // aligned to the grid so the wire shape stays valid.
    let merged_timeline =
        merge_timelines(per_file_timelines).unwrap_or_else(|| sfst::Timeline {
            bucket_start_ns,
            bucket_width_ns,
            dimensions: Vec::new(),
            buckets: vec![Vec::new(); num_buckets],
            unset: vec![0u64; num_buckets],
        });
    let wire_histogram = histogram_from_sfst(&histogram_field, &merged_timeline);
    let wire_available = available_histograms_from_fields(&unioned);

    let matched = matched_total as usize;

    LogsResponse {
        progress: 100,
        version: Version::default(),
        accepted_params: ACCEPTED_PARAMS.to_vec(),
        required_params: Vec::new(),
        facets: wire_facets,
        available_histograms: wire_available,
        histogram: wire_histogram,
        columns: serde_json::json!({}),
        data: serde_json::json!([]),
        default_charts: Vec::new(),
        items: Items {
            evaluated: matched,
            unsampled: 0,
            estimated: matched,
            matched,
            before: 0,
            after: 0,
            returned: 0,
            max_to_return: req.last,
        },
        show_ids: false,
        has_history: true,
        status: 200,
        response_type: String::from("table"),
        help: String::from("Query and visualize OpenTelemetry logs."),
        pagination: Pagination::default(),
    }
}

/// Per-file matched count: filter-matching logs restricted to the
/// request window. `evaluate` returns positions across the file's
/// full range; intersect with a range bitmap built from the file's
/// own timestamps to clip outside-window logs.
fn per_file_matched(
    reader: &sfst::IndexReader<'_>,
    filter: &sfst::Filter,
    after_s: u32,
    before_s: u32,
) -> Result<u64, sfst::Error> {
    let bm = reader.evaluate(filter)?;
    if after_s == 0 && before_s == 0 {
        return Ok(bm.len());
    }
    let timestamps = reader.load_timestamps()?;
    let after_ns = (after_s as i64) * NS_PER_S;
    let before_ns = (before_s as i64) * NS_PER_S;
    let lo = timestamps.partition_point(|&t| t < after_ns) as u32;
    let hi = timestamps.partition_point(|&t| t < before_ns) as u32;
    if lo >= hi {
        return Ok(0);
    }
    let mut range = RoaringBitmap::new();
    range.insert_range(lo..hi);
    Ok((bm & range).len())
}

/// Number of buckets in the request-aligned grid. Picks the smallest
/// count that covers `[after, before)` at the given width. Falls
/// back to [`TARGET_BUCKETS`] when `after`/`before` aren't set so a
/// no-window request still gets a sensible chart.
fn num_buckets_for_window(after: u32, before: u32, bucket_width_ns: i64) -> usize {
    if before <= after {
        return TARGET_BUCKETS as usize;
    }
    let span_ns = ((before - after) as i64) * NS_PER_S;
    ((span_ns as u64).div_ceil(bucket_width_ns as u64)) as usize
}

/// Translate the request's `selections` map into an [`sfst::Filter`].
/// Same shape, just a constructor walk: OR within field, AND across
/// fields (matches the UI's selection semantics).
fn build_filter(selections: &HashMap<String, Vec<String>>) -> sfst::Filter {
    let mut filter = sfst::Filter::new();
    for (field, values) in selections {
        for value in values {
            filter = filter.select(field.clone(), value.clone());
        }
    }
    filter
}

/// Pick the histogram field. Honors the request's `histogram` param
/// when set; otherwise returns [`DEFAULT_HISTOGRAM_FIELD`]. No
/// eligibility filtering — if the chosen field isn't in this SFST or
/// is high-cardinality, `sfst::timeline` will surface that as an
/// error and the handler falls back to the empty envelope. The UI
/// can then drive the user toward a different field via
/// `available_histograms`.
fn pick_histogram_field(requested: &str) -> String {
    if requested.is_empty() {
        DEFAULT_HISTOGRAM_FIELD.to_string()
    } else {
        requested.to_string()
    }
}

/// Pick the facet field set. When the caller didn't specify any,
/// default to low/mid-card fields whose cardinality is small enough
/// to be UI-usable, sorted by ascending cardinality so the most
/// filter-useful fields surface first; capped at [`MAX_FACET_FIELDS`]
/// total. Cardinality < 2 (single distinct value) is dropped — such
/// facets have no filter utility. Explicit `requested` is honored
/// as-is, modulo high-card / unknown fields (those would error or
/// surface no options).
fn pick_facet_fields(requested: &[String], fields: &[sfst::FieldEntry]) -> Vec<String> {
    if requested.is_empty() {
        let mut candidates: Vec<&sfst::FieldEntry> = fields
            .iter()
            .filter(|f| !is_high_card(f))
            .filter(|f| f.cardinality >= 2 && f.cardinality <= MAX_FACET_OPTIONS_PER_FIELD)
            .collect();
        // Stable sort by cardinality — ties keep field-table order
        // (low tier first, then alphabetical within tier).
        candidates.sort_by_key(|f| f.cardinality);
        return candidates
            .into_iter()
            .take(MAX_FACET_FIELDS)
            .map(|f| f.name.clone())
            .collect();
    }
    requested
        .iter()
        .filter(|name| {
            fields
                .iter()
                .any(|f| f.name == **name && !is_high_card(f))
        })
        .cloned()
        .collect()
}

/// Bucket width in nanoseconds aimed at [`TARGET_BUCKETS`] buckets
/// across the request window. Falls back to a 15-minute window if the
/// caller omits `after`/`before`. Minimum width is 1 second so a
/// narrow window doesn't produce sub-second buckets that the UI's
/// chart axis can't render distinctly.
fn pick_bucket_width_ns(after: u32, before: u32) -> i64 {
    let span_s = if before > after {
        before - after
    } else {
        DEFAULT_WINDOW_SECS
    };
    let width_s = (span_s / TARGET_BUCKETS).max(1);
    (width_s as i64) * 1_000_000_000
}

fn is_high_card(f: &sfst::FieldEntry) -> bool {
    matches!(f.tier, sfst::FieldTier::High)
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
