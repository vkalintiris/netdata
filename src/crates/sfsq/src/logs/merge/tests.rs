use super::*;

const NS_PER_S: i64 = 1_000_000_000;

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

    let grid = sfst::Grid::new(start, width, 3);
    let a = sfst::Timeline {
        grid,
        dimensions: vec!["error".into(), "info".into()],
        buckets: vec![vec![1, 2], vec![0, 3], vec![4, 0]],
        unset: vec![1, 0, 2],
    };
    let b = sfst::Timeline {
        grid,
        dimensions: vec!["debug".into(), "info".into()],
        buckets: vec![vec![5, 0], vec![1, 1], vec![0, 0]],
        unset: vec![0, 3, 0],
    };

    let merged = merge_timelines(vec![a, b]).unwrap();
    assert_eq!(merged.grid.bucket_start_ns, start);
    assert_eq!(merged.grid.bucket_width_ns, width);
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
    // facets()/timeline() would error if a consumer picked it.
    let a: sfst::FieldTable = vec![
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
    ]
    .into();
    let b: sfst::FieldTable = vec![
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
    ]
    .into();

    let merged = union_field_tables(&[a, b]);
    let names: Vec<&str> = merged.iter().map(|f| f.name.as_str()).collect();
    // `level` dropped; remaining fields sorted by name.
    assert_eq!(names, vec!["host", "service"]);
}

#[test]
fn union_field_tables_keeps_max_cardinality() {
    // Same field across files with different cardinalities: union keeps
    // the max as a conservative estimate.
    let a: sfst::FieldTable = vec![sfst::FieldEntry {
        name: "level".into(),
        cardinality: 3,
        tier: sfst::FieldTier::Low,
    }]
    .into();
    let b: sfst::FieldTable = vec![sfst::FieldEntry {
        name: "level".into(),
        cardinality: 20,
        tier: sfst::FieldTier::Mid,
    }]
    .into();
    let merged = union_field_tables(&[a, b]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].name, "level");
    assert_eq!(merged[0].cardinality, 20);
}
