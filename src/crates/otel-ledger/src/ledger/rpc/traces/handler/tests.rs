use super::*;
use crate::ledger::rpc::traces::fixtures::{
    install_sealed, install_sfst, install_wal, make_registries, otlp_req, otlp_req_svc,
    sealed_traces,
};
use bridge::function::ProgressState;
use file_lifecycle::registry::TenantRegistries;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn make_handler_over(registries: Arc<RwLock<TenantRegistries>>) -> OtelTracesHandler {
    // Small min_entries so the WAL fixtures split into chunks + tail —
    // the end-to-end tests then cross the chunk-build path for real.
    OtelTracesHandler::new(
        registries,
        Arc::new(ChunkCache::new(64 * 1024 * 1024)),
        4,
        None,
    )
}

fn make_handler() -> OtelTracesHandler {
    make_handler_over(make_registries())
}

fn make_ctx() -> FunctionCallContext {
    FunctionCallContext::new(
        "tx-test".to_string(),
        ProgressState::new(),
        CancellationToken::new(),
    )
}

async fn call_on(
    h: &OtelTracesHandler,
    v: serde_json::Value,
) -> netdata_plugin_error::Result<OtelTracesResponse> {
    let req: OtelTracesRequest = serde_json::from_value(v).unwrap();
    h.on_call(make_ctx(), req).await
}

async fn call(v: serde_json::Value) -> netdata_plugin_error::Result<OtelTracesResponse> {
    call_on(&make_handler(), v).await
}

#[tokio::test]
async fn info_returns_the_descriptor() {
    let resp = call(json!({"info": {}})).await.unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["mode"], "info");
    assert_eq!(v["version"], 1);
    assert_eq!(v["status"], 200);
    assert_eq!(v["type"], "traces");
    assert_eq!(v["has_history"], true);
    assert_eq!(v["v"], 3);
    assert_eq!(
        v["accepted_params"],
        json!(["info", "explore", "values", "trace", "tenant"])
    );
    assert_eq!(v["required_params"], json!([]));
}

#[tokio::test]
async fn every_mode_is_implemented_an_empty_agent_answers_them_all() {
    // The mode catalog is complete: no selector errors as
    // not-implemented anymore; an empty agent answers each cleanly.
    for body in [
        json!({"explore": {}}),
        json!({"values": {"field": "name"}}),
        json!({"trace": {"id": "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a"}}),
    ] {
        call(body.clone()).await.unwrap_or_else(|e| panic!("{body}: {e}"));
    }
}

// Conflicting/malformed/missing-mode bodies no longer reach on_call —
// they fail request DESERIALIZATION (pinned in the wire tests) and the
// bridge maps them to transport 400s (pinned in the bridge-level tests
// at the end of this file).

#[test]
fn declaration_advertises_otel_traces() {
    let d = make_handler().declaration();
    assert_eq!(d.name, "otel-traces");
    assert!(d.global);
    assert_eq!(d.tags.as_deref(), Some("traces"));
    assert_eq!(
        d.access,
        Some(HttpAccess::SIGNED_ID | HttpAccess::SAME_SPACE | HttpAccess::SENSITIVE_DATA)
    );
}

// ── The trace mode ──────────────────────────────────────────────────

/// The fixture trace: 3 spans, ids [0x11;16]/[i;8], span 1 the root.
const FIXTURE_TRACE_ID: &str = "11111111111111111111111111111111";

async fn handler_with_fixture_wal() -> OtelTracesHandler {
    let registries = make_registries();
    install_wal(
        &registries,
        "default",
        1,
        vec![otlp_req(0x11, 3, 1_000_000_000)],
    )
    .await;
    make_handler_over(registries)
}

