use super::*;
use serde_json::json;
use sfsq::traces::QueryStatus;

// ── parse_trace_id ──────────────────────────────────────────────────

#[test]
fn parses_hex_ids_case_insensitively_and_trimmed() {
    let id = parse_trace_id(" 0102030405060708090A0b0c0D0e0F10 ").unwrap();
    assert_eq!(
        id.to_string(),
        "0102030405060708090a0b0c0d0e0f10",
        "echo form is canonical lowercase"
    );
}

#[test]
fn rejects_wrong_length_and_non_hex() {
    for bad in ["", "0102", "zz02030405060708090a0b0c0d0e0f10", "0102030405060708090a0b0c0d0e0f1"] {
        let err = parse_trace_id(bad).expect_err("must reject");
        assert!(err.contains("32 hex"), "{err}");
    }
}

#[test]
fn unset_id_parses_here_and_is_the_engines_call() {
    // The all-zero sentinel is syntactically valid hex; the ENGINE
    // rejects the lookup with its precise message (pinned in the
    // handler test).
    let id = parse_trace_id("00000000000000000000000000000000").unwrap();
    assert!(id.is_unset());
}

// ── to_trace_result ─────────────────────────────────────────────────

fn span(id: u8, parent: Option<u8>, start_ns: i64) -> sfst::TraceSpan {
    sfst::TraceSpan {
        span_id: sfst::SpanId::from([id; 8]),
        parent_span_id: parent
            .map(|p| sfst::SpanId::from([p; 8]))
            .unwrap_or_default(),
        start_ns,
        duration_ns: 500,
        kind: 2,
        flags: 1,
        dropped_attributes_count: 0,
        dropped_events_count: 0,
        dropped_links_count: 0,
        fields: vec![("name".into(), format!("span-{id}"))],
        events: vec![sfst::TraceEvent {
            time_unix_nano: 42,
            name: "ev".into(),
            dropped_attributes_count: 0,
            attributes: vec![("k".into(), "v".into())],
        }],
        links: vec![sfst::TraceLink {
            trace_id: sfst::TraceId::from([0xAB; 16]),
            span_id: sfst::SpanId::from([0xCD; 8]),
            trace_state: "ot=th:8".into(),
            flags: 0,
            dropped_attributes_count: 0,
            attributes: vec![],
        }],
    }
}

#[test]
fn trace_result_shape_is_pinned() {
    let trace_id = parse_trace_id("11111111111111111111111111111111").unwrap();
    let data = TraceData {
        trace: sfst::Trace {
            spans: vec![span(1, None, 1_000), span(2, Some(1), 2_000)],
            roots: vec![0],
            children: vec![vec![1], vec![]],
        },
        status: QueryStatus::Complete,
        field_kinds: FieldKinds {
            fields: vec![("name".into(), sfst::ValueKind::Str)],
            event_attributes: vec![("k".into(), sfst::ValueKind::Str)],
            link_attributes: vec![],
        },
        family: sfst::SpanFamily {
            error_origin: vec![false, true],
            child_ns: vec![100, 0],
        },
    };
    let v = serde_json::to_value(to_trace_result(&trace_id, data, CoverageWire { after: 0, before: u32::MAX })).unwrap();

    assert_eq!(v["version"], 1);
    assert_eq!(v["trace_id"], "11111111111111111111111111111111");
    assert_eq!(v["coverage"], json!({"after": 0, "before": 4_294_967_295_u32}));
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"], json!({"returned": 2}));
    assert_eq!(v["summary_root"], 0);
    assert_eq!(v["roots"], json!([0]));
    assert_eq!(v["children"], json!([[1], []]));

    let s0 = &v["spans"][0];
    assert_eq!(s0["span_id"], "0101010101010101");
    assert!(
        s0.get("parent_span_id").is_none(),
        "an unset parent is ABSENT (OTel root convention), not a zero string"
    );
    assert_eq!(s0["fields"], json!([["name", "span-1"]]));
    assert_eq!(s0["events"][0]["name"], "ev");
    assert_eq!(s0["events"][0]["attributes"], json!([["k", "v"]]));
    assert_eq!(s0["links"][0]["trace_id"], "ab".repeat(16));
    assert_eq!(s0["links"][0]["span_id"], "cd".repeat(8));

    assert_eq!(
        (&s0["self_duration_ns"], &s0["error_origin"]),
        (&json!(400), &json!(false))
    );

    let s1 = &v["spans"][1];
    assert_eq!(s1["parent_span_id"], "0101010101010101");
    assert_eq!(
        (&s1["self_duration_ns"], &s1["error_origin"]),
        (&json!(500), &json!(true))
    );

    assert_eq!(
        v["field_kinds"],
        json!({
            "fields": [["name", "str"]],
            "event_attributes": [["k", "str"]],
            "link_attributes": [],
        })
    );
}

