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

/// Histogram field defaults, tried in order if the request doesn't
/// specify one. Matches the OTel canonical names that typical
/// ingestors populate; falls through to the first eligible field in
/// the file if none of these are present.
const DEFAULT_HISTOGRAM_FIELDS: &[&str] = &["severity_text", "severity_number", "level"];

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

    /// Canonical function declaration. Used both by `FunctionHandler::declaration`
    /// and by the worker entry point in `lib.rs` to advertise the function
    /// to the supervisor before the full ledger is initialized.
    pub(crate) fn function_declaration() -> FunctionDeclaration {
        let mut d = FunctionDeclaration::new("otel-logs", "Query OpenTelemetry logs");
        d.global = true;
        d.tags = Some("logs".to_string());
        d.access =
            Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA);
        d
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

        let last = req.last.unwrap_or(200);

        // Take the read lock just long enough to find the freshest file.
        // The lock is released before any file I/O happens.
        let target = {
            let guard = self.registries.read().await;
            guard.most_recent_sfst()
        };

        let Some((summary, path)) = target else {
            // No SFST files exist yet — honest empty result.
            return Ok(OtelLogsResponse::Logs(LogsResponse::empty_stub(
                req.after, req.before, last,
            )));
        };

        // Honest empty result when the request window doesn't overlap
        // the freshest file (decision (a) from step 0). The UI renders
        // an empty chart aligned to the user's selection.
        if !window_overlaps_file(req.after, req.before, &summary) {
            return Ok(OtelLogsResponse::Logs(LogsResponse::empty_stub(
                req.after, req.before, last,
            )));
        }

        let response = match build_logs_response(&path, &req, last) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    "otel-logs query failed for {}: {e}",
                    path.display()
                );
                LogsResponse::empty_stub(req.after, req.before, last)
            }
        };

        Ok(OtelLogsResponse::Logs(response))
    }

    fn declaration(&self) -> FunctionDeclaration {
        Self::function_declaration()
    }
}

