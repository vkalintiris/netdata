//! `OtelLogsHandler` — typed `FunctionHandler` implementation.
//!
//! Holds a shared, read-only handle to the tenant registries. The
//! run-loop's mutators take brief write locks; this handler takes a
//! read lock just long enough to identify the most-recent SFST file,
//! then drops it before doing any I/O.
//!
//! Step 3 of the otel-logs rewrite (MVP integration): non-info requests
//! open the single most-recent SFST file across all tenants, run
//! [`sfst::IndexReader::facets`] + [`sfst::IndexReader::timeline`], and
//! return a populated [`LogsResponse`]. The log-row table (`data` /
//! `columns`) stays empty — log materialization is a later phase.
//! Multi-file merging is also a later phase; for now the response
//! reflects what the freshest file alone has.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use bridge::function::{FunctionCallContext, FunctionHandler};
use netdata_plugin_protocol::FunctionDeclaration;
use netdata_plugin_types::HttpAccess;
use tokio::sync::RwLock;

use super::adapter::{
    available_histograms_from_fields, facet_from_sfst, histogram_from_sfst,
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

        // Take the read lock just long enough to find the freshest file.
        // The lock is released before any file I/O happens.
        let target = {
            let guard = self.registries.read().await;
            guard.most_recent_sfst()
        };

        let Some((summary, path)) = target else {
            // No SFST files exist yet — honest empty result.
            return Ok(OtelLogsResponse::Logs(LogsResponse::empty_stub(
                req.after, req.before, req.last,
            )));
        };

        // Honest empty result when the request window doesn't overlap
        // the freshest file (decision (a) from step 0). The UI renders
        // an empty chart aligned to the user's selection.
        if !window_overlaps_file(req.after, req.before, &summary) {
            return Ok(OtelLogsResponse::Logs(LogsResponse::empty_stub(
                req.after, req.before, req.last,
            )));
        }

        let response = match build_logs_response(&path, &req) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "otel-logs query failed for {}: {e}",
                    path.display()
                );
                LogsResponse::empty_stub(req.after, req.before, req.last)
            }
        };

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

/// Open the SFST file, run the facets + timeline queries, and assemble
/// the wire envelope. Pure sync — no awaits — so the caller does the
/// `tokio` orchestration above.
fn build_logs_response(
    path: &Path,
    req: &OtelLogsRequest,
) -> Result<LogsResponse, sfst::Error> {
    let data = std::fs::read(path)?;
    let reader = sfst::IndexReader::open(&data)?;
    let field_table: Vec<sfst::FieldEntry> = reader.field_table().to_vec();

    let filter = build_filter(&req.selections);
    let histogram_field = pick_histogram_field(&req.histogram);
    let facet_fields = pick_facet_fields(&req.facets, &field_table);
    let bucket_width_ns = pick_bucket_width_ns(req.after, req.before);

    // `matched` reflects total filter-matching logs — the legacy
    // semantic. Computed independently of the histogram so it doesn't
    // depend on whether logs have the histogram dimension field set
    // (which would under-count when the dimension is sparse in the
    // stream).
    let matched: usize = reader.evaluate(&filter)?.len() as usize;

    let sfst_facets = reader.facets(&facet_fields, &filter)?;
    let sfst_timeline = reader.timeline(&histogram_field, &filter, bucket_width_ns)?;

    let wire_facets = sfst_facets
        .iter()
        .enumerate()
        .map(|(i, f)| facet_from_sfst(i, f))
        .collect();
    let wire_histogram = histogram_from_sfst(&histogram_field, &sfst_timeline);
    let wire_available = available_histograms_from_fields(&field_table);

    Ok(LogsResponse {
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
    })
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

/// True iff the request's `[after, before)` window shares any second
/// with the file's `[min, max]` second range. An empty window
/// (`after == 0 && before == 0`, the UI's "no time bound" form) is
/// treated as "match everything" so first-load requests still see
/// data.
fn window_overlaps_file(after: u32, before: u32, summary: &sfst::Summary) -> bool {
    if after == 0 && before == 0 {
        return true;
    }
    if after >= before {
        return false;
    }
    summary.max_timestamp_s >= after && summary.min_timestamp_s < before
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
