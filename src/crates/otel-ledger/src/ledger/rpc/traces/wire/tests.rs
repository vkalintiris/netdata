use super::explore::{FacetsRequest, HistogramRequest};
use super::*;
use serde_json::json;
use sfsq::traces::StatusBuilder;

// ── Request / mode resolution ───────────────────────────────────────

fn req(v: serde_json::Value) -> OtelTracesRequest {
    serde_json::from_value(v).expect("request deserializes")
}

fn req_err(v: serde_json::Value) -> String {
    serde_json::from_value::<OtelTracesRequest>(v)
        .expect_err("request must be rejected")
        .to_string()
}

#[test]
fn a_request_without_a_mode_selector_selects_the_functions_search_view() {
    let TracesMode::Functions(p) = req(json!({})).mode else {
        panic!("Functions mode expected");
    };
    assert_eq!((p.after, p.before, p.last), (0, 0, 20));
    assert_eq!(p.anchor, None);
    assert!(p.selections.is_empty());

    let r = req(json!({"tenant": "t1"}));
    assert!(matches!(r.mode, TracesMode::Functions(_)));
    assert_eq!(r.tenant.as_deref(), Some("t1"));

    let TracesMode::Functions(p) = req(json!({
        "after": -3600,
        "before": -60,
        "last": 7,
        "anchor": "cursor",
        "selections": {"root_name": ["GET /api"]},
        "timeout": 120000
    }))
    .mode
    else {
        panic!("Functions mode expected");
    };
    assert_eq!((p.after, p.before, p.last), (-3600, -60, 7));
    assert_eq!(p.anchor.as_deref(), Some("cursor"));
    assert_eq!(p.selections["root_name"], ["GET /api"]);

    let search = p.search_params(10_000).unwrap();
    assert_eq!((search.after, search.before), (6_340, 9_940));
}

#[test]
fn the_functions_view_forwards_the_minimum_trace_duration() {
    let TracesMode::Functions(p) = req(json!({"min_trace_duration_ns": 250_000_000})).mode else {
        panic!("Functions mode expected");
    };
    assert_eq!(
        p.search_params(10_000).unwrap().min_trace_duration_ns,
        Some(250_000_000)
    );

    // Omitted stays unset — existing callers keep their behaviour.
    let TracesMode::Functions(p) = req(json!({})).mode else {
        panic!("Functions mode expected");
    };
    assert_eq!(p.search_params(10_000).unwrap().min_trace_duration_ns, None);
}

#[test]
fn the_functions_view_forwards_the_maximum_trace_duration() {
    let TracesMode::Functions(p) = req(json!({"max_trace_duration_ns": 250_000_000})).mode else {
        panic!("Functions mode expected");
    };
    assert_eq!(
        p.search_params(10_000).unwrap().max_trace_duration_ns,
        Some(250_000_000)
    );

    // Omitted stays unset — existing callers keep their behaviour.
    let TracesMode::Functions(p) = req(json!({})).mode else {
        panic!("Functions mode expected");
    };
    assert_eq!(p.search_params(10_000).unwrap().max_trace_duration_ns, None);
}

#[test]
fn overview_facets_rides_with_the_functions_parameters() {
    // The aggregate's facet opt-in is a Functions parameter: it selects
    // the Functions view like the others, and it cannot ride with a
    // mode selector (the response key is not a request field — the
    // `overview` selector still means the legacy mode).
    assert!(matches!(
        req(json!({"after": -900, "overview_facets": true})).mode,
        TracesMode::Functions(_)
    ));
    let err = req_err(json!({"overview": {}, "overview_facets": true}));
    assert!(
        err.contains("cannot mix a mode selector with Functions parameters"),
        "{err}"
    );
}

#[test]
fn info_is_the_strict_empty_object() {
    assert!(matches!(req(json!({"info": {}})).mode, TracesMode::Info));
    // Both of the old wire's boolean forms, junk scalars, null, and
    // non-empty objects are malformed selectors — a malformed selector
    // must not silently select.
    for body in [
        json!({"info": true}),
        json!({"info": false}),
        json!({"info": 7}),
        json!({"info": null}),
        json!({"info": {"extra": 1}}),
        json!({"info": []}),
    ] {
        let err = req_err(body.clone());
        assert!(err.contains("invalid info selector"), "for {body}: {err}");
    }
}

