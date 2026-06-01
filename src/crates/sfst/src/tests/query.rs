//! Query API tests for [`IndexReader::matched_count`],
//! [`IndexReader::matched_positions`], [`IndexReader::facets`], and
//! [`IndexReader::timeline`].

use crate::*;

/// Synthetic SFST for query tests. 6 logs, 1 second apart.
///
/// `level` (low-card): `info` at positions 0, 2, 4; `error` at 1, 3, 5.
/// `service` (low-card): `api` at 0, 1, 2; `worker` at 3, 4, 5.
fn build_query_fixture() -> Vec<u8> {
    let mut data = Vec::new();
    let lvl_info = treight::Bitmap::from_sorted_iter([0, 2, 4].into_iter(), 6, &mut data);
    let lvl_info_data = std::mem::take(&mut data);
    let lvl_error = treight::Bitmap::from_sorted_iter([1, 3, 5].into_iter(), 6, &mut data);
    let lvl_error_data = std::mem::take(&mut data);
    let svc_api = treight::Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 6, &mut data);
    let svc_api_data = std::mem::take(&mut data);
    let svc_worker = treight::Bitmap::from_sorted_iter([3, 4, 5].into_iter(), 6, &mut data);
    let svc_worker_data = data;

    // FST iteration order is lexicographic.
    // KvId 0=level=error, 1=level=info, 2=service=api, 3=service=worker.
    let primary_entries: Vec<(&str, BitmapValue)> = vec![
        (
            "level=error",
            BitmapValue {
                desc: lvl_error,
                data: lvl_error_data,
            },
        ),
        (
            "level=info",
            BitmapValue {
                desc: lvl_info,
                data: lvl_info_data,
            },
        ),
        (
            "service=api",
            BitmapValue {
                desc: svc_api,
                data: svc_api_data,
            },
        ),
        (
            "service=worker",
            BitmapValue {
                desc: svc_worker,
                data: svc_worker_data,
            },
        ),
    ];
    let primary: fst_index::FstIndex<BitmapValue> =
        fst_index::FstIndex::build(primary_entries).unwrap();

    // Spread across 6 seconds for predictable bucketing.
    let summary = Summary {
        min_timestamp_s: 1_700_000_000,
        max_timestamp_s: 1_700_000_005,
        total_logs: 6,
        stream: ServiceStream::new("ns", "svc"),
    };
    let metadata = Metadata {
        histogram: Histogram {
            timestamps: vec![1_700_000_000],
            counts: vec![6],
        },
        id_ranges: IdRanges {
            low_end: KvId(4),
            mid_end: KvId(4),
            high_end: KvId(4),
        },
        fields: vec![
            FieldEntry {
                name: "level".into(),
                cardinality: 2,
                tier: FieldTier::Low,
            },
            FieldEntry {
                name: "service".into(),
                cardinality: 2,
                tier: FieldTier::Low,
            },
        ]
        .into(),
    };
    let timestamps: Vec<i64> = (0..6)
        .map(|i| 1_700_000_000i64 * 1_000_000_000 + i * 1_000_000_000)
        .collect();
    // Each log has one level + one service KvId (4 distinct combinations).
    let stream_entries: Vec<Vec<KvId>> = vec![
        vec![KvId(1), KvId(2)], // pos 0: info, api
        vec![KvId(0), KvId(2)], // pos 1: error, api
        vec![KvId(1), KvId(2)], // pos 2: info, api
        vec![KvId(0), KvId(3)], // pos 3: error, worker
        vec![KvId(1), KvId(3)], // pos 4: info, worker
        vec![KvId(0), KvId(3)], // pos 5: error, worker
    ];

    let mut writer = Writer::new();
    writer.set_summary(pack(&summary, 1).unwrap());
    writer.set_metadata(pack(&metadata, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.set_timestamps(pack(&timestamps, 1).unwrap());
    writer.add_stream_batch(pack(&stream_entries, 1).unwrap());
    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();
    buf
}

/// Window covering the fixture's whole log range (all 6 logs).
const FULL_WINDOW: std::ops::Range<i64> = FILE_MIN_NS..(FILE_MIN_NS + 6 * 1_000_000_000);

#[test]
fn matched_empty_filter_matches_all() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let count = reader.matched_count(&Filter::new(), FULL_WINDOW).unwrap();
    assert_eq!(count, 6);
}