#[tokio::test]
async fn a_sealed_traces_fixture_serves_as_a_local_sealed_file() {
    // The fixture is what the traces seal writes: its summary spans the
    // spans' seconds and counts them, and the file answers a lookup alone.
    let (summary, _) = sealed_traces(vec![otlp_req(0x11, 3, 1_000_000_000)]);
    assert_eq!(
        (
            summary.min_timestamp_s,
            summary.max_timestamp_s,
            summary.record_count
        ),
        (1, 1, 3)
    );

    let registries = make_registries();
    install_sealed(
        &registries,
        "default",
        1,
        vec![otlp_req(0x11, 3, 1_000_000_000)],
    )
    .await;
    let h = make_handler_over(registries);
    let v = serde_json::to_value(
        call_on(&h, json!({"trace": {"id": FIXTURE_TRACE_ID}}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 3);
    // The logs ingest keys this service's stream by (namespace, name).
    let stream = otel_logs_identity::ServiceStream::new("", "svc");
    assert_eq!(
        v["log_streams"],
        json!([format!("{:016x}", stream.ns_hash())]),
        "the trace names where its logs live"
    );
}

#[tokio::test]
async fn a_refused_wal_makes_a_trace_lookup_partial() {
    let registries = make_registries();
    install_sealed(
        &registries,
        "default",
        1,
        vec![otlp_req(0x11, 3, 1_000_000_000)],
    )
    .await;
    let path = install_wal(
        &registries,
        "default",
        2,
        vec![otlp_req(0x22, 3, 2_000_000_000)],
    )
    .await;
    let len = std::fs::metadata(&path).unwrap().len();
    let garbage = vec![0xFFu8; (len - wal::HEADER_SIZE as u64) as usize];
    {
        use std::io::{Seek, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        f.seek(std::io::SeekFrom::Start(wal::HEADER_SIZE as u64)).unwrap();
        f.write_all(&garbage).unwrap();
    }

    let h = make_handler_over(registries);
    let v = serde_json::to_value(
        call_on(&h, json!({"trace": {"id": FIXTURE_TRACE_ID}}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        v["status"],
        json!({"partial": [{"reason": "source_failure", "count": 1}]})
    );
    assert_eq!(v["items"]["returned"], 3, "the sealed file still answers");
}

#[tokio::test]
async fn trace_coverage_declares_the_full_range_for_absent_bounds() {
    let h = handler_with_fixture_wal().await;
    let resp = call_on(&h, json!({"trace": {"id": FIXTURE_TRACE_ID}}))
        .await
        .unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["coverage"], json!({"after": 0, "before": 4_294_967_295_u32}));
}

#[tokio::test]
async fn bounded_trace_fetch_echoes_coverage_and_assembles_identically() {
    let h = handler_with_fixture_wal().await;
    let unbounded = serde_json::to_value(
        call_on(&h, json!({"trace": {"id": FIXTURE_TRACE_ID}}))
            .await
            .unwrap(),
    )
    .unwrap();
    let bounded = serde_json::to_value(
        call_on(
            &h,
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 0, "before": 100}}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(bounded["coverage"], json!({"after": 0, "before": 100}));
    assert_eq!(bounded["spans"], unbounded["spans"]);
    assert_eq!(bounded["status"], json!({"complete": true}));
}

#[tokio::test]
async fn bounded_trace_fetch_prunes_non_overlapping_sealed_files() {
    let registries = make_registries();
    install_wal(
        &registries,
        "default",
        1,
        vec![otlp_req(0x11, 3, 1_000_000_000)],
    )
    .await;
    // Tracked but never written: probing it fails a source, so partial
    // status is the observable for "this file was captured".
    install_sfst(&registries, "default", 2, 500_000, 500_100).await;
    let h = make_handler_over(registries);

    // Bounds not overlapping the sealed summary: pruned at capture,
    // never probed — the assembly stays complete.
    let outside = serde_json::to_value(
        call_on(
            &h,
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 0, "before": 100}}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(outside["status"], json!({"complete": true}));

    // Absent bounds capture the full range: the unreadable sealed file
    // is probed and surfaces as a source failure.
    let full = serde_json::to_value(
        call_on(&h, json!({"trace": {"id": FIXTURE_TRACE_ID}}))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(full["status"], json!({"partial": [{"reason": "source_failure", "count": 1}]}));
}

#[tokio::test]
async fn bounds_excluding_every_file_yield_a_complete_empty_trace() {
    let registries = make_registries();
    install_sfst(&registries, "default", 1, 1_000, 1_100).await;
    let h = make_handler_over(registries);
    let v = serde_json::to_value(
        call_on(
            &h,
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 500_000, "before": 500_100}}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 0);
    assert_eq!(v["coverage"], json!({"after": 500_000, "before": 500_100}));
}

#[tokio::test]
async fn trace_bounds_of_any_width_are_accepted() {
    // No width cap: a range far wider than any UI formula is served and
    // declared as asked.
    let h = handler_with_fixture_wal().await;
    let v = serde_json::to_value(
        call_on(
            &h,
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 1, "before": 4_000_000_000_u32}}),
        )
        .await
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        v["coverage"],
        json!({"after": 1, "before": 4_000_000_000_u32})
    );
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 3);
}

#[tokio::test]
async fn trace_by_id_assembles_the_fixture_trace_end_to_end() {
    let h = handler_with_fixture_wal().await;
    let resp = call_on(&h, json!({"trace": {"id": FIXTURE_TRACE_ID}}))
        .await
        .unwrap();
    let v = serde_json::to_value(&resp).unwrap();

    assert_eq!(v["trace_id"], FIXTURE_TRACE_ID);
    assert_eq!(v["coverage"], json!({"after": 0, "before": 4_294_967_295_u32}));
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 3);
    // Span 1 (unset parent) is the root; spans 2 and 3 parent to it.
    assert_eq!(v["summary_root"], 0);
    assert_eq!(v["roots"], json!([0]));
    assert_eq!(v["children"], json!([[1, 2], [], []]));
    let s0 = &v["spans"][0];
    assert_eq!(s0["span_id"], "01".repeat(8));
    assert!(s0.get("parent_span_id").is_none());
    assert_eq!(v["spans"][1]["parent_span_id"], "01".repeat(8));
    // The seal indexed the OTLP name into the row facets.
    assert!(
        s0["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|kv| kv == &json!(["name", "span-1"])),
        "fields carry the span name: {}",
        s0["fields"]
    );
    // And the typed map describes what came back.
    assert!(
        v["field_kinds"]["fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|kv| kv[0] == "name"),
        "{}",
        v["field_kinds"]
    );
}

#[tokio::test]
async fn span_cap_returns_the_earliest_spans_and_a_size_cap_partial() {
    let h = handler_with_fixture_wal().await;
    let resp = call_on(
        &h,
        json!({"trace": {"id": FIXTURE_TRACE_ID, "span_cap": 2}}),
    )
    .await
    .unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["status"], json!({"partial": [{"reason": "size_cap", "count": 1}]}));
    assert_eq!(v["items"]["returned"], 2);
    // Pin WHICH spans: the globally earliest two (fixture starts ascend
    // with the span id), so a latest-two regression fails here, not only
    // in the combiner's own unit test.
    assert_eq!(v["spans"][0]["span_id"], "01".repeat(8));
    assert_eq!(v["spans"][1]["span_id"], "02".repeat(8));
}