#[test]
fn each_selector_selects_its_mode() {
    assert!(matches!(
        req(json!({"trace": {"id": "00"}})).mode,
        TracesMode::Trace(_)
    ));
    assert!(matches!(
        req(json!({"overview": {}})).mode,
        TracesMode::Overview(_)
    ));
    assert!(matches!(
        req(json!({"explore": {}})).mode,
        TracesMode::Explore(_)
    ));
    assert!(matches!(
        req(json!({"values": {"field": "name"}})).mode,
        TracesMode::Values(_)
    ));
    assert!(matches!(
        req(json!({"search": {}})).mode,
        TracesMode::Search(_)
    ));
    assert!(matches!(req(json!({"info": {}})).mode, TracesMode::Info));
}

#[test]
fn a_present_but_null_selector_selects_then_rejects() {
    // serde's stock Option<Value> would swallow the null into "absent"
    // and surface a missing-mode error; the presence-preserving
    // deserializer keeps the selection so the error names the selector.
    for (body, needle) in [
        (json!({"trace": null}), "invalid trace selector"),
        (json!({"overview": null}), "invalid overview selector"),
        (json!({"explore": null}), "invalid explore selector"),
        (json!({"search": null}), "invalid search selector"),
    ] {
        let err = req_err(body.clone());
        assert!(err.contains(needle), "for {body}: {err}");
    }
}

#[test]
fn selectors_are_object_only_arrays_reject() {
    // serde-derived structs also accept positional JSON arrays via the
    // seq visitor; the object gate closes that hole at both levels.
    for (body, needle) in [
        (json!({"overview": []}), "invalid overview selector"),
        (json!({"trace": ["00ff", 7]}), "invalid trace selector"),
        (json!({"search": []}), "invalid search selector"),
    ] {
        let err = req_err(body.clone());
        assert!(
            err.contains(needle) && err.contains("expected an object"),
            "for {body}: {err}"
        );
    }
}

#[test]
fn the_top_level_must_be_a_json_object() {
    for body in [json!([]), json!([{}]), json!([1, 2]), json!(7), json!("x")] {
        let err = req_err(body.clone());
        assert!(err.contains("otel-traces request object"), "for {body}: {err}");
    }
}

#[test]
fn unknown_and_retired_top_level_keys_are_client_errors() {
    for body in [json!({"search": {}, "bogus": 1}), json!({"bogus": 1})] {
        let err = req_err(body.clone());
        assert!(err.contains("unknown field"), "for {body}: {err}");
    }

    for body in [
        json!({"search": {}, "after": 1, "before": 2}),
        json!({"search": {}, "last": 5}),
        json!({"search": {}, "timeout": 30}),
        json!({"search": {}, "min_trace_duration_ns": 1}),
        json!({"search": {}, "max_trace_duration_ns": 1}),
        json!({"trace": {"id": "00"}, "anchor": "x"}),
    ] {
        let err = req_err(body.clone());
        assert!(
            err.contains("cannot mix a mode selector with Functions parameters"),
            "for {body}: {err}"
        );
    }
}

#[test]
fn trace_params_reject_malformed_and_parse_bounds() {
    let cases = [
        (json!({"trace": 7}), "invalid trace selector"),
        (json!({"trace": {}}), "missing field `id`"),
        (json!({"trace": {"id": "00", "bogus": 1}}), "unknown field"),
    ];
    for (body, needle) in cases {
        let err = req_err(body.clone());
        assert!(err.contains(needle), "for {body}: {err}");
    }
    let TracesMode::Trace(ok) = req(json!({"trace": {"id": "00ff", "span_cap": 9}})).mode else {
        panic!("trace mode expected");
    };
    assert_eq!(ok.id, "00ff");
    assert_eq!(ok.span_cap, Some(9));
    assert_eq!(ok.after, None);
    assert_eq!(ok.before, None);

    let TracesMode::Trace(bounded) =
        req(json!({"trace": {"id": "00ff", "after": 100, "before": 200}})).mode
    else {
        panic!("trace mode expected");
    };
    assert_eq!(bounded.after, Some(100));
    assert_eq!(bounded.before, Some(200));
}