#[test]
fn matched_single_selection() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let positions = reader
        .matched_positions(&Filter::new().select("level", "info"), FULL_WINDOW)
        .unwrap();
    assert_eq!(positions, vec![0, 2, 4]);
}

#[test]
fn matched_or_within_field() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // level=info OR level=error → all positions.
    let count = reader
        .matched_count(
            &Filter::new()
                .select("level", "info")
                .select("level", "error"),
            FULL_WINDOW,
        )
        .unwrap();
    assert_eq!(count, 6);
}

#[test]
fn matched_and_across_fields() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // level=info AND service=worker → only position 4.
    let positions = reader
        .matched_positions(
            &Filter::new()
                .select("level", "info")
                .select("service", "worker"),
            FULL_WINDOW,
        )
        .unwrap();
    assert_eq!(positions, vec![4]);
}

#[test]
fn matched_unknown_field_yields_empty() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Unknown field → no matches in this file (not an error).
    let positions = reader
        .matched_positions(&Filter::new().select("nonexistent", "anything"), FULL_WINDOW)
        .unwrap();
    assert!(positions.is_empty());
}

#[test]
fn facets_show_all_values_with_self_exclusion() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Selecting `level=info` should NOT hide `level=error` from the
    // `level` facet — that's the whole point of self-exclusion.
    let filter = Filter::new().select("level", "info");
    // Window spans all 6 logs, so counts are unaffected by clipping.
    let results = reader
        .facets(
            &["level", "service"],
            &filter,
            FILE_MIN_NS..FILE_MIN_NS + 6 * 1_000_000_000,
        )
        .unwrap();

    // `level` facet sees both values (filter excluding `level` is
    // empty → full bitmap).
    let level = results.iter().find(|f| f.field == "level").unwrap();
    let level_counts: std::collections::HashMap<_, _> = level.values.iter().cloned().collect();
    assert_eq!(level_counts.get("info"), Some(&3));
    assert_eq!(level_counts.get("error"), Some(&3));

    // `service` facet sees both values under the filter `level=info`
    // (positions 0, 2, 4): service=api at pos 0, 2; service=worker at pos 4.
    let service = results.iter().find(|f| f.field == "service").unwrap();
    let svc_counts: std::collections::HashMap<_, _> = service.values.iter().cloned().collect();
    assert_eq!(svc_counts.get("api"), Some(&2));
    assert_eq!(svc_counts.get("worker"), Some(&1));
}

#[test]
fn facets_unknown_field_errors() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let err = reader
        .facets(
            &["nonexistent"],
            &Filter::new(),
            FILE_MIN_NS..FILE_MIN_NS + 6 * 1_000_000_000,
        )
        .unwrap_err();
    assert!(matches!(err, Error::UnknownField(s) if s == "nonexistent"));
}

#[test]
fn facets_clip_to_window() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Window covers only positions 2 and 3 (logs at file_min + 2s and
    // + 3s). The fixture has level=info at 2 / error at 3, and
    // service=api at 2 / worker at 3 — so each facet value is counted
    // exactly once, vs. 3 each over the whole file.
    let window = (FILE_MIN_NS + 2 * 1_000_000_000)..(FILE_MIN_NS + 4 * 1_000_000_000);
    let results = reader
        .facets(&["level", "service"], &Filter::new(), window)
        .unwrap();

    let level: std::collections::HashMap<_, _> = results
        .iter()
        .find(|f| f.field == "level")
        .unwrap()
        .values
        .iter()
        .cloned()
        .collect();
    assert_eq!(level.get("info"), Some(&1));
    assert_eq!(level.get("error"), Some(&1));

    let service: std::collections::HashMap<_, _> = results
        .iter()
        .find(|f| f.field == "service")
        .unwrap()
        .values
        .iter()
        .cloned()
        .collect();
    assert_eq!(service.get("api"), Some(&1));
    assert_eq!(service.get("worker"), Some(&1));
}

