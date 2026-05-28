use super::*;

#[test]
fn facet_preserves_value_order_and_counts() {
    let f = sfst::FacetResult {
        field: "level".into(),
        values: vec![("error".into(), 3), ("info".into(), 5)],
    };
    let wire = facet_from_sfst(7, &f);
    assert_eq!(wire.id, "level");
    assert_eq!(wire.name, "level");
    assert_eq!(wire.order, 7);
    assert_eq!(wire.options.len(), 2);
    assert_eq!(wire.options[0].id, "error");
    assert_eq!(wire.options[0].count, 3);
    assert_eq!(wire.options[0].order, 0);
    assert_eq!(wire.options[1].id, "info");
    assert_eq!(wire.options[1].count, 5);
    assert_eq!(wire.options[1].order, 1);
}

#[test]
fn facet_with_no_values_yields_empty_options() {
    let f = sfst::FacetResult {
        field: "service".into(),
        values: Vec::new(),
    };
    let wire = facet_from_sfst(0, &f);
    assert!(wire.options.is_empty());
}

#[test]
fn histogram_emits_one_datapoint_per_bucket() {
    // 3 buckets × 2 value dimensions + the "(unset)" trailer.
    let t = sfst::Timeline {
        bucket_start_ns: 1_700_000_000 * NS_PER_S,
        bucket_width_ns: 2 * NS_PER_S,
        dimensions: vec!["error".into(), "info".into()],
        buckets: vec![vec![1, 4], vec![0, 3], vec![2, 2]],
        unset: vec![2, 1, 0],
    };

    let h = histogram_from_sfst("level", &t);

    assert_eq!(h.id, "level");
    assert_eq!(h.chart.view.after, 1_700_000_000);
    assert_eq!(h.chart.view.before, 1_700_000_000 + 6);
    assert_eq!(h.chart.view.update_every, 2);
    assert_eq!(h.chart.view.chart_type, "stackedBar");
    assert_eq!(
        h.chart.view.dimensions.ids,
        vec!["error", "info", "(unset)"]
    );
    assert_eq!(
        h.chart.view.dimensions.units,
        vec!["events", "events", "events"]
    );

    // labels: ["time", value dims..., "(unset)"].
    assert_eq!(
        h.chart.result.labels,
        vec!["time", "error", "info", "(unset)"]
    );

    let dps = &h.chart.result.data;
    assert_eq!(dps.len(), 3);
    // Each DataPoint carries value dims + "(unset)" as the trailing triple.
    assert_eq!(dps[0].items, vec![[1, 0, 0], [4, 0, 0], [2, 0, 0]]);
    assert_eq!(dps[1].items, vec![[0, 0, 0], [3, 0, 0], [1, 0, 0]]);
    assert_eq!(dps[2].items, vec![[2, 0, 0], [2, 0, 0], [0, 0, 0]]);
}

#[test]
fn histogram_with_zero_buckets_still_well_formed() {
    let t = sfst::Timeline {
        bucket_start_ns: 0,
        bucket_width_ns: NS_PER_S,
        dimensions: Vec::new(),
        buckets: Vec::new(),
        unset: Vec::new(),
    };
    let h = histogram_from_sfst("severity_text", &t);
    assert!(h.chart.result.data.is_empty());
    // Even with no value dims, the "(unset)" label is part of the
    // dimension list — that's the legacy wire shape's invariant
    // (result.labels = ["time"] + value-dims + ["(unset)"]).
    assert_eq!(h.chart.result.labels, vec!["time", "(unset)"]);
    assert_eq!(h.chart.view.dimensions.ids, vec!["(unset)"]);
}

// ── Merge helper tests ─────────────────────────────────────────────

#[test]
fn merge_facet_results_unions_fields_and_sums_counts() {
    // File A: level={info:3, error:1}, service={api:4}
    // File B: level={info:2, warn:5}, host={a:1}
    // Merged: level={error:1, info:5, warn:5}, service={api:4}, host={a:1}
    let file_a = vec![
        sfst::FacetResult {
            field: "level".into(),
            values: vec![("info".into(), 3), ("error".into(), 1)],
        },
        sfst::FacetResult {
            field: "service".into(),
            values: vec![("api".into(), 4)],
        },
    ];
    let file_b = vec![
        sfst::FacetResult {
            field: "level".into(),
            values: vec![("info".into(), 2), ("warn".into(), 5)],
        },
        sfst::FacetResult {
            field: "host".into(),
            values: vec![("a".into(), 1)],
        },
    ];

    let merged = merge_facet_results(vec![file_a, file_b]);

    // Output fields sorted lexicographically by BTreeMap iteration.
    let field_names: Vec<&str> = merged.iter().map(|f| f.field.as_str()).collect();
    assert_eq!(field_names, vec!["host", "level", "service"]);

    let level = merged.iter().find(|f| f.field == "level").unwrap();
    assert_eq!(
        level.values,
        vec![("error".into(), 1), ("info".into(), 5), ("warn".into(), 5)]
    );

    let svc = merged.iter().find(|f| f.field == "service").unwrap();
    assert_eq!(svc.values, vec![("api".into(), 4)]);

    let host = merged.iter().find(|f| f.field == "host").unwrap();
    assert_eq!(host.values, vec![("a".into(), 1)]);
}

