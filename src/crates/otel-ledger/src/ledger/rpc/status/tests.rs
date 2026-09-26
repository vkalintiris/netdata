use super::*;
use serde_json::json;
use sfsq::StatusBuilder;

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
