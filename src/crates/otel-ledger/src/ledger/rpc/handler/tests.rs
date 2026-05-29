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
async fn only_overlapping_file_contributes() {
    // Two files in different tenants. The window matches only the
    // newer file's span — the older one's range is filtered out by
    // the candidate planner.
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
    // Only the new file's 6 logs overlap the window.
    assert_eq!(v["items"]["matched"], 6);
}

#[tokio::test]
async fn multiple_overlapping_files_merge_counts_and_facets() {
    // Two SFSTs in the same tenant whose spans both fall inside the
    // request window. The planner returns both; the handler should
    // sum `matched` and union facet counts.
    //
    // Each file has 6 logs (3 info, 3 error; 3 api, 3 worker) so
    // the merged response should show 12 logs total.
    let mut tr = make_tenant_registries();
    let earlier = 1_700_000_000u32;
    let later = earlier + 100; // 6-second spans don't touch each other
    install_sfst(&mut tr, "tenant-a", 1, earlier);
    install_sfst(&mut tr, "tenant-a", 2, later);
    let h = make_handler(tr);

    // Window covers both files' spans.
    let payload = format!(
        r#"{{"info": false, "after": {a}, "before": {b}}}"#,
        a = earlier,
        b = later + 60
    );
    let req: OtelLogsRequest = serde_json::from_slice(payload.as_bytes()).unwrap();
    let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
    let v = serde_json::to_value(&resp).unwrap();

    // Both files contribute → 12 matched.
    assert_eq!(v["items"]["matched"], 12);

    // Facets union across files; per-value counts sum.
    let facets = v["facets"].as_array().unwrap();
    let sev = facets
        .iter()
        .find(|f| f["id"] == "severity_text")
        .expect("severity_text facet must be present in both files");
    // Each file has 3 `info` logs → merged 6.
    let info_count = sev["options"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["id"] == "info")
        .map(|o| o["count"].as_u64().unwrap())
        .unwrap_or(0);
    assert_eq!(info_count, 6);

    let svc = facets
        .iter()
        .find(|f| f["id"] == "service")
        .expect("service facet must be present");
    let svc_counts: HashMap<&str, u64> = svc["options"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| (o["id"].as_str().unwrap(), o["count"].as_u64().unwrap()))
        .collect();
    // Each file: 3 api, 3 worker → merged 6 each.
    assert_eq!(svc_counts.get("api"), Some(&6));
    assert_eq!(svc_counts.get("worker"), Some(&6));
}

#[tokio::test]
async fn no_time_bound_falls_back_to_recent_window() {
    // `(after=0, before=0)` is the legacy "no time bound" sentinel.
    // The effective-window helper should fall back to the last 15
    // minutes, so an SFST installed in that range produces a
    // populated response (rather than an empty stub).
    let now_s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as u32;
    let recent = now_s.saturating_sub(300); // 5 min ago, inside the 15-min fallback

    let mut tr = make_tenant_registries();
    install_sfst(&mut tr, "tenant-a", 1, recent);
    let h = make_handler(tr);

    let req: OtelLogsRequest = serde_json::from_slice(br#"{"info": false}"#).unwrap();
    let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    // Fixture has 6 logs — all should match (the file's range
    // [recent, recent+5] sits inside the 15-min fallback window).
    assert_eq!(v["items"]["matched"], 6);
}

#[tokio::test]
async fn no_time_bound_with_only_stale_data_yields_empty_envelope() {
    // `(after=0, before=0)` defaults to the last 15 minutes. The only
    // SFST is from 2024, so nothing overlaps the defaulted window. The
    // handler returns the empty envelope — it does not reach back to a
    // stale file just because the window came up empty.
    let mut tr = make_tenant_registries();
    let file_min_s = 1_700_000_000u32; // far in the past
    install_sfst(&mut tr, "tenant-a", 1, file_min_s);
    let h = make_handler(tr);

    let req: OtelLogsRequest = serde_json::from_slice(br#"{"info": false}"#).unwrap();
    let resp = h.on_call(make_ctx("t1"), req).await.unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["items"]["matched"], 0);
    assert!(v["facets"].as_array().unwrap().is_empty());
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
fn pick_histogram_field_honors_requested() {
    // Whatever the request supplies is returned verbatim; the
    // timeline call decides whether it's actually usable.
    assert_eq!(pick_histogram_field("service.name"), "service.name");
    assert_eq!(pick_histogram_field("trace_id"), "trace_id");
}

#[test]
fn pick_histogram_field_defaults_to_severity_text() {
    // Empty `histogram` → OTel canonical default. No file-shape
    // dependence — producers + UI handle "is this meaningful?"
    assert_eq!(pick_histogram_field(""), "severity_text");
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
fn bucket_width_picks_from_curated_set() {
    // 15-minute window → 15s (largest in VALID_BUCKET_WIDTHS_S
    // with span/w >= TARGET_BUCKETS=60).
    assert_eq!(bucket_width_for_span_s(900), 15);
    // 1-minute window → 1s buckets (60 / 1 == 60).
    assert_eq!(bucket_width_for_span_s(60), 1);
    // Very small spans (< TARGET_BUCKETS seconds) → 1s fallback.
    assert_eq!(bucket_width_for_span_s(30), 1);
    // 1-hour window → 60s buckets (3600 / 60 == 60).
    assert_eq!(bucket_width_for_span_s(3600), 60);
    // 1-day window → 1800s (30-min) buckets (86400 / 1800 == 48 < 60,
    // 86400 / 900 == 96 >= 60 → 900s wins).
    assert_eq!(bucket_width_for_span_s(86400), 900);
}

#[test]
fn align_window_snaps_outward_to_bucket_boundaries() {
    // Identity when already aligned.
    assert_eq!(align_window(0, 900, 15), (0, 900));
    // Floor the `after`, ceil the `before`.
    assert_eq!(align_window(1, 14, 15), (0, 15));
    // Larger window — both bounds rounded outward.
    assert_eq!(align_window(7, 92, 15), (0, 105));
    // Consecutive 1-second shifts within the same bucket-width slot
    // produce the same aligned window — this is what kills the chart's
    // sub-bucket shape jitter across the UI's per-second polling.
    let a = align_window(1779995982, 1779996882, 15);
    let b = align_window(1779995983, 1779996883, 15);
    let c = align_window(1779995984, 1779996884, 15);
    assert_eq!(a, b);
    assert_eq!(b, c);
}