#[test]
fn merge_facet_results_empty_input_yields_empty() {
    let merged = merge_facet_results(Vec::new());
    assert!(merged.is_empty());
}

#[test]
fn merge_timelines_unions_dimensions_and_sums_buckets() {
    // Same grid (3 buckets × 2s starting at 100s).
    // File A dims [error, info], file B dims [debug, info].
    // Merged dims [debug, error, info] (BTreeSet order).
    let start = 100 * NS_PER_S;
    let width = 2 * NS_PER_S;

    let a = sfst::Timeline {
        bucket_start_ns: start,
        bucket_width_ns: width,
        dimensions: vec!["error".into(), "info".into()],
        buckets: vec![vec![1, 2], vec![0, 3], vec![4, 0]],
        unset: vec![1, 0, 2],
    };
    let b = sfst::Timeline {
        bucket_start_ns: start,
        bucket_width_ns: width,
        dimensions: vec!["debug".into(), "info".into()],
        buckets: vec![vec![5, 0], vec![1, 1], vec![0, 0]],
        unset: vec![0, 3, 0],
    };

    let merged = merge_timelines(vec![a, b]).unwrap();
    assert_eq!(merged.bucket_start_ns, start);
    assert_eq!(merged.bucket_width_ns, width);
    assert_eq!(merged.dimensions, vec!["debug", "error", "info"]);

    // Bucket 0: a[error=1, info=2], b[debug=5, info=0]
    //         → merged[debug=5, error=1, info=2]
    assert_eq!(merged.buckets[0], vec![5, 1, 2]);
    // Bucket 1: a[error=0, info=3], b[debug=1, info=1]
    //         → merged[debug=1, error=0, info=4]
    assert_eq!(merged.buckets[1], vec![1, 0, 4]);
    // Bucket 2: a[error=4, info=0], b[debug=0, info=0]
    //         → merged[debug=0, error=4, info=0]
    assert_eq!(merged.buckets[2], vec![0, 4, 0]);

    // unset sums bucket-wise.
    assert_eq!(merged.unset, vec![1, 3, 2]);
}

#[test]
fn merge_timelines_empty_input_yields_none() {
    assert!(merge_timelines(Vec::new()).is_none());
}

#[test]
fn union_field_tables_drops_field_high_in_any_file() {
    // File A: `level` is Low (card 3); File B: `level` is High (card 50k).
    // Merged should DROP `level` entirely — it's high-card in B, so
    // facets()/timeline() would error if the user picked it.
    let a = vec![
        sfst::FieldEntry {
            name: "level".into(),
            cardinality: 3,
            tier: sfst::FieldTier::Low,
        },
        sfst::FieldEntry {
            name: "service".into(),
            cardinality: 5,
            tier: sfst::FieldTier::Low,
        },
    ];
    let b = vec![
        sfst::FieldEntry {
            name: "level".into(),
            cardinality: 50_000,
            tier: sfst::FieldTier::High,
        },
        sfst::FieldEntry {
            name: "host".into(),
            cardinality: 10,
            tier: sfst::FieldTier::Low,
        },
    ];

    let merged = union_field_tables(&[a.as_slice(), b.as_slice()]);
    let names: Vec<&str> = merged.iter().map(|f| f.name.as_str()).collect();
    // `level` dropped; remaining fields sorted by name.
    assert_eq!(names, vec!["host", "service"]);
}

#[test]
fn union_field_tables_keeps_max_cardinality() {
    // Same field across files with different cardinalities: union
    // keeps the max as a conservative estimate.
    let a = vec![sfst::FieldEntry {
        name: "level".into(),
        cardinality: 3,
        tier: sfst::FieldTier::Low,
    }];
    let b = vec![sfst::FieldEntry {
        name: "level".into(),
        cardinality: 20,
        tier: sfst::FieldTier::Mid,
    }];
    let merged = union_field_tables(&[a.as_slice(), b.as_slice()]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "level");
    assert_eq!(merged[0].cardinality, 20);
    // Tier of the output is functionally irrelevant once we've kept
    // the field: `available_histograms_from_fields` only checks for
    // `FieldTier::High`, and `ever_high` already gated that out.
}

#[test]
fn available_histograms_drops_high_card_fields() {
    let fields = vec![
        sfst::FieldEntry {
            name: "level".into(),
            cardinality: 3,
            tier: sfst::FieldTier::Low,
        },
        sfst::FieldEntry {
            name: "host".into(),
            cardinality: 200,
            tier: sfst::FieldTier::Mid,
        },
        sfst::FieldEntry {
            name: "trace_id".into(),
            cardinality: 50_000,
            tier: sfst::FieldTier::High,
        },
    ];
    let av = available_histograms_from_fields(&fields);
    let names: Vec<&str> = av.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["level", "host"]);
    assert_eq!(av[0].order, 0);
    assert_eq!(av[1].order, 1);
}
