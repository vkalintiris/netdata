use super::*;

#[test]
fn to_query_maps_histogram_and_anchor_forms() {
    // Empty histogram → None; a cursor string parses to Anchor::Cursor.
    let req: OtelLogsRequest =
        serde_json::from_slice(br#"{"histogram":"","anchor":"100:2:3"}"#).unwrap();
    let q = to_query(req);
    assert!(q.histogram_field.is_none());
    assert!(matches!(
        q.anchor,
        Some(Anchor::Cursor(c)) if c.timestamp_ns == 100 && c.file_seq == 2 && c.position == 3
    ));

    // Non-empty histogram is carried through; a bare µs integer becomes
    // an Anchor::Timestamp in nanoseconds.
    let req: OtelLogsRequest =
        serde_json::from_slice(br#"{"histogram":"service","anchor":5000}"#).unwrap();
    let q = to_query(req);
    assert_eq!(q.histogram_field.as_deref(), Some("service"));
    assert!(matches!(q.anchor, Some(Anchor::Timestamp(5_000_000))));

    // A malformed cursor string is dropped → no anchor.
    let req: OtelLogsRequest = serde_json::from_slice(br#"{"anchor":"not-a-cursor"}"#).unwrap();
    let q = to_query(req);
    assert!(q.anchor.is_none());
}

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
        grid: sfst::Grid::new(1_700_000_000 * NS_PER_S, 2 * NS_PER_S, 3),
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
        grid: sfst::Grid::new(0, NS_PER_S, 0),
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
fn available_histograms_enumerates_fields_in_order() {
    // The engine already excludes high-card fields, so the converter is
    // a straight enumeration in field order.
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
    ];
    let av = available_histograms_from_fields(&fields);
    let names: Vec<&str> = av.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["level", "host"]);
    assert_eq!(av[0].order, 0);
    assert_eq!(av[1].order, 1);
}