#[test]
fn conflicting_selectors_are_a_client_error() {
    let err = req_err(json!({"trace": {}, "overview": {}}));
    assert!(
        err.contains("conflicting mode selectors: trace, overview"),
        "{err}"
    );
    let err = req_err(json!({"overview": {}, "search": {}}));
    assert!(
        err.contains("conflicting mode selectors: overview, search"),
        "{err}"
    );
    // info is a PEER selector — no precedence.
    let err = req_err(json!({"info": {}, "trace": {"id": "00"}}));
    assert!(
        err.contains("conflicting mode selectors: info, trace"),
        "{err}"
    );
}

#[test]
fn a_conflict_is_reported_before_a_malformed_selector() {
    // ALL present selectors are counted before any is decoded — a
    // conflicting body reports the conflict even when one selector is
    // also malformed.
    let err = req_err(json!({"trace": null, "overview": {}}));
    assert!(
        err.contains("conflicting mode selectors: trace, overview"),
        "{err}"
    );
}

#[test]
fn duplicate_keys_are_rejected_not_last_value_wins() {
    // The manual top-level visitor streams the original map into the
    // derived raw visitor, preserving serde's duplicate-field
    // rejection (a `json!` literal cannot express duplicates — raw
    // bytes only).
    let err = serde_json::from_slice::<OtelTracesRequest>(
        br#"{"trace": {"id": "00"}, "trace": {"id": "ff"}}"#,
    )
    .expect_err("duplicate selector must be rejected");
    assert!(err.to_string().contains("duplicate field"), "{err}");

    let err = serde_json::from_slice::<OtelTracesRequest>(
        br#"{"search": {}, "tenant": "a", "tenant": "b"}"#,
    )
    .expect_err("duplicate tenant must be rejected");
    assert!(err.to_string().contains("duplicate field"), "{err}");
}

#[test]
fn search_params_defaults_and_strictness() {
    let TracesMode::Search(p) = req(json!({"search": {}})).mode else {
        panic!("search mode expected");
    };
    assert_eq!(p.after, 0);
    assert_eq!(p.before, 0);
    assert_eq!(p.limit, sfsq::traces::DEFAULT_SEARCH_LIMIT);
    assert_eq!(p.spans_per_trace, None);
    assert!(p.selections.is_empty());
    assert_eq!(p.anchor, None);

    let TracesMode::Search(p) = req(json!({"search": {
        "after": 10, "before": 20, "limit": 1,
        "selections": {"kind": ["SERVER"]}
    }}))
    .mode
    else {
        panic!("search mode expected");
    };
    assert_eq!((p.after, p.before), (10, 20));
    assert_eq!(p.limit, 1);
    assert_eq!(p.selections["kind"], vec!["SERVER"]);

    let err = req_err(json!({"search": {"last": 5}}));
    assert!(
        err.contains("invalid search selector") && err.contains("unknown field"),
        "the old `last` name is retired: {err}"
    );
}

#[test]
fn windowed_mode_objects_carry_their_own_window() {
    let TracesMode::Overview(p) = req(json!({"overview": {"after": 1, "before": 2}})).mode else {
        panic!("overview mode expected");
    };
    assert_eq!((p.after, p.before), (1, 2));

    // Omitted windows keep the 0 = "unspecified" sentinel — the
    // adapter's resolve_window defaults are untouched.
    let TracesMode::Overview(p) = req(json!({"overview": {}})).mode else {
        panic!("overview mode expected");
    };
    assert_eq!((p.after, p.before), (0, 0));
}