#[tokio::test]
async fn absent_trace_id_is_a_complete_empty_trace() {
    // "Nothing stored under this id" is an answer, not an error.
    let h = handler_with_fixture_wal().await;
    let resp = call_on(
        &h,
        json!({"trace": {"id": "99999999999999999999999999999999"}}),
    )
    .await
    .unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 0);
    assert_eq!(v["spans"], json!([]));
    assert_eq!(v["summary_root"], serde_json::Value::Null);
}

#[tokio::test]
async fn semantically_invalid_trace_requests_are_clean_client_errors() {
    // SHAPE errors (null/array selectors, missing/unknown fields) fail
    // request deserialization and are pinned in the wire tests; what
    // stays here is the SEMANTIC validation the handler owns.
    for (body, needle) in [
        (json!({"trace": {"id": "xyz"}}), "32 hex"),
        // The engine's own request validation surfaces verbatim.
        (
            json!({"trace": {"id": "00000000000000000000000000000000"}}),
            "all-zero",
        ),
        (
            json!({"trace": {"id": FIXTURE_TRACE_ID, "span_cap": 0}}),
            "zero span cap",
        ),
        // The wire may only tighten the runaway-merge bound.
        (
            json!({"trace": {"id": FIXTURE_TRACE_ID, "span_cap": 65_537}}),
            "exceeds the maximum",
        ),
        // Assembly bounds: both-or-neither, ordered.
        (
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 100}}),
            "both 'after' and 'before'",
        ),
        (
            json!({"trace": {"id": FIXTURE_TRACE_ID, "before": 100}}),
            "both 'after' and 'before'",
        ),
        (
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 200, "before": 100}}),
            "after 200 >= before 100",
        ),
        (
            json!({"trace": {"id": FIXTURE_TRACE_ID, "after": 100, "before": 100}}),
            "after 100 >= before 100",
        ),
    ] {
        let err = call(body.clone()).await.expect_err("must be a client error");
        let msg = err.to_string();
        assert!(msg.contains(needle), "for {body}: {msg}");
    }
}

// ── Mode-neutral behaviour over one corpus ───────────────────────────

/// Corpus base, unix seconds (chosen inside an explicit query window).
const T_S: u32 = 1_700_000_000;

fn base_ns(offset_s: u64) -> u64 {
    (T_S as u64 + offset_s) * 1_000_000_000
}

/// Five traces in one WAL: A/C/E on svc-a, B/D on svc-b.
async fn handler_with_corpus() -> OtelTracesHandler {
    let registries = make_registries();
    install_wal(
        &registries,
        "default",
        1,
        vec![
            otlp_req_svc(0x0A, 1, base_ns(10), "svc-a"),
            otlp_req_svc(0x0B, 2, base_ns(20), "svc-b"),
            otlp_req_svc(0x0C, 1, base_ns(30), "svc-a"),
            otlp_req_svc(0x0D, 1, base_ns(30), "svc-b"),
            otlp_req_svc(0x0E, 3, base_ns(40), "svc-a"),
        ],
    )
    .await;
    make_handler_over(registries)
}

fn window_body() -> serde_json::Value {
    json!({"after": T_S, "before": T_S + 100})
}

/// Wrap flat params as the wire's `{mode: params}` shape — tests build
/// params flat and name the mode at the call site.
fn as_mode(mode: &str, params: serde_json::Value) -> serde_json::Value {
    json!({ mode: params })
}

fn merge(base: &mut serde_json::Value, extra: serde_json::Value) {
    base.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
}

/// Run `body` with a progress state the test keeps; returns `(done, total)`.
async fn progress_of(h: &OtelTracesHandler, body: serde_json::Value) -> (usize, usize) {
    let progress = ProgressState::new();
    let ctx = FunctionCallContext::new(
        "tx-test".to_string(),
        progress.clone(),
        CancellationToken::new(),
    );
    let req: OtelTracesRequest = serde_json::from_value(body.clone()).unwrap();
    h.on_call(ctx, req)
        .await
        .unwrap_or_else(|e| panic!("{body}: {e}"));
    progress.load()
}

#[tokio::test]
async fn every_mode_sets_its_progress_total_and_completes_it() {
    // The corpus WAL resolves to two chunks at min_entries 4; every pass
    // ticks once per source it walks. The explorer walks them twice (its
    // Groups section reads every source a second time).
    let h = handler_with_corpus().await;
    let windowed = |mode: &str, extra: serde_json::Value| {
        let mut body = window_body();
        merge(&mut body, extra);
        as_mode(mode, body)
    };
    for (body, expected) in [
        (
            json!({"trace": {"id": "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a"}}),
            (2, 2),
        ),
        (windowed("explore", json!({})), (4, 4)),
        (windowed("values", json!({"field": "name"})), (2, 2)),
    ] {
        assert_eq!(progress_of(&h, body.clone()).await, expected, "{body}");
    }
}

