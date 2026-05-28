//! Query API tests for [`IndexReader::evaluate`],
//! [`IndexReader::facets`], and [`IndexReader::timeline`].

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
        ],
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

#[test]
fn evaluate_empty_filter_matches_all() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let bm = reader.evaluate(&Filter::new()).unwrap();
    assert_eq!(bm.len(), 6);
}

#[test]
fn evaluate_single_selection() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let bm = reader
        .evaluate(&Filter::new().select("level", "info"))
        .unwrap();
    let positions: Vec<u32> = bm.iter().collect();
    assert_eq!(positions, vec![0, 2, 4]);
}

#[test]
fn evaluate_or_within_field() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // level=info OR level=error → all positions.
    let bm = reader
        .evaluate(
            &Filter::new()
                .select("level", "info")
                .select("level", "error"),
        )
        .unwrap();
    assert_eq!(bm.len(), 6);
}

#[test]
fn evaluate_and_across_fields() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // level=info AND service=worker → only position 4.
    let bm = reader
        .evaluate(
            &Filter::new()
                .select("level", "info")
                .select("service", "worker"),
        )
        .unwrap();
    let positions: Vec<u32> = bm.iter().collect();
    assert_eq!(positions, vec![4]);
}

#[test]
fn evaluate_unknown_field_yields_empty() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Unknown field → no matches in this file (not an error).
    let bm = reader
        .evaluate(&Filter::new().select("nonexistent", "anything"))
        .unwrap();
    assert!(bm.is_empty());
}

#[test]
fn facets_show_all_values_with_self_exclusion() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Selecting `level=info` should NOT hide `level=error` from the
    // `level` facet — that's the whole point of self-exclusion.
    let filter = Filter::new().select("level", "info");
    let results = reader.facets(&["level", "service"], &filter).unwrap();

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
    let err = reader.facets(&["nonexistent"], &Filter::new()).unwrap_err();
    assert!(matches!(err, Error::UnknownField(s) if s == "nonexistent"));
}

#[test]
fn timeline_buckets_match_filter() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // 6 logs spread across 6 seconds. Bucket width = 2 seconds.
    // Total file span = (1_700_000_005 - 1_700_000_000 + 1) seconds = 6s.
    // → 3 buckets of 2s each (positions {0,1}, {2,3}, {4,5}).
    let timeline = reader
        .timeline("level", &Filter::new(), 2 * 1_000_000_000)
        .unwrap();
    assert_eq!(timeline.bucket_width_ns, 2_000_000_000);
    assert_eq!(timeline.buckets.len(), 3);
    // Dimensions are FST-iteration-order: "error", "info".
    assert_eq!(timeline.dimensions, vec!["error", "info"]);
    // Bucket 0 (pos 0-1): info=1, error=1
    assert_eq!(timeline.buckets[0], vec![1, 1]);
    // Bucket 1 (pos 2-3): info=1, error=1
    assert_eq!(timeline.buckets[1], vec![1, 1]);
    // Bucket 2 (pos 4-5): info=1, error=1
    assert_eq!(timeline.buckets[2], vec![1, 1]);
}

#[test]
fn timeline_unset_counts_match_logs_missing_the_field() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    // Every log in the fixture has `level` set, so the unset
    // dimension should be zero across every bucket.
    let t = reader
        .timeline("level", &Filter::new(), 2 * 1_000_000_000)
        .unwrap();
    assert_eq!(t.unset, vec![0, 0, 0]);
    // And the per-bucket dim sums equal the bucket totals (no
    // logs "fall off" the dimensions list — the partition is exact).
    for (bucket, unset) in t.buckets.iter().zip(t.unset.iter()) {
        let dim_sum: u64 = bucket.iter().sum();
        // 2 logs per bucket in this fixture.
        assert_eq!(dim_sum + unset, 2);
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
        .timeline("level", &filter, 6 * 1_000_000_000)
        .unwrap();
    // One bucket covering everything.
    assert_eq!(timeline.buckets.len(), 1);
    // Both dimensions visible.
    assert_eq!(timeline.dimensions, vec!["error", "info"]);
    // Counts reflect the full bitmap (filter excluded its own field).
    assert_eq!(timeline.buckets[0], vec![3, 3]);
}

#[test]
fn timeline_invalid_bucket_width_errors() {
    let data = build_query_fixture();
    let reader = IndexReader::open(&data).unwrap();
    let err = reader.timeline("level", &Filter::new(), 0).unwrap_err();
    assert!(matches!(err, Error::InvalidBucketWidth(0)));
}