/// The standalone grid takes the page's `selections` grammar and
/// nothing else of the page's filters: the duration bounds are unknown
/// fields here (the grid never applies them).
#[test]
fn overview_params_take_selections_but_no_duration_bounds() {
    let TracesMode::Overview(p) =
        req(json!({"overview": {"selections": {"name": ["GET", "POST"]}}})).mode
    else {
        panic!("overview mode expected");
    };
    assert_eq!(p.selections["name"], vec!["GET", "POST"]);
    let TracesMode::Overview(p) = req(json!({"overview": {}})).mode else {
        panic!("overview mode expected");
    };
    assert!(p.selections.is_empty());
    for field in ["min_trace_duration_ns", "max_trace_duration_ns", "min_duration_ns"] {
        let msg = req_err(json!({"overview": {field: 1}}));
        assert!(msg.contains("unknown field"), "{field}: {msg}");
    }
}

#[test]
fn tenant_rides_beside_any_mode() {
    let r = req(json!({"search": {}, "tenant": "t1"}));
    assert_eq!(r.tenant.as_deref(), Some("t1"));
    let r = req(json!({"info": {}}));
    assert_eq!(r.tenant, None);
}

// ── Info response ───────────────────────────────────────────────────

#[test]
fn info_response_shape_is_pinned() {
    let v = serde_json::to_value(InfoResponse::default()).unwrap();
    assert_eq!(
        v,
        json!({
            "mode": "info",
            "version": 1,
            "status": 200,
            "type": "traces",
            "has_history": true,
            "v": 3,
            "accepted_params": [
                "info", "explore", "values", "trace", "overview", "search", "tenant", "after",
                "before", "last", "anchor", "selections",
                "min_trace_duration_ns", "max_trace_duration_ns", "overview_facets"
            ],
            "required_params": [],
            "help": "Query and visualize OpenTelemetry traces.",
            "sections": ["histogram", "facets", "rows", "fields"],
            "defaults": {
                "filter": {"_role": ["root", "inbound"]},
                "stack": "status_code",
                "window_s": 900,
                "rows_limit": 100,
                "values_limit": 100
            },
            "limits": {
                "rows_page_max": 1000, "top_k_max": 1000, "row_columns_max": 32, "values_max": 1000,
                "trace_ids_max": 100, "facet_values_max": 1000
            },
            "percentiles": {"approximate": true, "max_relative_error": 0.0078125, "label": "≈"},
            "duration_bands": [
                {"label": "<1ms", "min_ns": 0, "max_ns": 999_999},
                {"label": "1-10ms", "min_ns": 1_000_000, "max_ns": 9_999_999},
                {"label": "10-100ms", "min_ns": 10_000_000, "max_ns": 99_999_999},
                {"label": "100ms-1s", "min_ns": 100_000_000, "max_ns": 999_999_999},
                {"label": "1-10s", "min_ns": 1_000_000_000, "max_ns": 9_999_999_999_i64},
                {"label": ">10s", "min_ns": 10_000_000_000_i64, "max_ns": null}
            ],
            "partial_reasons": [
                "source_failure", "remote_unavailable", "cancelled", "legacy_file",
                "stack_field_high_card", "facet_high_card", "facet_value_cap", "groups_cap",
                "live_pass_failed"
            ],
        })
    );
}

#[test]
fn info_bands_follow_the_flattener() {
    let v = serde_json::to_value(InfoResponse::default()).unwrap();
    let bands = v["duration_bands"].as_array().unwrap();
    let labels: Vec<&str> = bands.iter().map(|b| b["label"].as_str().unwrap()).collect();
    assert_eq!(labels, sfsq::traces::explore::DURATION_BAND_LABELS);
    let edges: Vec<i64> = bands[1..]
        .iter()
        .map(|b| b["min_ns"].as_i64().unwrap())
        .collect();
    assert_eq!(edges, sfsq::traces::explore::DURATION_BAND_EDGES_NS);
    for (band, next) in bands.iter().zip(&bands[1..]) {
        assert_eq!(
            band["max_ns"].as_i64().unwrap() + 1,
            next["min_ns"].as_i64().unwrap()
        );
    }
}

#[test]
fn response_envelope_is_untagged() {
    // The Info variant serializes as the bare descriptor object, no
    // enum tag wrapper; the mode field self-describes instead.
    let v = serde_json::to_value(OtelTracesResponse::Info(InfoResponse::default())).unwrap();
    assert!(v.get("version").is_some());
    assert!(v.get("Info").is_none());
    assert_eq!(v.get("mode").and_then(|m| m.as_str()), Some("info"));
}

