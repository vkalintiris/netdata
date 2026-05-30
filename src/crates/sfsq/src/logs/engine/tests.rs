use super::*;

#[test]
fn pick_histogram_field_honors_requested() {
    // Whatever the query supplies is returned verbatim; the timeline
    // call decides whether it's actually usable.
    assert_eq!(pick_histogram_field(Some("service.name")), "service.name");
    assert_eq!(pick_histogram_field(Some("trace_id")), "trace_id");
}

#[test]
fn pick_histogram_field_defaults_to_severity_text() {
    // No field → OTel canonical default. No file-shape dependence —
    // producers + consumer handle "is this meaningful?"
    assert_eq!(pick_histogram_field(None), "severity_text");
}

#[test]
fn pick_facet_fields_defaults_to_severity_text() {
    // Empty request → exactly the single default facet, regardless of
    // what other low-card fields the file table carries.
    let fields = vec![
        sfst::FieldEntry {
            name: "service".into(),
            cardinality: 5,
            tier: sfst::FieldTier::Low,
        },
        sfst::FieldEntry {
            name: "severity_text".into(),
            cardinality: 2,
            tier: sfst::FieldTier::Low,
        },
    ];
    let picked = pick_facet_fields(&[], &fields);
    assert_eq!(picked, vec!["severity_text".to_string()]);
}

#[test]
fn pick_facet_fields_honors_explicit_request() {
    // Explicit selections are returned as-is; no cardinality cap. A
    // mid-card field the user asked for is kept.
    let fields = vec![sfst::FieldEntry {
        name: "noisy".into(),
        cardinality: 500,
        tier: sfst::FieldTier::Mid,
    }];
    let picked = pick_facet_fields(&["noisy".to_string()], &fields);
    assert_eq!(picked, vec!["noisy".to_string()]);
}

#[test]
fn pick_facet_fields_drops_explicit_high_card_and_unknown() {
    // Explicit requests are still filtered: a high-card field would
    // make facets() error, and an unknown field has no entry.
    let fields = vec![
        sfst::FieldEntry {
            name: "trace_id".into(),
            cardinality: 50_000,
            tier: sfst::FieldTier::High,
        },
        sfst::FieldEntry {
            name: "service".into(),
            cardinality: 5,
            tier: sfst::FieldTier::Low,
        },
    ];
    let picked = pick_facet_fields(
        &[
            "trace_id".to_string(),
            "service".to_string(),
            "ghost".to_string(),
        ],
        &fields,
    );
    assert_eq!(picked, vec!["service".to_string()]);
}
