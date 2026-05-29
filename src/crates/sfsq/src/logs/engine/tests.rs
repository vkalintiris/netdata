use super::*;

#[test]
fn anchor_param_deserializes_string_and_number() {
    let s: LogsRequest = serde_json::from_slice(br#"{"anchor":"100:2:3"}"#).unwrap();
    assert!(matches!(s.anchor, Some(AnchorParam::Cursor(ref c)) if c == "100:2:3"));
    let n: LogsRequest = serde_json::from_slice(br#"{"anchor":1780056601000000}"#).unwrap();
    assert!(matches!(
        n.anchor,
        Some(AnchorParam::TimestampUs(1780056601000000))
    ));
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