#[tokio::test]
async fn tenant_scoping_isolates_and_defaults() {
    // The tenant selector scopes every data mode: another tenant's data
    // is invisible, an unknown tenant is empty (never an all-tenant
    // union), and the omitted selector reads the default tenant.
    let registries = make_registries();
    install_wal(
        &registries,
        "tenant-a",
        1,
        vec![otlp_req_svc(0x0A, 1, base_ns(10), "svc")],
    )
    .await;
    let h = make_handler_over(registries);
    let count = |v: &serde_json::Value| -> usize {
        if let Some(items) = v.get("items").and_then(|i| i.get("returned")) {
            items.as_u64().unwrap() as usize
        } else if let Some(values) = v.get("values") {
            values.as_array().unwrap().len()
        } else {
            v["data"]["histogram"]["totals"]["count"].as_u64().unwrap() as usize
        }
    };

    // Default tenant: tenant-a's data is invisible; the owning tenant sees
    // it — `tenant` rides at the TOP level; an unknown tenant is empty, not
    // an error and not a union.
    let explore = as_mode("explore", window_body());
    let v = serde_json::to_value(call_on(&h, explore.clone()).await.unwrap()).unwrap();
    assert_eq!(count(&v), 0);
    let mut wrapped = explore.clone();
    wrapped["tenant"] = json!("tenant-a");
    let v = serde_json::to_value(call_on(&h, wrapped).await.unwrap()).unwrap();
    assert_eq!(count(&v), 1);
    let mut wrapped = explore;
    wrapped["tenant"] = json!("nope");
    let v = serde_json::to_value(call_on(&h, wrapped).await.unwrap()).unwrap();
    assert_eq!(count(&v), 0);

    // Tenant routes identically through the other data modes.
    let mut values_body = window_body();
    merge(
        &mut values_body,
        json!({"field": "resource.attributes.service.name"}),
    );
    for mut wrapped in [
        json!({"trace": {"id": "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a"}}),
        as_mode("values", values_body),
    ] {
        let unscoped = serde_json::to_value(call_on(&h, wrapped.clone()).await.unwrap()).unwrap();
        wrapped["tenant"] = json!("tenant-a");
        let scoped = serde_json::to_value(call_on(&h, wrapped.clone()).await.unwrap()).unwrap();
        assert_eq!(count(&unscoped), 0, "unscoped sees nothing: {wrapped}");
        assert!(count(&scoped) > 0, "tenant-a sees its data: {wrapped}");
    }
}

// ── The transport status boundary ───────────────────────────────────
//
// Shape errors fail request DESERIALIZATION and must surface as
// transport 400s through the bridge; the handler's semantic
// validation keeps its (pre-existing) 500 mapping. These cross
// `HandlerAdapter::handle_raw` — the same path the live bridge runs.

async fn raw_call(payload: Option<&[u8]>) -> (u32, String) {
    let adapter = bridge::function::HandlerAdapter::new(make_handler());
    let (status, payload) =
        crate::ledger::rpc::traces::fixtures::call_through_bridge(&adapter, payload).await;
    (status, String::from_utf8_lossy(&payload).into_owned())
}