// ── Status serialization ────────────────────────────────────────────

#[test]
fn complete_status_serializes_as_complete_true() {
    let wire = StatusWire::from(&QueryStatus::Complete);
    assert_eq!(serde_json::to_value(&wire).unwrap(), json!({"complete": true}));
}

#[test]
fn partial_status_serializes_reason_names_deterministically() {
    // Insertion order must not matter — the engine's BTreeSet renders
    // deterministically, and the wire names are pinned snake_case.
    let mut b = StatusBuilder::new();
    b.add(PartialReason::SourceFailure);
    b.add(PartialReason::SizeCap);
    let wire = StatusWire::from(&b.finish());
    assert_eq!(
        serde_json::to_value(&wire).unwrap(),
        json!({"partial": [
            {"reason": "size_cap", "count": 1},
            {"reason": "source_failure", "count": 1}
        ]})
    );
}

#[test]
fn status_partial_with_counts() {
    let mut b = StatusBuilder::new();
    b.add_n(PartialReason::SourceFailure, 2);
    b.of(PartialReason::SourceFailure, 14);
    b.add(PartialReason::FacetHighCard);
    b.detail(PartialReason::FacetHighCard, "attributes.request.id");
    let wire = StatusWire::from(&b.finish());
    let v = serde_json::to_value(&wire).unwrap();
    assert_eq!(
        v,
        json!({"partial": [
            {"reason": "source_failure", "count": 2, "of": 14},
            {"reason": "facet_high_card", "count": 1, "detail": ["attributes.request.id"]}
        ]})
    );
    let back: StatusWire = serde_json::from_value(v).unwrap();
    assert_eq!(back, wire);
    assert!(
        serde_json::from_value::<StatusWire>(json!({"partial": ["source_failure"]})).is_err(),
        "a bare reason name is not a status any more"
    );
}

/// Every engine reason. The exhaustive match fails compilation when the
/// engine gains a reason, so the pins below cannot silently miss it.
fn every_partial_reason() -> Vec<PartialReason> {
    let all = vec![
        PartialReason::SizeCap,
        PartialReason::SourceFailure,
        PartialReason::WorkCeiling,
        PartialReason::Cancelled,
        PartialReason::OverviewCeiling,
        PartialReason::RollupAbsent,
        PartialReason::RemoteUnavailable,
        PartialReason::LegacyFile,
        PartialReason::StackFieldHighCard,
        PartialReason::FacetHighCard,
        PartialReason::FacetValueCap,
        PartialReason::GroupsCap,
        PartialReason::LivePassFailed,
    ];
    for reason in &all {
        match reason {
            PartialReason::SizeCap
            | PartialReason::SourceFailure
            | PartialReason::WorkCeiling
            | PartialReason::Cancelled
            | PartialReason::OverviewCeiling
            | PartialReason::RollupAbsent
            | PartialReason::RemoteUnavailable
            | PartialReason::LegacyFile
            | PartialReason::StackFieldHighCard
            | PartialReason::FacetHighCard
            | PartialReason::FacetValueCap
            | PartialReason::GroupsCap
            | PartialReason::LivePassFailed => {}
        }
    }
    all
}

#[test]
fn every_partial_reason_wire_name_is_pinned() {
    let mut b = StatusBuilder::new();
    for reason in every_partial_reason() {
        b.add(reason);
    }
    let wire = serde_json::to_value(StatusWire::from(&b.finish())).unwrap();
    let names: Vec<&str> = wire["partial"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["reason"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "size_cap",
            "source_failure",
            "work_ceiling",
            "cancelled",
            "overview_ceiling",
            "rollup_absent",
            "remote_unavailable",
            "legacy_file",
            "stack_field_high_card",
            "facet_high_card",
            "facet_value_cap",
            "groups_cap",
            "live_pass_failed"
        ]
    );
}