/// Fixture's file_min_ns — the first log's timestamp.
const FILE_MIN_NS: i64 = 1_700_000_000 * 1_000_000_000;

#[test]
fn timeline_buckets_match_filter() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // 6 logs spread across 6 seconds. Bucket width = 2 seconds.
    // Grid anchored at file_min, 3 buckets covering positions {0,1},
    // {2,3}, {4,5}.
    let timeline = reader
        .timeline(
            "level",
            &Filter::new(),
            Grid::new(FILE_MIN_NS, 2 * 1_000_000_000, 3),
        )
        .unwrap();
    assert_eq!(timeline.grid.bucket_start_ns, FILE_MIN_NS);
    assert_eq!(timeline.grid.bucket_width_ns, 2_000_000_000);
    assert_eq!(timeline.buckets.len(), 3);
    // Dimensions are FST-iteration-order: "error", "info".
    assert_eq!(timeline.dimensions, vec!["error", "info"]);
    // Bucket 0 (pos 0-1): info=1, error=1
    assert_eq!(timeline.buckets[0].counts, vec![1, 1]);
    // Bucket 1 (pos 2-3): info=1, error=1
    assert_eq!(timeline.buckets[1].counts, vec![1, 1]);
    // Bucket 2 (pos 4-5): info=1, error=1
    assert_eq!(timeline.buckets[2].counts, vec![1, 1]);
}

#[test]
fn timeline_unset_counts_match_logs_missing_the_field() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Every log in the fixture has `level` set, so the unset
    // dimension should be zero across every bucket.
    let t = reader
        .timeline(
            "level",
            &Filter::new(),
            Grid::new(FILE_MIN_NS, 2 * 1_000_000_000, 3),
        )
        .unwrap();
    assert!(t.buckets.iter().all(|b| b.unset == 0));
    // And the per-bucket dim sums equal the bucket totals (no
    // logs "fall off" the dimensions list — the partition is exact).
    for bucket in &t.buckets {
        let dim_sum: u64 = bucket.counts.iter().sum();
        // 2 logs per bucket in this fixture.
        assert_eq!(dim_sum + bucket.unset, 2);
    }
}

#[test]
fn timeline_excludes_own_field_from_filter() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Selecting `level=info` shouldn't collapse the `level` timeline
    // to a single dimension.
    let filter = Filter::new().select("level", "info");
    let timeline = reader
        .timeline(
            "level",
            &filter,
            Grid::new(FILE_MIN_NS, 6 * 1_000_000_000, 1),
        )
        .unwrap();
    // One bucket covering everything.
    assert_eq!(timeline.buckets.len(), 1);
    // Both dimensions visible.
    assert_eq!(timeline.dimensions, vec!["error", "info"]);
    // Counts reflect the full bitmap (filter excluded its own field).
    assert_eq!(timeline.buckets[0].counts, vec![3, 3]);
}

#[test]
fn timeline_invalid_bucket_width_errors() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let err = reader
        .timeline("level", &Filter::new(), Grid::new(FILE_MIN_NS, 0, 1))
        .unwrap_err();
    assert!(matches!(err, Error::InvalidBucketWidth(0)));
}