#[tokio::test]
async fn shape_errors_are_transport_400s() {
    for (payload, needle) in [
        (&br#"{"bogus": 1}"#[..], "unknown field"),
        (br#"{"explore": {}, "after": 1}"#, "unknown field"),
        (br#"{"search": {}}"#, "unknown field"),
        (
            br#"{"trace": {}, "explore": {}}"#,
            "conflicting mode selectors",
        ),
        (br#"{"info": true}"#, "invalid info selector"),
        (br#"{"trace": null}"#, "invalid trace selector"),
        (br#"{"explore": []}"#, "expected an object"),
        (br#"[]"#, "otel-traces request object"),
        (
            br#"{"explore": {}, "tenant": "a", "tenant": "b"}"#,
            "duplicate field",
        ),
    ] {
        let (status, body) = raw_call(Some(payload)).await;
        assert_eq!(
            status,
            400,
            "for {}: {body}",
            String::from_utf8_lossy(payload)
        );
        assert!(
            body.contains(needle),
            "for {}: {body}",
            String::from_utf8_lossy(payload)
        );
    }
}

#[tokio::test]
async fn semantic_errors_keep_the_handler_status() {
    // The handler's own validation (zero span cap, one-sided trace
    // bounds, bad trace id) rides the pre-existing handler-error
    // mapping — frozen behavior, deliberately not reclassified here.
    for payload in [
        &br#"{"trace": {"id": "11111111111111111111111111111111", "span_cap": 0}}"#[..],
        br#"{"trace": {"id": "11111111111111111111111111111111", "after": 100}}"#,
        br#"{"trace": {"id": "xyz"}}"#,
    ] {
        let (status, body) = raw_call(Some(payload)).await;
        assert_eq!(status, 500, "for {}: {body}", String::from_utf8_lossy(payload));
        assert!(
            body.contains("invalid otel-traces request"),
            "for {}: {body}",
            String::from_utf8_lossy(payload)
        );
    }
}

#[tokio::test]
async fn absent_empty_and_mode_less_payloads_are_400s() {
    // The bridge parses an absent payload as `{}`; a body naming no mode
    // is refused either way, the explicit `{}` with the modes named.
    let (status, body) = raw_call(None).await;
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("Request payload is empty"), "{body}");

    let (status, body) = raw_call(Some(b"{}")).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body.contains("no mode selector: name exactly one of info, explore, values, trace"),
        "{body}"
    );

    let (status, body) = raw_call(Some(b"")).await;
    assert_eq!(status, 400);
    assert!(body.contains("EOF"), "{body}");
}

#[tokio::test]
async fn every_response_shape_declares_its_mode() {
    let h = handler_with_corpus().await;
    for (body, mode) in [
        (json!({"info": {}}), "info"),
        (as_mode("explore", window_body()), "explore"),
        (
            {
                let mut inner = window_body();
                merge(&mut inner, json!({"field": "name"}));
                as_mode("values", inner)
            },
            "values",
        ),
        (json!({"trace": {"id": "0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a"}}), "trace"),
    ] {
        let v = serde_json::to_value(call_on(&h, body.clone()).await.unwrap()).unwrap();
        let declared = v.get("mode").or_else(|| v["data"].get("mode"));
        assert_eq!(declared, Some(&json!(mode)), "for {body}");
    }
}

// ── Explore ─────────────────────────────────────────────────────────

/// A sealed file with one failing request at 1 s, and an active WAL with
/// requests at 2, 3 and 4 s (at min_entries 4: one chunk and a tail).
async fn explore_corpus() -> (OtelTracesHandler, std::path::PathBuf) {
    use crate::ledger::rpc::traces::fixtures::otlp_req_err;
    let registries = make_registries();
    install_sealed(
        &registries,
        "default",
        1,
        vec![otlp_req_err(0x11, 3, 1_000_000_000, "checkout")],
    )
    .await;
    let wal = install_wal(
        &registries,
        "default",
        2,
        vec![
            otlp_req(0x22, 3, 2_000_000_000),
            otlp_req(0x33, 3, 3_000_000_000),
            otlp_req(0x44, 3, 4_000_000_000),
        ],
    )
    .await;
    (make_handler_over(registries), wal)
}

fn entry_spans_body() -> serde_json::Value {
    json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root", "inbound"]},
        "sections": {"histogram": {"stack": "status_code", "percentiles": false}}
    }})
}

#[tokio::test]
async fn explore_percentiles_are_on_by_default_and_marked_approximate() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {"after": 1, "before": 10, "filter": {"_role": ["root"]}}});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    let histogram = &v["data"]["histogram"];
    assert_eq!(
        histogram["percentiles"],
        json!({"approximate": true, "max_relative_error": 0.0078125})
    );
    // Every root lasts 500 ns: its bucket reports 502 (the midpoint of [500, 504)).
    let one = json!({"p50_ns": 502, "p95_ns": 502, "p99_ns": 502});
    for field in ["p50_ns", "p95_ns", "p99_ns"] {
        assert_eq!(histogram["buckets"][0][field], one[field]);
        assert_eq!(histogram["totals"][field], one[field]);
        assert!(histogram["buckets"][5].get(field).is_none(), "an empty bucket has none");
    }
}

#[tokio::test]
async fn explore_answers_the_histogram_in_the_functions_envelope() {
    let (h, _) = explore_corpus().await;
    let v = serde_json::to_value(call_on(&h, entry_spans_body()).await.unwrap()).unwrap();
    let unset_only = json!({"counts": [0, 1], "unset": 0, "other": 0});
    let empty = json!({"counts": [0, 0], "unset": 0, "other": 0});
    assert_eq!(
        v,
        json!({
            "status": 200,
            "type": "traces",
            "data": {
                "mode": "explore",
                "version": 1,
                "window": {
                    "after": 1,
                    "before": 10,
                    "grid": {"start_ns": "1000000000", "bucket_ns": 1_000_000_000, "buckets": 9}
                },
                "status": {"complete": true},
                "histogram": {
                    "status": {"complete": true},
                    "stack": "status_code",
                    "dimensions": ["error", "unset"],
                    "buckets": [
                        {"counts": [1, 0], "unset": 0, "other": 0},
                        unset_only, unset_only, unset_only,
                        empty, empty, empty, empty, empty
                    ],
                    "totals": {"count": 4, "errors": 1}
                }
            }
        })
    );
    // One capture: the sealed file, the WAL's chunk and its tail.
    assert_eq!(progress_of(&h, entry_spans_body()).await, (3, 3));
}

