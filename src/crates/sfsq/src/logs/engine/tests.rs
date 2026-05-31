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
    let fields: sfst::FieldTable = vec![
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
    ]
    .into();
    let picked = pick_facet_fields(&[], &fields);
    assert_eq!(picked, vec!["severity_text".to_string()]);
}

#[test]
fn pick_facet_fields_honors_explicit_request() {
    // Explicit selections are returned as-is; no cardinality cap. A
    // mid-card field the user asked for is kept.
    let fields: sfst::FieldTable = vec![sfst::FieldEntry {
        name: "noisy".into(),
        cardinality: 500,
        tier: sfst::FieldTier::Mid,
    }]
    .into();
    let picked = pick_facet_fields(&["noisy".to_string()], &fields);
    assert_eq!(picked, vec!["noisy".to_string()]);
}

#[test]
fn pick_facet_fields_drops_explicit_high_card_and_unknown() {
    // Explicit requests are still filtered: a high-card field would
    // make facets() error, and an unknown field has no entry.
    let fields: sfst::FieldTable = vec![
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
    ]
    .into();
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
fn merge_sums_matched_and_drops_facet_high_card_in_any_shard() {
    // Shard A computed a `level` facet (Low here); shard B has `level`
    // High and produced no facet for it. The merge sums matched and drops
    // the `level` facet entirely — it's high-card in B, so offering it
    // would be inconsistent with `available_fields`.
    let shard_a = LogsShard {
        matched: 3,
        facets: vec![sfst::FacetResult {
            field: "level".into(),
            values: vec![("info".into(), 3)],
        }],
        timeline: None,
        fields: vec![sfst::FieldEntry {
            name: "level".into(),
            cardinality: 3,
            tier: sfst::FieldTier::Low,
        }]
        .into(),
    };
    let shard_b = LogsShard {
        matched: 2,
        facets: Vec::new(),
        timeline: None,
        fields: vec![sfst::FieldEntry {
            name: "level".into(),
            cardinality: 50_000,
            tier: sfst::FieldTier::High,
        }]
        .into(),
    };

    let merged = LogsShard::merge(vec![shard_a, shard_b]);
    assert_eq!(merged.matched, 5);
    assert!(merged.facets.is_empty(), "high-card `level` facet dropped");
    assert!(merged.fields.get("level").unwrap().is_high_card());
    assert!(merged.timeline.is_none());
}

#[test]
fn merge_empty_is_identity() {
    let merged = LogsShard::merge(Vec::new());
    assert_eq!(merged.matched, 0);
    assert!(merged.facets.is_empty());
    assert!(merged.timeline.is_none());
    assert!(merged.fields.is_empty());
}

fn cursor_at(ts: i64) -> Cursor {
    Cursor {
        timestamp_ns: ts,
        file_seq: 1,
        position: ts as u32,
    }
}

fn timestamps(cursors: &[Cursor]) -> Vec<i64> {
    cursors.iter().map(|c| c.timestamp_ns).collect()
}

#[test]
fn page_merge_backward_keeps_nearest_and_finalize_flags_more() {
    // Backward: closest-to-anchor is the largest (newest) cursor. With
    // limit 2 the bound is 3; merge keeps the nearest 3, finalize takes 2.
    let a = PageShard {
        cursors: vec![cursor_at(50), cursor_at(20)],
        has_opposite: false,
    };
    let b = PageShard {
        cursors: vec![cursor_at(40), cursor_at(30), cursor_at(10)],
        has_opposite: true,
    };

    let merged = PageShard::merge(vec![a, b], Direction::Backward, Some(3));
    assert_eq!(timestamps(&merged.cursors), vec![50, 40, 30]);
    assert!(merged.has_opposite);

    let selected = finalize_page(merged, Direction::Backward, 2);
    // Page is newest-first; backward is already in that order.
    assert_eq!(timestamps(&selected.cursors), vec![50, 40]);
    assert!(selected.has_older, "a 3rd candidate (30) lies beyond the page");
    assert!(selected.has_newer, "has_opposite -> rows newer than the anchor");
}

#[test]
fn page_merge_forward_orders_oldest_first_and_outputs_newest_first() {
    // Forward: closest-to-anchor is the smallest (oldest) cursor; the page
    // is reversed to newest-first for output, and the flags swap sides.
    let a = PageShard {
        cursors: vec![cursor_at(50), cursor_at(20)],
        has_opposite: true,
    };
    let b = PageShard {
        cursors: vec![cursor_at(10), cursor_at(30), cursor_at(40)],
        has_opposite: false,
    };

    let merged = PageShard::merge(vec![a, b], Direction::Forward, Some(3));
    assert_eq!(timestamps(&merged.cursors), vec![10, 20, 30]);
    assert!(merged.has_opposite);

    let selected = finalize_page(merged, Direction::Forward, 2);
    // Nearest 2 are [10, 20] (oldest-first), reversed to newest-first.
    assert_eq!(timestamps(&selected.cursors), vec![20, 10]);
    assert!(selected.has_newer, "a 3rd candidate (30) lies beyond the page");
    assert!(selected.has_older, "has_opposite -> rows older than the anchor");
}