#[test]
fn timeline_grid_before_file_yields_leading_zero_buckets() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Request grid starts 4 seconds before the file's first log and
    // runs 10 buckets of 1 second — so buckets 0..=3 cover times the
    // file has no data (expect zero counts), then buckets 4..=9 cover
    // the file's 6 logs one-per-bucket.
    let grid_start = FILE_MIN_NS - 4 * 1_000_000_000;
    let timeline = reader
        .timeline(
            "level",
            &Filter::new(),
            Grid::new(grid_start, 1_000_000_000, 10),
        )
        .unwrap();
    assert_eq!(timeline.buckets.len(), 10);
    // Leading buckets all zero.
    for i in 0..4 {
        assert_eq!(
            timeline.buckets[i].counts,
            vec![0, 0],
            "bucket {i} should be empty"
        );
        assert_eq!(timeline.buckets[i].unset, 0);
    }
    // Each subsequent bucket holds one log; FST order puts "error"
    // first, then "info". Positions in the fixture: 0=info, 1=error,
    // 2=info, 3=error, 4=info, 5=error.
    let expected = [
        vec![0, 1], // pos 0: info
        vec![1, 0], // pos 1: error
        vec![0, 1], // pos 2: info
        vec![1, 0], // pos 3: error
        vec![0, 1], // pos 4: info
        vec![1, 0], // pos 5: error
    ];
    for (i, exp) in expected.iter().enumerate() {
        assert_eq!(
            timeline.buckets[i + 4].counts,
            *exp,
            "bucket {} mismatch",
            i + 4
        );
    }
}

#[test]
fn materialize_rows_resolves_timestamp_and_attributes() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Fixture positions: 0 = (info, api), 3 = (error, worker); 1s apart
    // starting at FILE_MIN_NS. Stream KvIds resolve via the reverse
    // string table to "level=…"/"service=…" pairs.
    let rows = reader.materialize_rows(&[0, 3]).unwrap();
    assert_eq!(rows.len(), 2);

    assert_eq!(rows[0].timestamp_ns, FILE_MIN_NS);
    assert_eq!(
        rows[0].fields,
        vec![
            ("level".to_string(), "info".to_string()),
            ("service".to_string(), "api".to_string()),
        ]
    );

    assert_eq!(rows[1].timestamp_ns, FILE_MIN_NS + 3 * 1_000_000_000);
    assert_eq!(
        rows[1].fields,
        vec![
            ("level".to_string(), "error".to_string()),
            ("service".to_string(), "worker".to_string()),
        ]
    );
}

#[test]
fn materialize_rows_preserves_position_order_and_skips_out_of_range() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Requested order is honored; position 99 (>= 6 logs) is skipped.
    let rows = reader.materialize_rows(&[5, 99, 1]).unwrap();
    assert_eq!(rows.len(), 2);
    // pos 5 = (error, worker), pos 1 = (error, api).
    assert_eq!(rows[0].timestamp_ns, FILE_MIN_NS + 5 * 1_000_000_000);
    assert_eq!(
        rows[0].fields[0],
        ("level".to_string(), "error".to_string())
    );
    assert_eq!(rows[1].timestamp_ns, FILE_MIN_NS + 1_000_000_000);
    assert_eq!(
        rows[1].fields[1],
        ("service".to_string(), "api".to_string())
    );
}

#[test]
fn materialize_rows_empty_input_yields_empty() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    assert!(reader.materialize_rows(&[]).unwrap().is_empty());
}

#[test]
fn timeline_absent_field_routes_all_logs_to_unset() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // A field not present in this file: no dimensions, and every
    // matching log falls into `unset`. 6 logs over 3 two-second
    // buckets → 2 per bucket, all unset.
    let timeline = reader
        .timeline(
            "nonexistent",
            &Filter::new(),
            Grid::new(FILE_MIN_NS, 2 * 1_000_000_000, 3),
        )
        .unwrap();
    assert!(timeline.dimensions.is_empty());
    // No dimensions → empty `counts`; every matching log lands in `unset`.
    for bucket in &timeline.buckets {
        assert!(bucket.counts.is_empty());
        assert_eq!(bucket.unset, 2);
    }
    assert_eq!(timeline.buckets.len(), 3);
}