#[tokio::test]
async fn explore_counts_a_refused_wal() {
    let (h, wal) = explore_corpus().await;
    let len = std::fs::metadata(&wal).unwrap().len();
    let garbage = vec![0xFFu8; (len - wal::HEADER_SIZE as u64) as usize];
    {
        use std::io::{Seek, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.seek(std::io::SeekFrom::Start(wal::HEADER_SIZE as u64)).unwrap();
        f.write_all(&garbage).unwrap();
    }
    let v = serde_json::to_value(call_on(&h, entry_spans_body()).await.unwrap()).unwrap();
    let partial = json!({"partial": [{"reason": "source_failure", "count": 1, "of": 2}]});
    assert_eq!(v["data"]["status"], partial);
    assert_eq!(v["data"]["histogram"]["status"], partial);
    assert_eq!(
        v["data"]["histogram"]["totals"],
        json!({"count": 1, "errors": 1}),
        "the sealed file still answers"
    );
}

const SECTIONS: [&str; 5] = ["histogram", "facets", "groups", "rows", "fields"];

/// QRY-01: one request answers every section off one capture of the three
/// sources (Groups walk them twice), and a rows-only request answers only its
/// rows off a single walk.
#[tokio::test]
async fn explore_sections_one_capture() {
    let (h, _) = explore_corpus().await;
    let every = json!({"explore": {"after": 1, "before": 10}});
    let v = serde_json::to_value(call_on(&h, every.clone()).await.unwrap()).unwrap();
    for section in SECTIONS {
        assert!(v["data"].get(section).is_some(), "{section}: {v}");
    }
    assert_eq!(progress_of(&h, every).await, (6, 6));

    let rows_only = json!({"explore": {
        "after": 1, "before": 10,
        "sections": {"rows": {"order": "newest", "limit": 10}}
    }});
    let v = serde_json::to_value(call_on(&h, rows_only.clone()).await.unwrap()).unwrap();
    for section in SECTIONS {
        assert_eq!(
            v["data"].get(section).is_some(),
            section == "rows",
            "{section}: {v}"
        );
    }
    assert!(!v["data"]["rows"]["items"].as_array().unwrap().is_empty());
    assert_eq!(progress_of(&h, rows_only).await, (3, 3));
}

/// QRY-44: a source that cannot be read is counted in every section.
#[tokio::test]
async fn a_refused_wal_marks_every_section() {
    let (h, wal) = explore_corpus().await;
    let len = std::fs::metadata(&wal).unwrap().len();
    {
        use std::io::{Seek, Write};
        let mut f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.seek(std::io::SeekFrom::Start(wal::HEADER_SIZE as u64))
            .unwrap();
        f.write_all(&vec![0xFFu8; (len - wal::HEADER_SIZE as u64) as usize])
            .unwrap();
    }
    let every = json!({"explore": {"after": 1, "before": 10}});
    let v = serde_json::to_value(call_on(&h, every).await.unwrap()).unwrap();
    let partial = json!({"partial": [{"reason": "source_failure", "count": 1, "of": 2}]});
    for section in SECTIONS {
        assert_eq!(v["data"][section]["status"], partial, "{section}: {v}");
    }
}

/// Of the four roots, the three svc roots have an unset status, stored as a
/// value, so a `null` chip (rows without the field) keeps none of them.
#[tokio::test]
async fn explore_scopes_rows_by_the_unset_status() {
    let (h, _) = explore_corpus().await;
    let count = |statuses: serde_json::Value| {
        json!({"explore": {
            "after": 1, "before": 10,
            "filter": {"_role": ["root"], "status_code": statuses},
            "sections": {"histogram": {"stack": "status_code"}}
        }})
    };
    for (statuses, want) in [
        (json!(["unset"]), 3),
        (json!(["error", "unset"]), 4),
        (json!([null]), 0),
    ] {
        let v = serde_json::to_value(call_on(&h, count(statuses.clone())).await.unwrap()).unwrap();
        assert_eq!(
            v["data"]["histogram"]["totals"]["count"], want,
            "{statuses}"
        );
    }
}

/// The status facet lists the unset roots like any value.
#[tokio::test]
async fn explore_lists_the_unset_status() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root"]},
        "sections": {"facets": {"fields": ["status_code"]}}
    }});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    assert_eq!(
        v["data"]["facets"]["fields"][0]["values"],
        json!([{"value": "error", "count": 1}, {"value": "unset", "count": 3}])
    );
}

#[tokio::test]
async fn explore_answers_a_selection() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root"]},
        "selection": {"filter": {"status_code": ["error"]}},
        "sections": {
            "facets": {"fields": ["resource.attributes.service.name", "status_code"]},
            "rows": {"limit": 5}
        }
    }});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    // One root of four is an error: checkout's. Neither service has the five
    // selection rows a rank needs, so the values keep the count order. Status
    // is what the selection is made of: plain counts, flagged, before the
    // unranked fields (none rank here).
    assert_eq!(
        v["data"]["facets"],
        json!({
            "status": {"complete": true},
            "comparison": {"scope": 4, "selection": 1, "min_support": 5},
            "fields": [{
                "field": "status_code",
                "in_selection": true,
                "omitted_values": 0,
                "omitted_rows": 0,
                "values": [
                    {"value": "error", "count": 1},
                    {"value": "unset", "count": 3}
                ]
            }, {
                "field": "resource.attributes.service.name",
                "totals": {"scope": 4, "selection": 1},
                "omitted_values": 0,
                "omitted_rows": 0,
                "values": [
                    {"value": "svc", "count": 3, "selection": 0, "baseline": 3,
                     "eligible": false, "diff": -1.0},
                    {"value": "checkout", "count": 1, "selection": 1, "baseline": 0,
                     "eligible": false, "diff": 1.0}
                ]
            }],
            "unavailable": []
        })
    );
    let rows = &v["data"]["rows"];
    assert_eq!(rows["matched"], 1);
    assert_eq!(rows["items"].as_array().unwrap().len(), 1);
    assert_eq!(rows["items"][0]["service"], "checkout");
}