#[test]
fn every_partial_reason_is_in_the_published_schema() {
    // The published Functions schema closes the reason list; a wire name
    // missing there makes a valid response fail schema validation.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../plugins.d/FUNCTION_UI_SCHEMA.json"
    );
    let schema: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let published: Vec<&str> = schema["definitions"]["traces_status"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|variant| {
            variant["properties"]["partial"]["items"]["properties"]["reason"]["enum"].as_array()
        })
        .flatten()
        .map(|name| name.as_str().unwrap())
        .collect();
    let mut names = Vec::new();
    for reason in every_partial_reason() {
        let wire = serde_json::to_value(PartialReasonWire::from(reason)).unwrap();
        names.push(wire.as_str().unwrap().to_string());
    }
    assert_eq!(published, names, "the schema lists exactly the wire reasons, in order");
}

#[test]
fn overview_facets_knob_parses_and_junk_is_rejected() {
    let facets = |v: serde_json::Value| -> Option<bool> {
        let TracesMode::Overview(p) = req(v).mode else {
            panic!("overview mode expected");
        };
        p.facets
    };
    assert_eq!(facets(json!({"overview": {}})), None);
    assert_eq!(facets(json!({"overview": {"facets": true}})), Some(true));
    assert_eq!(facets(json!({"overview": {"facets": false}})), Some(false));
    assert_eq!(
        facets(json!({"overview": {"facets": null}})),
        None,
        "null means off, not an error (Option<bool> semantics)"
    );
    for body in [
        json!({"overview": {"facets": "yes"}}),
        json!({"overview": {"bogus": 1}}),
    ] {
        let err = req_err(body.clone());
        assert!(err.contains("invalid overview selector"), "for {body}: {err}");
    }
}

#[test]
fn status_wire_round_trips() {
    for status in [
        StatusWire::Complete {
            complete: CompleteTrue,
        },
        StatusWire::Partial {
            partial: vec![
                ReasonWire {
                    reason: PartialReasonWire::SizeCap,
                    count: 1,
                    of: None,
                    detail: vec![],
                },
                ReasonWire {
                    reason: PartialReasonWire::LegacyFile,
                    count: 2,
                    of: Some(9),
                    detail: vec![],
                },
            ],
        },
    ] {
        let v = serde_json::to_value(&status).unwrap();
        let back: StatusWire = serde_json::from_value(v).unwrap();
        assert_eq!(back, status);
    }
}

#[test]
fn complete_false_is_unrepresentable() {
    // `{"complete": false}` means nothing — it must fail to
    // deserialize rather than masquerade as a status.
    assert!(serde_json::from_value::<StatusWire>(json!({"complete": false})).is_err());
}

// ── Explore ─────────────────────────────────────────────────────────

fn explore(v: serde_json::Value) -> ExploreParams {
    let TracesMode::Explore(p) = req(json!({ "explore": v })).mode else {
        panic!("explore mode expected");
    };
    *p
}

#[test]
fn explore_defaults_to_the_last_fifteen_minutes_stacked_by_status() {
    let p = explore(json!({}));
    assert_eq!((p.window.after, p.window.before), (-900, 0));
    assert!(p.filter.is_empty());
    assert_eq!(
        p.histogram,
        Some(HistogramRequest {
            stack: "status_code".to_string(),
            percentiles: true
        })
    );

    let p = explore(json!({"before": 1_700_000_900}));
    assert_eq!((p.window.after, p.window.before), (1_700_000_000, 1_700_000_900));

    let p = explore(json!({
        "after": -3600,
        "filter": {"_role": ["root", "inbound"]},
        "sections": {"histogram": {"stack": "_duration_band", "percentiles": false}}
    }));
    assert_eq!((p.window.after, p.window.before), (-3600, 0));
    assert_eq!(p.filter["_role"], ["root", "inbound"]);
    assert_eq!(
        p.histogram,
        Some(HistogramRequest {
            stack: "_duration_band".to_string(),
            percentiles: false
        })
    );

    assert_eq!(
        explore(json!({})).facets,
        Some(FacetsRequest { fields: None }),
        "omitted sections mean every section"
    );
    let p = explore(json!({"sections": {}}));
    assert!(p.histogram.is_none(), "an empty section list asks for nothing");
    assert!(p.facets.is_none());
    let p = explore(json!({"sections": {"facets": {"fields": ["_role", "name"]}}}));
    assert_eq!(
        p.facets,
        Some(FacetsRequest {
            fields: Some(vec!["_role".to_string(), "name".to_string()])
        })
    );
    assert_eq!(
        explore(json!({"sections": {"facets": {}}})).facets,
        Some(FacetsRequest { fields: None })
    );
}