/// Fixture with a value dense enough to be stored *complemented* (inverted
/// treight bitmap), mirroring what the writer's `remap_one_bitmap` does for
/// dense values. 6 logs:
///
/// `lvl` (low-card): `hi` at positions 0..=4 (5/6 → stored as the
/// complement `{5}`, inverted), `lo` at position 5.
/// `svc` (low-card): `a` at 0,1,2; `b` at 3,4,5.
fn build_complemented_fixture() -> Vec<u8> {
    let mut data = Vec::new();
    // `lvl=hi` covers 5 of 6 → store the complement {5} as an inverted bitmap.
    let lvl_hi = treight::Bitmap::from_sorted_iter_complemented([5].into_iter(), 6, &mut data);
    let lvl_hi_data = std::mem::take(&mut data);
    let lvl_lo = treight::Bitmap::from_sorted_iter([5].into_iter(), 6, &mut data);
    let lvl_lo_data = std::mem::take(&mut data);
    let svc_a = treight::Bitmap::from_sorted_iter([0, 1, 2].into_iter(), 6, &mut data);
    let svc_a_data = std::mem::take(&mut data);
    let svc_b = treight::Bitmap::from_sorted_iter([3, 4, 5].into_iter(), 6, &mut data);
    let svc_b_data = data;

    // FST iteration order is lexicographic: lvl=hi(0), lvl=lo(1), svc=a(2), svc=b(3).
    let primary_entries: Vec<(&str, BitmapValue)> = vec![
        (
            "lvl=hi",
            BitmapValue {
                desc: lvl_hi,
                data: lvl_hi_data,
            },
        ),
        (
            "lvl=lo",
            BitmapValue {
                desc: lvl_lo,
                data: lvl_lo_data,
            },
        ),
        (
            "svc=a",
            BitmapValue {
                desc: svc_a,
                data: svc_a_data,
            },
        ),
        (
            "svc=b",
            BitmapValue {
                desc: svc_b,
                data: svc_b_data,
            },
        ),
    ];
    let primary: fst_index::FstIndex<BitmapValue> =
        fst_index::FstIndex::build(primary_entries).unwrap();

    let summary = Summary {
        min_timestamp_s: 1_700_000_000,
        max_timestamp_s: 1_700_000_005,
        total_logs: 6,
        stream: ServiceStream::new("ns", "svc"),
    };
    let metadata = Metadata {
        histogram: Histogram {
            timestamps: vec![1_700_000_000],
            counts: vec![6],
        },
        id_ranges: IdRanges {
            low_end: KvId(4),
            mid_end: KvId(4),
            high_end: KvId(4),
        },
        fields: vec![
            FieldEntry {
                name: "lvl".into(),
                cardinality: 2,
                tier: FieldTier::Low,
            },
            FieldEntry {
                name: "svc".into(),
                cardinality: 2,
                tier: FieldTier::Low,
            },
        ]
        .into(),
    };
    let timestamps: Vec<i64> = (0..6)
        .map(|i| 1_700_000_000i64 * 1_000_000_000 + i * 1_000_000_000)
        .collect();
    let stream_entries: Vec<Vec<KvId>> = vec![
        vec![KvId(0), KvId(2)], // pos 0: hi, a
        vec![KvId(0), KvId(2)], // pos 1: hi, a
        vec![KvId(0), KvId(2)], // pos 2: hi, a
        vec![KvId(0), KvId(3)], // pos 3: hi, b
        vec![KvId(0), KvId(3)], // pos 4: hi, b
        vec![KvId(1), KvId(3)], // pos 5: lo, b
    ];

    let mut writer = Writer::new();
    writer.set_summary(pack(&summary, 1).unwrap());
    writer.set_metadata(pack(&metadata, 1).unwrap());
    writer.set_primary(pack(&primary, 1).unwrap());
    writer.set_timestamps(pack(&timestamps, 1).unwrap());
    writer.add_stream_batch(pack(&stream_entries, 1).unwrap());
    let mut buf = Vec::new();
    writer.write_to(&mut buf).unwrap();
    buf
}