/// Open the SFST file, run the facets + timeline queries, and assemble
/// the wire envelope. Pure sync — no awaits — so the caller does the
/// `tokio` orchestration above.
fn build_logs_response(
    path: &Path,
    req: &OtelLogsRequest,
    last: usize,
) -> Result<LogsResponse, sfst::Error> {
    let data = std::fs::read(path)?;
    let reader = sfst::IndexReader::open(&data)?;
    let field_table: Vec<sfst::FieldEntry> = reader.field_table().to_vec();

    let filter = build_filter(&req.selections);
    let histogram_field = pick_histogram_field(&req.histogram, &field_table);
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
            max_to_return: last,
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

/// Pick the histogram field. Request → known OTel defaults → first
/// non-high-card field in the file. Returns an empty string only if
/// the file has no fields at all (which `sfst::timeline` will then
/// reject as `UnknownField`).
fn pick_histogram_field(requested: &str, fields: &[sfst::FieldEntry]) -> String {
    let is_eligible =
        |name: &str| fields.iter().any(|f| f.name == name && !is_high_card(f));

    if !requested.is_empty() && is_eligible(requested) {
        return requested.to_string();
    }
    for &candidate in DEFAULT_HISTOGRAM_FIELDS {
        if is_eligible(candidate) {
            return candidate.to_string();
        }
    }
    fields
        .iter()
        .find(|f| !is_high_card(f))
        .map(|f| f.name.clone())
        .unwrap_or_default()
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
mod tests {
    use super::*;
    use file_registry::{ByteSize, FileId, ServiceStream, TenantId};
    use fst_index::FstIndex;
    use serde_json::Value;
    use sfst::BitmapValue;
    use tokio_util::sync::CancellationToken;
    use uuid::Uuid;

    fn make_tenant_registries() -> TenantRegistries {
        TenantRegistries::new(
            tempfile::tempdir().unwrap().keep(),
            tempfile::tempdir().unwrap().keep(),
            tempfile::tempdir().unwrap().keep(),
        )
    }

    fn make_handler(tr: TenantRegistries) -> OtelLogsHandler {
        OtelLogsHandler::new(Arc::new(RwLock::new(tr)))
    }

    fn make_ctx(transaction: &str) -> FunctionCallContext {
        FunctionCallContext::new(
            transaction.to_string(),
            bridge::function::ProgressState::new(),
            CancellationToken::new(),
        )
    }

    fn bitmap_with(positions: &[u32], universe: u32) -> BitmapValue {
        let mut data = Vec::new();
        let desc =
            treight::Bitmap::from_sorted_iter(positions.iter().copied(), universe, &mut data);
        BitmapValue { desc, data }
    }

    /// Write a 6-log SFST to `path` with two low-card fields:
    ///
    /// - `severity_text`: `info` at 0/2/4, `error` at 1/3/5
    /// - `service`: `api` at 0/1/2, `worker` at 3/4/5
    ///
    /// Timestamps span 6 seconds starting at `min_s`.
    fn write_test_sfst(path: &std::path::Path, min_s: u32) {
        let primary_entries: Vec<(&str, BitmapValue)> = vec![
            ("service=api", bitmap_with(&[0, 1, 2], 6)),
            ("service=worker", bitmap_with(&[3, 4, 5], 6)),
            ("severity_text=error", bitmap_with(&[1, 3, 5], 6)),
            ("severity_text=info", bitmap_with(&[0, 2, 4], 6)),
        ];
        let primary: FstIndex<BitmapValue> = FstIndex::build(primary_entries).unwrap();

        let summary = sfst::Summary {
            min_timestamp_s: min_s,
            max_timestamp_s: min_s + 5,
            total_logs: 6,
            stream: ServiceStream::new("ns", "svc"),
        };
        let metadata = sfst::Metadata {
            histogram: sfst::Histogram {
                timestamps: vec![min_s],
                counts: vec![6],
            },
            id_ranges: sfst::IdRanges {
                low_end: sfst::KvId(4),
                mid_end: sfst::KvId(4),
                high_end: sfst::KvId(4),
            },
            fields: vec![
                sfst::FieldEntry {
                    name: "service".into(),
                    cardinality: 2,
                    tier: sfst::FieldTier::Low,
                },
                sfst::FieldEntry {
                    name: "severity_text".into(),
                    cardinality: 2,
                    tier: sfst::FieldTier::Low,
                },
            ],
        };
        let timestamps: Vec<i64> = (0..6)
            .map(|i| (min_s as i64) * 1_000_000_000 + i * 1_000_000_000)
            .collect();
        // FST key order is lex: KvId 0=service=api, 1=service=worker,
        // 2=severity_text=error, 3=severity_text=info.
        let stream_entries: Vec<Vec<sfst::KvId>> = vec![
            vec![sfst::KvId(0), sfst::KvId(3)], // pos 0: api, info
            vec![sfst::KvId(0), sfst::KvId(2)], // pos 1: api, error
            vec![sfst::KvId(0), sfst::KvId(3)], // pos 2: api, info
            vec![sfst::KvId(1), sfst::KvId(2)], // pos 3: worker, error
            vec![sfst::KvId(1), sfst::KvId(3)], // pos 4: worker, info
            vec![sfst::KvId(1), sfst::KvId(2)], // pos 5: worker, error
        ];

        let mut writer = sfst::Writer::new();
        writer.set_summary(sfst::pack(&summary, 1).unwrap());
        writer.set_metadata(sfst::pack(&metadata, 1).unwrap());
        writer.set_primary(sfst::pack(&primary, 1).unwrap());
        writer.set_timestamps(sfst::pack(&timestamps, 1).unwrap());
        writer.add_stream_batch(sfst::pack(&stream_entries, 1).unwrap());
        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();
        std::fs::write(path, &buf).unwrap();
    }

    /// Install a single SFST file under tenant `t`, returning the
    /// machine/boot uuids used so callers can reason about seq.
    fn install_sfst(
        tr: &mut TenantRegistries,
        tenant: &str,
        seq: u64,
        min_s: u32,
    ) -> (Uuid, Uuid) {
        let machine = Uuid::from_u128(0x11);
        let boot = Uuid::from_u128(0x22);
        let id = FileId::new(machine, boot, seq, 7);

        // get_or_create initializes the tenant subdir; we then write
        // the file at the registry's computed path and track it.
        let reg = tr.get_or_create(&TenantId::from(tenant));
        let path = reg.sfst.file_path(id);
        write_test_sfst(&path, min_s);
        let size = ByteSize(std::fs::metadata(&path).unwrap().len());
        let summary = sfst::Summary {
            min_timestamp_s: min_s,
            max_timestamp_s: min_s + 5,
            total_logs: 6,
            stream: ServiceStream::new("ns", "svc"),
        };
        reg.sfst.track(id, size, summary);
        (machine, boot)
    }

    #[tokio::test]
    async fn info_request_returns_capability_descriptor() {
        let h = make_handler(make_tenant_registries());
        let req: OtelLogsRequest = serde_json::from_slice(br#"{"info": true}"#).unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], 200);
        assert!(
            v["accepted_params"]
                .as_array()
                .unwrap()
                .contains(&Value::String("after".into()))
        );
        assert!(v.get("facets").is_none());
    }

    #[tokio::test]
    async fn empty_payload_defaults_to_data_request() {
        // Matches the legacy JournalRequest semantic: a POST body
        // without an `info` field is a data request, not capability
        // discovery. The UI's data POSTs rely on this.
        let req: OtelLogsRequest = serde_json::from_slice(b"{}").unwrap();
        assert!(!req.info);
        let h = make_handler(make_tenant_registries());
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        // Empty registry → empty Logs envelope, not the capability descriptor.
        assert!(v.get("facets").is_some());
        assert_eq!(v["items"]["matched"], 0);
    }

    #[tokio::test]
    async fn no_sfst_yields_empty_envelope() {
        let h = make_handler(make_tenant_registries());
        let req: OtelLogsRequest =
            serde_json::from_slice(br#"{"info": false, "after": 100, "before": 200}"#)
                .unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], 200);
        assert!(v["facets"].as_array().unwrap().is_empty());
        assert_eq!(v["items"]["matched"], 0);
    }

    #[tokio::test]
    async fn non_overlapping_window_yields_empty_envelope() {
        let mut tr = make_tenant_registries();
        install_sfst(&mut tr, "tenant-a", 1, 1_700_000_000);
        let h = make_handler(tr);

        // Request window is 1900..2000 — nowhere near the file's 1.7e9 span.
        let req: OtelLogsRequest =
            serde_json::from_slice(br#"{"info": false, "after": 1900, "before": 2000}"#)
                .unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        assert!(v["facets"].as_array().unwrap().is_empty());
        assert_eq!(v["items"]["matched"], 0);
    }

    #[tokio::test]
    async fn populated_response_carries_facets_and_histogram() {
        let mut tr = make_tenant_registries();
        let min_s = 1_700_000_000;
        install_sfst(&mut tr, "tenant-a", 1, min_s);
        let h = make_handler(tr);

        let req: OtelLogsRequest = serde_json::from_slice(
            format!(
                r#"{{"info": false, "after": {}, "before": {}}}"#,
                min_s,
                min_s + 60
            )
            .as_bytes(),
        )
        .unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(v["status"], 200);

        // Two facets — service + severity_text — both low-card.
        let facets = v["facets"].as_array().unwrap();
        let ids: Vec<&str> = facets.iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert!(ids.contains(&"service"));
        assert!(ids.contains(&"severity_text"));

        // severity_text facet sees both values with count 3 each.
        let sev = facets
            .iter()
            .find(|f| f["id"] == "severity_text")
            .unwrap();
        let opts = sev["options"].as_array().unwrap();
        let counts: HashMap<&str, u64> = opts
            .iter()
            .map(|o| {
                (
                    o["id"].as_str().unwrap(),
                    o["count"].as_u64().unwrap(),
                )
            })
            .collect();
        assert_eq!(counts.get("info"), Some(&3));
        assert_eq!(counts.get("error"), Some(&3));

        // Histogram defaulted to severity_text (one of DEFAULT_HISTOGRAM_FIELDS).
        assert_eq!(v["histogram"]["id"], "severity_text");
        // 6 logs spread across 6 seconds, all in-window.
        assert_eq!(v["items"]["matched"], 6);

        // available_histograms drops high-card (none here) but lists both fields.
        let avh = v["available_histograms"].as_array().unwrap();
        let avh_ids: Vec<&str> =
            avh.iter().map(|a| a["id"].as_str().unwrap()).collect();
        assert!(avh_ids.contains(&"service"));
        assert!(avh_ids.contains(&"severity_text"));

        // Log-row table stays empty in this MVP.
        assert!(v["data"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn selection_filter_narrows_facet_counts_with_self_exclusion() {
        let mut tr = make_tenant_registries();
        let min_s = 1_700_000_000;
        install_sfst(&mut tr, "tenant-a", 1, min_s);
        let h = make_handler(tr);

        // Filter `service=api` (positions 0,1,2). The `severity_text` facet
        // should reflect that filter: info=2 (pos 0,2), error=1 (pos 1).
        // The `service` facet, by self-exclusion, should still see both
        // values at their full counts.
        let payload = format!(
            r#"{{"info": false, "after": {a}, "before": {b}, "selections": {{"service": ["api"]}}}}"#,
            a = min_s,
            b = min_s + 60
        );
        let req: OtelLogsRequest = serde_json::from_slice(payload.as_bytes()).unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();

        let facets = v["facets"].as_array().unwrap();

        let sev = facets
            .iter()
            .find(|f| f["id"] == "severity_text")
            .unwrap();
        let sev_counts: HashMap<&str, u64> = sev["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| (o["id"].as_str().unwrap(), o["count"].as_u64().unwrap()))
            .collect();
        assert_eq!(sev_counts.get("info"), Some(&2));
        assert_eq!(sev_counts.get("error"), Some(&1));

        let svc = facets.iter().find(|f| f["id"] == "service").unwrap();
        let svc_counts: HashMap<&str, u64> = svc["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|o| (o["id"].as_str().unwrap(), o["count"].as_u64().unwrap()))
            .collect();
        // Self-exclusion: `api` and `worker` both visible at full counts.
        assert_eq!(svc_counts.get("api"), Some(&3));
        assert_eq!(svc_counts.get("worker"), Some(&3));
    }

    #[tokio::test]
    async fn most_recent_wins_when_multiple_files_exist() {
        // Two files in different tenants; the higher seq's data should
        // surface. Older file's stream is `ns/old`; freshest is `ns/svc`.
        let mut tr = make_tenant_registries();
        install_sfst(&mut tr, "tenant-old", 1, 1_600_000_000);
        install_sfst(&mut tr, "tenant-new", 99, 1_700_000_000);
        let h = make_handler(tr);

        let req: OtelLogsRequest = serde_json::from_slice(
            br#"{"info": false, "after": 1700000000, "before": 1700000100}"#,
        )
        .unwrap();
        let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
        let v = serde_json::to_value(&resp).unwrap();
        // Only the new file (seq=99, 2024-ish) overlaps the window.
        assert_eq!(v["items"]["matched"], 6);
    }

    #[test]
    fn patches_data_request_args_into_payload() {
        // No "info" token — data request. info must be false so the
        // handler runs the query path, not the capability descriptor.
        let args = vec![
            "after:100".to_string(),
            "before:200".to_string(),
            "slice:true".to_string(),
        ];
        let bytes = patch_args_into_payload(&args, None).unwrap();
        let req: OtelLogsRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(req.after, 100);
        assert_eq!(req.before, 200);
        assert!(!req.info);
    }

    #[test]
    fn patches_info_request_args_into_payload() {
        // "info" token present — capability discovery.
        let args = vec![
            "info".to_string(),
            "after:100".to_string(),
            "before:200".to_string(),
        ];
        let bytes = patch_args_into_payload(&args, None).unwrap();
        let req: OtelLogsRequest = serde_json::from_slice(&bytes).unwrap();
        assert!(req.info);
        assert_eq!(req.after, 100);
        assert_eq!(req.before, 200);
    }

    #[test]
    fn declaration_carries_legacy_flags() {
        let h = make_handler(make_tenant_registries());
        let d = h.declaration();
        assert_eq!(d.name, "otel-logs");
        assert!(d.global);
        assert_eq!(d.tags.as_deref(), Some("logs"));
        let access = d.access.unwrap();
        assert!(access.contains(HttpAccess::SIGNED_ID));
        assert!(access.contains(HttpAccess::SAME_SPACE));
        assert!(access.contains(HttpAccess::SENSITIVE_DATA));
    }

    #[test]
    fn pick_histogram_field_prefers_requested_when_eligible() {
        let fields = vec![
            sfst::FieldEntry {
                name: "service".into(),
                cardinality: 2,
                tier: sfst::FieldTier::Low,
            },
            sfst::FieldEntry {
                name: "severity_text".into(),
                cardinality: 2,
                tier: sfst::FieldTier::Low,
            },
        ];
        assert_eq!(pick_histogram_field("service", &fields), "service");
    }

    #[test]
    fn pick_histogram_field_falls_back_to_default() {
        let fields = vec![sfst::FieldEntry {
            name: "severity_text".into(),
            cardinality: 2,
            tier: sfst::FieldTier::Low,
        }];
        // Empty request → walk DEFAULT_HISTOGRAM_FIELDS.
        assert_eq!(pick_histogram_field("", &fields), "severity_text");
        // Unknown requested field → walk DEFAULT_HISTOGRAM_FIELDS.
        assert_eq!(pick_histogram_field("nonexistent", &fields), "severity_text");
    }

    #[test]
    fn pick_histogram_field_avoids_high_card() {
        let fields = vec![sfst::FieldEntry {
            name: "trace_id".into(),
            cardinality: 50_000,
            tier: sfst::FieldTier::High,
        }];
        // No eligible field → empty string (timeline call will surface UnknownField).
        assert_eq!(pick_histogram_field("trace_id", &fields), "");
    }

    #[test]
    fn pick_facet_fields_caps_default_set() {
        // 20 low-card fields (cardinality ≤ MAX_FACET_OPTIONS_PER_FIELD)
        // and 3 mid-card-but-above-cap fields. The defaults should
        // keep at most MAX_FACET_FIELDS of the eligible ones and drop
        // the over-cap fields entirely.
        let mut fields: Vec<sfst::FieldEntry> = (0..20)
            .map(|i| sfst::FieldEntry {
                name: format!("low_{i}"),
                cardinality: 5,
                tier: sfst::FieldTier::Low,
            })
            .collect();
        fields.extend((0..3).map(|i| sfst::FieldEntry {
            name: format!("noisy_{i}"),
            cardinality: 500,
            tier: sfst::FieldTier::Mid,
        }));

        let picked = pick_facet_fields(&[], &fields);
        assert_eq!(picked.len(), MAX_FACET_FIELDS);
        // None of the high-cardinality fields snuck in.
        assert!(picked.iter().all(|n| n.starts_with("low_")));
    }

    #[test]
    fn pick_facet_fields_honors_explicit_request_even_over_cap() {
        let fields = vec![sfst::FieldEntry {
            name: "noisy".into(),
            cardinality: 500,
            tier: sfst::FieldTier::Mid,
        }];
        let picked = pick_facet_fields(&["noisy".to_string()], &fields);
        assert_eq!(picked, vec!["noisy".to_string()]);
    }

    #[test]
    fn pick_bucket_width_targets_60_buckets() {
        // 15-minute window → 15-second buckets → 15e9 ns.
        assert_eq!(pick_bucket_width_ns(0, 900), 15 * 1_000_000_000);
        // 60-second window → 1-second buckets (minimum).
        assert_eq!(pick_bucket_width_ns(0, 60), 1_000_000_000);
        // Inverted window → default 15-minute span.
        assert_eq!(pick_bucket_width_ns(0, 0), 15 * 1_000_000_000);
    }
}