#[test]
fn explore_windows_resolve_against_now() {
    let relative = explore(json!({"after": -900, "before": -60})).window;
    assert_eq!(relative.resolve(10_000), (9_100, 9_940));
    let absolute = explore(json!({"after": 5_000, "before": 6_000})).window;
    assert_eq!(absolute.resolve(10_000), (5_000, 6_000));
}

#[test]
fn explore_rejects_bad_requests() {
    for (body, needle) in [
        (json!({"explore": {"bogus": 1}}), "unknown field"),
        (json!({"explore": {"sections": {"bogus": {}}}}), "unknown field"),
        (json!({"explore": {"sections": {"histogram": null}}}), "omit it instead"),
        (json!({"explore": {"sections": {"histogram": []}}}), "must be an object"),
        (json!({"explore": {"sections": {"histogram": {"stack": " "}}}}), "`stack` field is empty"),
        (json!({"explore": {"sections": {"histogram": {"bogus": 1}}}}), "unknown field"),
        (json!({"explore": {"filter": {"_role": []}}}), "lists no values"),
        (json!({"explore": {"filter": {"": ["x"]}}}), "field name is empty"),
        (json!({"explore": {"after": -900, "before": 1_700_000_000}}), "both be relative"),
        (json!({"explore": {"after": 0, "before": -60}}), "must be before"),
        (json!({"explore": {"after": 7_000, "before": 6_000}}), "must be before"),
        (json!({"explore": {"sections": {"facets": null}}}), "omit it instead"),
        (json!({"explore": {"sections": {"facets": {"fields": []}}}}), "lists no fields"),
        (json!({"explore": {"sections": {"facets": {"fields": [""]}}}}), "names an empty field"),
        (json!({"explore": {"sections": {"facets": {"bogus": 1}}}}), "unknown field"),
        (json!({"explore": {"selection": {}}}), "not available yet"),
        (json!({"explore": {"text": "  "}}), "`text` is empty"),
        (json!({"explore": {"trace_ids": []}}), "needs 1 to 100 ids"),
        (json!({"explore": {"trace_ids": ["abc"]}}), "trace id"),
        (
            json!({"explore": {"trace_ids": ["00000000000000000000000000000000"]}}),
            "all-zero trace id",
        ),
        (json!({"explore": {"sections": {"groups": {}}}}), "not available yet"),
        (json!({"explore": {"sections": {"fields": {"bogus": 1}}}}), "unknown field"),
        (json!({"explore": {"sections": {"fields": null}}}), "omit it instead"),
        (json!({"explore": {"sections": {"rows": {"order": "oldest"}}}}), "unknown variant"),
        (json!({"explore": {"sections": {"rows": {"direction": "up"}}}}), "unknown variant"),
        (json!({"explore": {"sections": {"rows": {"limit": 0}}}}), "must be 1 to 1000"),
        (json!({"explore": {"sections": {"rows": {"limit": 1001}}}}), "must be 1 to 1000"),
        (json!({"explore": {"sections": {"rows": {"anchor": "12:ab"}}}}), "not a cursor"),
        (
            json!({"explore": {"sections": {"rows": {"order": "slowest", "direction": "older"}}}}),
            "only to the newest order",
        ),
        (json!({"explore": {"sections": {"rows": {"columns": [""]}}}}), "empty column"),
        (
            json!({"explore": {"sections": {"rows": {"columns": ["events.name"]}}}}),
            "not a span column",
        ),
        (
            json!({"explore": {"sections": {"rows": {"columns": ["name", "name"]}}}}),
            "`name` twice",
        ),
        (json!({"explore": {"sections": {"rows": {"bogus": 1}}}}), "unknown field"),
        (json!({"explore": {}, "trace": {"id": "00"}}), "conflicting mode selectors"),
        (json!({"explore": []}), "expected an object"),
    ] {
        let err = req_err(body.clone());
        assert!(err.contains(needle), "for {body}: {err}");
    }
}