#[test]
fn complemented_value_bitmap_counts_correctly() {
    let data = build_complemented_fixture();
    let reader = IndexReader::open(&data).unwrap();

    // `lvl=hi` is stored complemented (inverted). Exercise it through every
    // treight path and confirm the inverted representation is transparent.

    // Fast facet path: `range_cardinality` on the inverted bitmap.
    let facets = reader.facets(&["lvl"], &Filter::new(), FULL_WINDOW).unwrap();
    let lvl: std::collections::HashMap<_, _> = facets
        .iter()
        .find(|f| f.field == "lvl")
        .unwrap()
        .values
        .iter()
        .cloned()
        .collect();
    assert_eq!(lvl.get("hi"), Some(&5));
    assert_eq!(lvl.get("lo"), Some(&1));

    // matched_count / matched_positions: `from_value` on the inverted bitmap,
    // intersected with the (full) window range, then counted / iterated.
    let hi = Filter::new().select("lvl", "hi");
    assert_eq!(reader.matched_count(&hi, FULL_WINDOW).unwrap(), 5);
    assert_eq!(
        reader.matched_positions(&hi, FULL_WINDOW).unwrap(),
        vec![0, 1, 2, 3, 4]
    );

    // Intersection of the inverted bitmap with another field's set.
    let hi_and_a = Filter::new().select("lvl", "hi").select("svc", "a");
    assert_eq!(reader.matched_count(&hi_and_a, FULL_WINDOW).unwrap(), 3);

    // Slow facet path: `value_counts_under` intersects the inverted bitmap
    // with a scope from a *different* filter field.
    let facets = reader
        .facets(&["lvl"], &Filter::new().select("svc", "a"), FULL_WINDOW)
        .unwrap();
    let lvl: std::collections::HashMap<_, _> = facets
        .iter()
        .find(|f| f.field == "lvl")
        .unwrap()
        .values
        .iter()
        .cloned()
        .collect();
    assert_eq!(lvl.get("hi"), Some(&3));
    assert_eq!(lvl.get("lo"), None); // lo ∩ {0,1,2} is empty → omitted
}

#[test]
fn timestamps_lookups() {
    // Ascending ns, with a duplicate at 20.
    let ts = Timestamps::new(vec![10, 20, 20, 40]);
    assert_eq!(ts.len(), 4);
    assert!(!ts.is_empty());

    // position -> time
    assert_eq!(ts.at(0), Some(10));
    assert_eq!(ts.at(3), Some(40));
    assert_eq!(ts.at(4), None); // out of range

    // time -> position window `[start, end)`
    assert_eq!(ts.window(20..40), (1, 3)); // positions 1,2 (both t=20)
    assert_eq!(ts.window(0..100), (0, 4)); // whole file
    assert_eq!(ts.window(100..200), (4, 4)); // past the last log → empty
    assert_eq!(ts.window(15..16), (1, 1)); // between values → empty

    // buckets [0,20), [20,40), [40,60) → positions {0}, {1,2}, {3}
    assert_eq!(
        ts.bucket_ranges(Grid::new(0, 20, 3)),
        vec![(0, 1), (1, 3), (3, 4)]
    );
    // no buckets → no ranges
    assert_eq!(
        ts.bucket_ranges(Grid::new(0, 20, 0)),
        Vec::<(u32, u32)>::new()
    );
}

#[test]
fn filter_from_selections_map() {
    let mut selections: std::collections::HashMap<String, Vec<String>> = Default::default();
    selections.insert("level".into(), vec!["info".into(), "error".into()]);
    selections.insert("service".into(), vec!["api".into()]);
    // A field with no values must be dropped (no constraint), not stored as
    // an empty selection that would collapse the filter to match-nothing.
    selections.insert("cleared".into(), vec![]);

    let filter = Filter::from(&selections);

    let expected = Filter::new()
        .select("level", "info")
        .select("level", "error")
        .select("service", "api");
    assert_eq!(filter, expected);
    assert!(!filter.has_field("cleared"));
}
