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
use tokio::sync::RwLock;

use super::adapter::{
    NS_PER_S, available_histograms_from_fields, facet_from_sfst, histogram_from_sfst,
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

/// Aim for at least this many time buckets across the request
/// window when picking from [`VALID_BUCKET_WIDTHS_S`]. With the
/// curated widths and a 15-minute window this yields 15-second
/// buckets (60 of them).
const TARGET_BUCKETS: u32 = 60;

/// "Nice" bucket widths in seconds. Ported from the legacy
/// systemd-journal plugin's `calculate_bucket_duration` to keep
/// histograms anchored to wall-clock-friendly intervals (1s, 2s,
/// 5s, 10s, 15s, 30s, 1m, 5m, …). [`bucket_width_for_span_s`] picks
/// the largest entry that produces at least [`TARGET_BUCKETS`]
/// buckets across the span, so the chart density is stable as the
/// requested window scales.
const VALID_BUCKET_WIDTHS_S: &[u32] = &[
    1, 2, 5, 10, 15, 30, // seconds
    60, 120, 180, 300, 600, 900, 1800, // minutes
    3600, 7200, 21600, 28800, 43200, // hours
    86400, 172800, 259200, 432000, 604800, 1209600, 2592000, // days
];

/// Default request window in seconds when the caller doesn't specify
/// `after`/`before`. Matches the cloud-frontend default time range.
const DEFAULT_WINDOW_SECS: u32 = 15 * 60;

/// Default facet field when the request doesn't specify any. The UI
/// sends an empty facet list on first load, so we can't infer which
/// fields the user cares about; rather than auto-curate a set (which
/// can't be done well across multiple SFSTs — a field's cardinality
/// composes unpredictably across files), we surface only this one.
/// Always `severity_text` — the OTel canonical log-level field, same
/// rationale as [`DEFAULT_HISTOGRAM_FIELD`]. Users add more via the
/// UI's "+ Add Filter Field" control, which sends explicit
/// `req.facets`.
const DEFAULT_FACET_FIELD: &str = "severity_text";

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
        mut req: Self::Request,
    ) -> netdata_plugin_error::Result<Self::Response> {
        if req.info {
            return Ok(OtelLogsResponse::Info(InfoResponse::default()));
        }

        // Patch the request's window in place; everything downstream
        // reads the resolved window off `req`. See `effective_window`.
        (req.after, req.before) = effective_window(req.after, req.before);

        let candidates = {
            let guard = self.registries.read().await;
            let query = file_registry::Query {
                time_range: req.after..req.before,
                stream: None,
            };
            guard.sfst_candidates(&query)
        };

        // Snap the window outward to multiples of a "nice" bucket
        // width, in place. This stabilises the histogram x-axis across
        // the UI's per-second polling: successive polls within the
        // same bucket-width slot all see identical `[after, before)`,
        // so the chart only shifts when crossing a real boundary. The
        // width is chosen from a curated list (1s/2s/5s/10s/15s/30s/
        // 1m/…) so bars align to wall-clock-friendly intervals.
        let span_s = req.before.saturating_sub(req.after);
        let bucket_width_s = bucket_width_for_span_s(span_s);
        (req.after, req.before) = align_window(req.after, req.before, bucket_width_s);

        if candidates.is_empty() {
            // No SFST files exist at all — honest empty envelope
            // aligned to the (effective) window.
            return Ok(OtelLogsResponse::Logs(LogsResponse::empty_stub(
                req.after, req.before, req.last,
            )));
        }

        // Bucket grid derived directly from the aligned window —
        // `bucket_width_s` divides `(before - after)` exactly by
        // construction, so no `div_ceil` is needed. All per-file
        // timelines share this grid and are directly mergeable.
        let grid = sfst::Grid::new(
            (req.after as i64) * NS_PER_S,
            (bucket_width_s as i64) * NS_PER_S,
            ((req.before - req.after) / bucket_width_s) as usize,
        );

        // Capture primitives needed for the error fallback before
        // `req` moves into the blocking task.
        let req_after = req.after;
        let req_before = req.before;
        let req_last = req.last;

        let response = tokio::task::spawn_blocking(move || {
            build_merged_logs_response(candidates, req, grid)
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
    grid: sfst::Grid,
) -> LogsResponse {
    let filter = build_filter(&req.selections);
    let histogram_field = pick_histogram_field(&req.histogram);

    // Read every candidate's bytes, pairing each buffer with its
    // path. The `IndexReader` borrows from the bytes, so the owned
    // buffers must outlive the readers — we hold them in `opened` for
    // the duration of the per-file work. Files that fail to read are
    // logged and skipped.
    let mut opened: Vec<(Vec<u8>, &PathBuf)> = Vec::with_capacity(candidates.len());
    for (_summary, path) in &candidates {
        match std::fs::read(path) {
            Ok(bytes) => opened.push((bytes, path)),
            Err(e) => tracing::warn!("otel-logs: failed to read {}: {e}", path.display()),
        }
    }

    // Open readers + collect field tables; skip on open failure. The
    // reader, its path, and its field table travel together by
    // position across the three vecs.
    let mut readers: Vec<sfst::IndexReader<'_>> = Vec::new();
    let mut reader_paths: Vec<&PathBuf> = Vec::new();
    let mut field_tables: Vec<Vec<sfst::FieldEntry>> = Vec::new();
    for (bytes, path) in &opened {
        match sfst::IndexReader::open(bytes) {
            Ok(reader) => {
                field_tables.push(reader.field_table().to_vec());
                reader_paths.push(path);
                readers.push(reader);
            }
            Err(e) => {
                tracing::warn!("otel-logs: failed to open {}: {e}", path.display());
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

    // The request window in ns. Every per-file query — matched,
    // facets, and the histogram grid — clips to this same window, so
    // their counts describe the same set of logs and agree.
    let window_ns = (req.after as i64) * NS_PER_S..(req.before as i64) * NS_PER_S;

    let mut matched_total: u64 = 0;
    let mut per_file_facets: Vec<Vec<sfst::FacetResult>> = Vec::new();
    let mut per_file_timelines: Vec<sfst::Timeline> = Vec::new();

    for (reader, path) in readers.iter().zip(reader_paths.iter()) {
        // matched: filter-matching logs restricted to the request
        // window.
        match per_file_matched(reader, &filter, window_ns.clone()) {
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
        match reader.facets(&file_facet_fields, &filter, window_ns.clone()) {
            Ok(facets) => per_file_facets.push(facets),
            Err(e) => tracing::warn!(
                "otel-logs: facets failed for {}: {e}",
                path.display()
            ),
        }

        // Histogram: every file contributes a timeline on the shared
        // grid. A file that lacks the histogram field yields a
        // dimensionless timeline whose matching logs all land in
        // `unset`, so the merged histogram total stays equal to
        // `matched`. (`timeline` only errors here if the picked field
        // is high-card, which `available_histograms` never offers.)
        match reader.timeline(&histogram_field, &filter, grid) {
            Ok(t) => per_file_timelines.push(t),
            Err(e) => tracing::warn!(
                "otel-logs: timeline failed for {}: {e}",
                path.display()
            ),
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
            grid,
            dimensions: Vec::new(),
            buckets: vec![Vec::new(); grid.num_buckets],
            unset: vec![0u64; grid.num_buckets],
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
/// full range; intersect with the file's window range bitmap (the
/// same primitive `facets` uses) to clip outside-window logs.
fn per_file_matched(
    reader: &sfst::IndexReader<'_>,
    filter: &sfst::Filter,
    window_ns: std::ops::Range<i64>,
) -> Result<u64, sfst::Error> {
    let bm = reader.evaluate(filter)?;
    let range = reader.range_bitmap(window_ns)?;
    Ok((bm & &range).len())
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

/// Pick the facet field set. With no explicit request — the UI's
/// first-load behavior — return just [`DEFAULT_FACET_FIELD`]; we don't
/// try to auto-curate a wider set (see that constant). Explicit
/// `requested` selections are honored as-is, modulo high-card /
/// unknown fields (those would error or surface no options).
fn pick_facet_fields(requested: &[String], fields: &[sfst::FieldEntry]) -> Vec<String> {
    if requested.is_empty() {
        return vec![DEFAULT_FACET_FIELD.to_string()];
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

/// Pick a "nice" bucket width (seconds) for a given span. Walks
/// [`VALID_BUCKET_WIDTHS_S`] from largest to smallest and returns
/// the first one that produces at least [`TARGET_BUCKETS`] buckets.
/// Falls back to `1` for spans too short to satisfy the heuristic.
fn bucket_width_for_span_s(span_s: u32) -> u32 {
    VALID_BUCKET_WIDTHS_S
        .iter()
        .rev()
        .find(|&&w| span_s / w >= TARGET_BUCKETS)
        .copied()
        .unwrap_or(1)
}

/// Round `[after, before)` outward to multiples of `width_s`. The
/// returned bounds are still in `(seconds since epoch)`, but
/// `after` is floored and `before` is ceiled, so the histogram grid
/// anchors to absolute wall-clock boundaries (e.g. 15s buckets snap
/// to `t % 15 == 0`).
///
/// This is what keeps the chart x-axis stable across the UI's
/// per-second polling: requests within the same bucket-width slot
/// align to the same grid, so the chart only shifts when crossing a
/// real boundary.
fn align_window(after: u32, before: u32, width_s: u32) -> (u32, u32) {
    let aligned_after = (after / width_s) * width_s;
    let aligned_before = before.div_ceil(width_s) * width_s;
    (aligned_after, aligned_before)
}

/// Resolve a request's `[after, before)` to a usable time window.
/// Returns the inputs verbatim when they form a valid non-empty
/// range; falls back to the last [`DEFAULT_WINDOW_SECS`] computed
/// from system time otherwise — the legacy "no time bound" form
/// `(0, 0)` and any inverted / zero-width window.
///
/// A defaulted window with no overlapping data returns the empty
/// envelope just like an explicit one; the handler does not
/// second-guess the caller by reaching for the most-recent file.
fn effective_window(after: u32, before: u32) -> (u32, u32) {
    let malformed = (after == 0 && before == 0) || after >= before;
    if !malformed {
        return (after, before);
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(u32::MAX);
    (now.saturating_sub(DEFAULT_WINDOW_SECS), now)
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
