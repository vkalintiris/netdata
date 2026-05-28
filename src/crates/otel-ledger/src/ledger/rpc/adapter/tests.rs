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