#[tokio::test]
async fn explore_answers_groups() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root"]},
        "sections": {"groups": {}}
    }});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    assert!(v["data"].get("histogram").is_none(), "only the asked sections");
    let window = &v["data"]["window"];
    let window_s = window["before"].as_u64().unwrap() - window["after"].as_u64().unwrap();
    // Every span lasts 500 ns (p95 502, the midpoint of [500, 504)); the
    // children start after their root ends, so self time is the duration.
    let group = |service: &str, operation: &str, spans: u64, errors: u64| {
        json!({
            "service": service, "operation": operation, "spans": spans, "errors": errors,
            "errors_originated": errors, "p95_ns": 502, "self_ns": (spans * 500).to_string()
        })
    };
    assert_eq!(
        v["data"]["groups"],
        json!({
            "status": {"complete": true},
            "window_s": window_s,
            "self_ns_total": "6000",
            "rows": [
                group("svc", "span-1", 3, 0),
                group("svc", "span-2", 3, 0),
                group("svc", "span-3", 3, 0),
                group("checkout", "span-1", 1, 1),
                group("checkout", "span-2", 1, 0),
                group("checkout", "span-3", 1, 0),
            ],
            "other": null
        }),
        "no delta without a selection"
    );
}

/// QRY-20: under a selection each group splits by its traces' side; the
/// sides add up to the group, and the totals count traces, not spans.
#[tokio::test]
async fn explore_answers_groups_under_a_selection() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root"]},
        "selection": {"filter": {"status_code": ["error"]}},
        "sections": {"groups": {}}
    }});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    // Checkout's trace has the one error root; the three svc traces make the
    // baseline. Every span has 500 ns of self time.
    let side = |spans: u64, origins: u64| {
        let self_ns = (spans * 500).to_string();
        json!({"spans": spans, "errors_originated": origins, "self_ns": self_ns})
    };
    let row = |service: &str, operation: &str, selection, baseline| {
        json!({"service": service, "operation": operation,
               "selection": selection, "baseline": baseline})
    };
    assert_eq!(
        v["data"]["groups"]["delta"],
        json!({
            "selection_traces": 1,
            "baseline_traces": 3,
            "selection_self_ns_total": "1500",
            "baseline_self_ns_total": "4500",
            "rows": [
                row("svc", "span-1", side(0, 0), side(3, 0)),
                row("svc", "span-2", side(0, 0), side(3, 0)),
                row("svc", "span-3", side(0, 0), side(3, 0)),
                row("checkout", "span-1", side(1, 1), side(0, 0)),
                row("checkout", "span-2", side(1, 0), side(0, 0)),
                row("checkout", "span-3", side(1, 0), side(0, 0)),
            ],
            "other": null
        })
    );
}

#[tokio::test]
async fn explore_answers_facets_with_their_own_status() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root"]},
        "sections": {"facets": {"fields": ["_role", "resource.attributes.service.name"]}}
    }});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    assert!(v["data"].get("histogram").is_none(), "only the asked sections");
    assert_eq!(
        v["data"]["facets"],
        json!({
            "status": {"complete": true},
            "fields": [
                {
                    "field": "_role",
                    "values": [{"value": "internal", "count": 8}, {"value": "root", "count": 4}],
                    "omitted_values": 0,
                    "omitted_rows": 0
                },
                {
                    "field": "resource.attributes.service.name",
                    "values": [{"value": "checkout", "count": 1}, {"value": "svc", "count": 3}],
                    "omitted_values": 0,
                    "omitted_rows": 0
                }
            ],
            "unavailable": []
        })
    );
}


/// A root span of the explore corpus: it has no children, so its self time
/// is its duration.
fn root_row(trace_byte: u8, second: u64, service: &str, status: Option<&str>) -> serde_json::Value {
    let start_ns = (second * 1_000_000_000 + 1_000).to_string();
    let trace_id = format!("{trace_byte:02x}").repeat(16);
    let span_id = "01".repeat(8);
    json!({
        "cursor": format!("{start_ns}:{trace_id}:{span_id}"),
        "start_ns": start_ns,
        "duration_ns": 500,
        "self_duration_ns": 500,
        "trace_id": trace_id,
        "span_id": span_id,
        "service": service,
        "name": "span-1",
        "role": "root",
        "status": status,
        "columns": {}
    })
}

#[tokio::test]
async fn explore_pages_rows_newest_first_by_cursor() {
    let (h, _) = explore_corpus().await;
    let page = |anchor: Option<&str>| {
        let mut rows = json!({"limit": 2, "columns": ["attributes.nope"]});
        if let Some(anchor) = anchor {
            rows["anchor"] = json!(anchor);
        }
        json!({"explore": {
            "after": 1, "before": 10, "filter": {"_role": ["root"]},
            "sections": {"rows": rows}
        }})
    };
    let v = serde_json::to_value(call_on(&h, page(None)).await.unwrap()).unwrap();
    assert!(
        v["data"].get("histogram").is_none(),
        "only the asked sections"
    );
    assert_eq!(
        v["data"]["rows"],
        json!({
            "status": {"complete": true},
            "order": "newest",
            "matched": 4,
            "has_older": true,
            "has_newer": false,
            "items": [root_row(0x44, 4, "svc", Some("unset")), root_row(0x33, 3, "svc", Some("unset"))]
        })
    );

    let cursor = v["data"]["rows"]["items"][1]["cursor"]
        .as_str()
        .unwrap()
        .to_string();
    let v = serde_json::to_value(call_on(&h, page(Some(&cursor))).await.unwrap()).unwrap();
    let rows = &v["data"]["rows"];
    assert_eq!(
        (&rows["has_older"], &rows["has_newer"]),
        (&json!(false), &json!(true))
    );
    assert_eq!(
        rows["items"],
        json!([
            root_row(0x22, 2, "svc", Some("unset")),
            root_row(0x11, 1, "checkout", Some("error"))
        ])
    );
}