#[test]
fn explore_takes_text_and_trace_ids() {
    let p = explore(json!({
        "text": "  order 84 ",
        "trace_ids": ["4BF92F3577B34DA6A3CE929D0E0E4736"]
    }));
    assert_eq!(p.text.as_deref(), Some("order 84"));
    assert_eq!(p.trace_ids.len(), 1);
    assert_eq!(
        p.trace_ids[0].as_bytes()[0],
        0x4b,
        "hex is read case-insensitively"
    );
}

#[test]
fn explore_takes_rows() {
    use sfsq::traces::explore::{RowDirection, RowKey, RowOrder, RowsSpec};
    assert_eq!(
        explore(json!({"sections": {}})).rows,
        None,
        "rows only when asked for"
    );
    assert_eq!(
        explore(json!({})).rows,
        explore(json!({"sections": {"rows": {}}})).rows,
        "omitted sections mean every section"
    );
    assert_eq!(
        explore(json!({"sections": {"rows": {}}})).rows,
        Some(RowsSpec {
            order: RowOrder::Newest {
                anchor: None,
                direction: RowDirection::Older
            },
            limit: 100,
            columns: Vec::new(),
        })
    );
    let anchor = "1758791650123456789:4bf92f3577b34da6a3ce929d0e0e4736:00f067aa0ba902b7";
    let p = explore(json!({"sections": {"rows": {
        "anchor": anchor, "direction": "newer", "limit": 50, "columns": ["attributes.http.route"]
    }}}));
    assert_eq!(
        p.rows,
        Some(RowsSpec {
            order: RowOrder::Newest {
                anchor: RowKey::decode(anchor),
                direction: RowDirection::Newer
            },
            limit: 50,
            columns: vec!["attributes.http.route".to_string()],
        })
    );
    assert_eq!(
        explore(json!({"sections": {"rows": {"order": "slowest", "limit": 10}}})).rows,
        Some(RowsSpec {
            order: RowOrder::Slowest,
            limit: 10,
            columns: Vec::new(),
        })
    );
}

#[test]
fn explore_lists_fields_when_asked_or_by_default() {
    assert!(
        explore(json!({})).fields,
        "omitted sections mean every section"
    );
    assert!(!explore(json!({"sections": {}})).fields);
    assert!(explore(json!({"sections": {"fields": {}}})).fields);
}

fn values(body: serde_json::Value) -> ValuesParams {
    match req(json!({ "values": body })).mode {
        TracesMode::Values(p) => p,
        other => panic!("expected values, got {other:?}"),
    }
}

#[test]
fn values_parse_with_defaults_and_refuse_bad_requests() {
    assert_eq!(
        values(json!({"field": "name"})),
        ValuesParams {
            window: explore(json!({})).window,
            field: "name".to_string(),
            prefix: String::new(),
            limit: 100,
        }
    );
    let p = values(json!({
        "after": 5_000, "before": 6_000, "field": "name", "prefix": "GET", "limit": 1000
    }));
    assert_eq!(
        (p.window.resolve(10_000), p.prefix.as_str(), p.limit),
        ((5_000, 6_000), "GET", 1000)
    );
    for (body, needle) in [
        (json!({"values": {}}), "missing field `field`"),
        (json!({"values": {"field": ""}}), "`field` is empty"),
        (
            json!({"values": {"field": "name", "limit": 0}}),
            "must be 1 to 1000",
        ),
        (
            json!({"values": {"field": "name", "limit": 1001}}),
            "must be 1 to 1000",
        ),
        (
            json!({"values": {"field": "name", "after": 0, "before": -60}}),
            "must be before",
        ),
        (
            json!({"values": {"field": "name", "bogus": 1}}),
            "unknown field",
        ),
        (
            json!({"values": {"field": "name"}, "explore": {}}),
            "conflicting mode selectors",
        ),
    ] {
        let err = req_err(body.clone());
        assert!(err.contains(needle), "for {body}: {err}");
    }
}