#[test]
fn empty_trace_maps_to_complete_zero_span_result() {
    let trace_id = parse_trace_id("22222222222222222222222222222222").unwrap();
    let data = TraceData {
        trace: sfst::Trace {
            spans: vec![],
            roots: vec![],
            children: vec![],
        },
        status: QueryStatus::Complete,
        field_kinds: FieldKinds::default(),
        family: sfst::SpanFamily {
            error_origin: vec![],
            child_ns: vec![],
        },
    };
    let v = serde_json::to_value(to_trace_result(&trace_id, data, CoverageWire { after: 0, before: u32::MAX })).unwrap();
    assert_eq!(v["status"], json!({"complete": true}));
    assert_eq!(v["items"]["returned"], 0);
    assert_eq!(v["summary_root"], serde_json::Value::Null);
    assert_eq!(v["spans"], json!([]));
}

// ── Explore ─────────────────────────────────────────────────────────

fn explore_params(v: serde_json::Value) -> crate::ledger::rpc::traces::wire::ExploreParams {
    serde_json::from_value(v).unwrap()
}

#[test]
fn explore_window_grid() {
    // An hour boundary, so every width below aligns exactly.
    const NOW: u32 = 1_699_999_200;
    const S: i64 = 1_000_000_000;
    for (body, width_s, buckets, after) in [
        (json!({}), 15, 60, NOW - 900),
        (json!({"after": -3600}), 60, 60, NOW - 3600),
        (json!({"after": -86_400}), 900, 96, NOW - 86_400),
        (json!({"after": 7, "before": 893}), 10, 90, 0),
    ] {
        let (query, aligned_after, aligned_before) =
            to_explore_query(&explore_params(body.clone()), NOW);
        assert_eq!(query.grid.bucket_width_ns, width_s * S, "{body}");
        assert_eq!(query.grid.num_buckets, buckets, "{body}");
        assert_eq!(query.grid.bucket_start_ns, i64::from(after) * S, "{body}");
        assert_eq!(aligned_after, after, "{body}");
        assert_eq!(
            i64::from(aligned_before),
            i64::from(after) + width_s * buckets as i64,
            "{body}"
        );
    }
}

/// Chips become the scope filter value for value; a `null` value becomes the
/// absent term, OR'd with the field's other values.
#[test]
fn explore_chips_become_the_scope_filter() {
    let (query, _, _) = to_explore_query(
        &explore_params(json!({
            "filter": {
                "_role": ["root", "inbound"],
                "resource.attributes.service.name": ["checkout"],
                "status_code": ["error", null]
            },
            "selection": {"filter": {"name": [null]}}
        })),
        1_700_000_000,
    );
    assert_eq!(
        query.scope.filter,
        sfst::Filter::new()
            .select("_role", "root")
            .select("_role", "inbound")
            .select("resource.attributes.service.name", "checkout")
            .select("status_code", "error")
            .select_absent("status_code")
    );
    assert_eq!(
        query.selection.map(|selection| selection.filter),
        Some(sfst::Filter::new().select_absent("name"))
    );
    assert_eq!(
        query.sections.histogram.map(|h| h.stack),
        Some("status_code".to_string())
    );
}

#[test]
fn explore_histogram_carries_its_own_status() {
    use sfsq::traces::StatusBuilder;
    use sfsq::traces::explore::{ExploreData, HistogramData, Totals};
    let mut facet_only = StatusBuilder::new();
    facet_only.add(sfsq::traces::PartialReason::FacetHighCard);
    let data = ExploreData {
        status: facet_only.finish(),
        sources: 1,
        histogram: Some(HistogramData {
            status: QueryStatus::Complete,
            stack: "status_code".to_string(),
            dimensions: Vec::new(),
            buckets: Vec::new(),
            totals: Totals {
                count: 0,
                errors: 0,
                percentiles: None,
            },
            percentiles: false,
            durations: false,
        }),
        facets: None,
        groups: None,
        rows: None,
        fields: None,
    };
    let grid = sfst::Grid::new(1_000_000_000, 1_000_000_000, 1);
    let v = serde_json::to_value(to_explore_response(data, grid, 1, 2)).unwrap();
    assert_eq!(
        v["data"]["status"],
        json!({"partial": [{"reason": "facet_high_card", "count": 1}]})
    );
    assert_eq!(v["data"]["histogram"]["status"], json!({"complete": true}));
}