#[tokio::test]
async fn explore_lists_the_slowest_rows() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {
        "after": 1, "before": 10, "filter": {"_role": ["root"]},
        "sections": {"rows": {"order": "slowest", "limit": 1}}
    }});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    assert_eq!(
        v["data"]["rows"],
        json!({
            "status": {"complete": true},
            "order": "slowest",
            "matched": 4,
            "items": [root_row(0x44, 4, "svc", Some("unset"))]
        }),
        "equal durations: the latest start first"
    );
}

#[tokio::test]
async fn explore_lists_the_window_fields() {
    let (h, _) = explore_corpus().await;
    let body = json!({"explore": {"after": 1, "before": 10, "sections": {"fields": {}}}});
    let v = serde_json::to_value(call_on(&h, body).await.unwrap()).unwrap();
    let fields = &v["data"]["fields"];
    assert_eq!(fields["status"], json!({"complete": true}));
    assert_eq!(
        fields["columns"],
        json!(["duration", "self_duration", "trace_id", "span_id"])
    );
    let low = |name: &str, text: bool| {
        json!({"name": name, "tier": "low", "chip": true, "facet": true, "stack": true,
               "text": text, "column": true})
    };
    assert_eq!(
        fields["items"],
        json!([
            low("_err_origin", false),
            low("_role", false),
            low("name", true),
            low("resource.attributes.service.name", true),
            low("status_code", true),
            low("status_message", true)
        ]),
        "every stored field by name, without `_status_code`"
    );
}

#[tokio::test]
async fn values_suggest_stored_values_by_prefix() {
    let (h, _) = explore_corpus().await;
    let call = |body: serde_json::Value| {
        let h = &h;
        async move { serde_json::to_value(call_on(h, body).await.unwrap()).unwrap() }
    };
    let v = call(json!({"values": {"after": 1, "before": 10, "field": "name", "prefix": "span-"}}))
        .await;
    assert_eq!(
        v,
        json!({
            "mode": "values",
            "version": 1,
            "field": "name",
            "values": ["span-1", "span-2", "span-3"],
            "truncated": false,
            "status": {"complete": true}
        })
    );
    let v = call(json!({"values": {"after": 1, "before": 10, "field": "name", "limit": 2}})).await;
    assert_eq!(
        (&v["values"], &v["truncated"]),
        (&json!(["span-1", "span-2"]), &json!(true))
    );
    let v = call(json!({"values": {
        "after": 1, "before": 10, "field": "resource.attributes.service.name", "prefix": "c"
    }}))
    .await;
    assert_eq!(v["values"], json!(["checkout"]), "the sealed file's value");
    let v = call(json!({"values": {"after": 1, "before": 10, "field": "nope"}})).await;
    assert_eq!(
        (&v["values"], &v["status"]),
        (&json!([]), &json!({"complete": true}))
    );
}

// ── Admission ────────────────────────────────────────────────────────

#[tokio::test]
async fn admission_limit_one_serializes() {
    let h = OtelTracesHandler::with_admission(
        make_registries(),
        Arc::new(ChunkCache::new(64 * 1024 * 1024)),
        4,
        None,
        1,
    );
    let wait = std::time::Duration::from_millis(200);
    let done = std::time::Duration::from_secs(10);

    // With the only turn taken a request waits, and runs once it is free.
    let held = Arc::clone(&h.admission).acquire_owned().await.unwrap();
    let waiting = call_on(&h, as_mode("explore", window_body()));
    tokio::pin!(waiting);
    assert!(tokio::time::timeout(wait, &mut waiting).await.is_err());
    drop(held);
    let resp = tokio::time::timeout(done, waiting).await.unwrap().unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["data"]["status"], json!({"complete": true}));

    // A request cancelled while it waits answers at once, cancelled, having
    // read nothing.
    let held = Arc::clone(&h.admission).acquire_owned().await.unwrap();
    let progress = ProgressState::new();
    let cancel = CancellationToken::new();
    let ctx = FunctionCallContext::new("tx-test".to_string(), progress.clone(), cancel.clone());
    let req: OtelTracesRequest = serde_json::from_value(as_mode(
        "values",
        merge_into(window_body(), json!({"field": "name"})),
    ))
    .unwrap();
    let cancelled = h.on_call(ctx, req);
    tokio::pin!(cancelled);
    assert!(tokio::time::timeout(wait, &mut cancelled).await.is_err());
    cancel.cancel();
    let resp = tokio::time::timeout(done, cancelled)
        .await
        .unwrap()
        .unwrap();
    let v = serde_json::to_value(&resp).unwrap();
    assert_eq!(v["status"]["partial"][0]["reason"], "cancelled", "{v}");
    assert_eq!(progress.load().0, 0);
    drop(held);
}

fn merge_into(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    merge(&mut base, extra);
    base
}
