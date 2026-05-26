//! Container format for split-FST log indexes.
//!
//! An SFST file holds one **primary** [`FstIndex`](fst_index::FstIndex) of
//! low-cardinality `key=value` pairs, zero or more **secondary** chunks
//! (mid-cardinality per-field FSTs, high-cardinality per-field sorted lists),
//! and one stream-log-entries chunk, all keyed by a [`gix_chunk`] TOC for
//! O(1) random access via mmap. Each SFST covers exactly one log stream
//! (one `(service.namespace, service.name)` pair).
//!
//! The complete on-disk specification — chunk layout, ids, encoding,
//! version compatibility, and reader access patterns — lives in
//! `FORMAT.md` alongside this crate. Treat it as the source of truth;
//! the rustdoc here covers only the public API.
//!
//! # Example
//!
//! ```no_run
//! use fst_index::FstIndex;
//! use sfst::BitmapValue;
//! use treight::Bitmap;
//!
//! // Build a minimal primary FST with one `key=value` entry.
//! let bm = BitmapValue { desc: Bitmap::empty(0), data: Vec::new() };
//! let primary: FstIndex<BitmapValue> =
//!     FstIndex::build([("level=info", bm)]).unwrap();
//!
//! // Write
//! let mut writer = sfst::Writer::new();
//! writer.set_primary(sfst::pack(&primary, 1).unwrap());
//! let mut buf = Vec::new();
//! writer.write_to(&mut buf).unwrap();
//!
//! // Read back
//! let reader = sfst::Reader::open(&buf).unwrap();
//! let primary = reader.primary().unwrap();
//! assert!(primary.get(b"level=info").is_some());
//! ```

mod error;
pub mod indexer;
pub mod query;
mod reader;
mod schema;
mod writer;

pub mod registry;

pub use error::{Error, IndexError};
pub use file_registry::ServiceStream;
pub use indexer::{IndexReader, IndexResult, build_and_write, index, index_with_options};
pub use query::{FacetResult, Filter, Timeline, bitmap_value_to_roaring};
pub use reader::{Reader, unpack};
pub use registry::{File, Registry};
pub use schema::{
    BitmapValue, FieldEntry, FieldTier, HighField, Histogram, IdRanges, KvId, Metadata, Summary,
};
pub use writer::{Writer, pack};

// ── Format constants ─────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"SFST";
const VERSION: u32 = 2;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)

const CHUNK_SUMMARY: gix_chunk::Id = *b"SUMR";
const CHUNK_META: gix_chunk::Id = *b"META";
const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";
const CHUNK_TIMS: gix_chunk::Id = *b"TIMS";

/// Minimum number of logs in each stream batch. Files with fewer than
/// `MIN_LOGS_PER_BATCH` total logs use a single batch; otherwise the
/// batch count grows up to [`MAX_STREAM_BATCHES`] so that no batch ever
/// holds fewer than ~`MIN_LOGS_PER_BATCH` entries.
pub const MIN_LOGS_PER_BATCH: u32 = 1024;

/// Hard cap on the number of stream-batch chunks per SFST. Chosen so the
/// per-value batch-membership mask fits in a `u8` (one bit per batch).
pub const MAX_STREAM_BATCHES: u8 = 8;

/// Number of stream-batch (`SB{i}`) chunks in a file with `total_logs`
/// log entries. Both writer and reader call this; the rule is the
/// format invariant, not stored in the file.
pub fn num_stream_batches(total_logs: u32) -> u8 {
    (total_logs / MIN_LOGS_PER_BATCH).clamp(1, MAX_STREAM_BATCHES as u32) as u8
}

/// Logical batch size for a file with `total_logs` log entries. Used by
/// the writer to partition log positions into batches and by the reader
/// to decide which batch a given position belongs to.
///
/// Returns `1` for an empty file (`total_logs == 0`) — there are no
/// positions to partition, but a non-zero divisor lets callers compose
/// the result with integer division without a separate `total_logs == 0`
/// branch.
pub fn stream_batch_size(total_logs: u32) -> u32 {
    if total_logs == 0 {
        return 1;
    }
    total_logs.div_ceil(num_stream_batches(total_logs) as u32)
}

/// Chunk id for the mid-card field FST at `index`. The id encodes the
/// index in its trailing two bytes, big-endian, so each mid-card chunk
/// has a unique 4-byte id of the form `b"MF{hi}{lo}"`.
fn mid_field_id(index: u16) -> gix_chunk::Id {
    [b'M', b'F', (index >> 8) as u8, (index & 0xff) as u8]
}

/// Chunk id for the high-card field sorted list at `index`. Same shape
/// as [`mid_field_id`] but with prefix `b"HF"`.
fn high_field_id(index: u16) -> gix_chunk::Id {
    [b'H', b'F', (index >> 8) as u8, (index & 0xff) as u8]
}

/// Chunk id for the stream-batch chunk at `index` (0..[`MAX_STREAM_BATCHES`]).
/// Encodes the index as a single ASCII digit in the trailing byte, e.g.
/// `b"SB00"` through `b"SB07"`.
fn stream_batch_id(index: u8) -> gix_chunk::Id {
    [b'S', b'B', b'0', b'0' + index]
}

#[cfg(test)]
mod tests {
    use super::*;
    use fst_index::FstIndex;
    use treight::Bitmap;

    fn empty_bitmap() -> BitmapValue {
        BitmapValue {
            desc: Bitmap::empty(0),
            data: Vec::new(),
        }
    }

    fn build_primary(keys: &[&str]) -> FstIndex<BitmapValue> {
        let entries: Vec<(&str, BitmapValue)> = keys.iter().map(|k| (*k, empty_bitmap())).collect();
        FstIndex::build(entries).unwrap()
    }

    fn empty_timestamps() -> Vec<u8> {
        pack(&Vec::<i64>::new(), 1).unwrap()
    }

    fn empty_stream_batch() -> Vec<u8> {
        pack(&Vec::<Vec<KvId>>::new(), 1).unwrap()
    }

    fn sample_summary() -> Summary {
        Summary {
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 1234,
            stream: ServiceStream::new("prod", "api"),
        }
    }

    fn sample_metadata() -> Metadata {
        Metadata {
            histogram: Histogram {
                timestamps: vec![100, 200, 300],
                counts: vec![10, 25, 50],
            },
            id_ranges: IdRanges {
                low_end: KvId(3),
                mid_end: KvId(5),
                high_end: KvId(8),
            },
            fields: Vec::new(),
        }
    }

    #[test]
    fn round_trip_primary_only() {
        let primary = build_primary(&["alpha", "beta", "gamma"]);

        let mut writer = Writer::new();
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.set_timestamps(empty_timestamps());
        writer.add_stream_batch(empty_stream_batch());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(!reader.has_summary());
        assert!(!reader.has_metadata());

        let p = reader.primary().unwrap();
        assert!(p.get(b"alpha").is_some());
        assert!(p.get(b"beta").is_some());
        assert!(p.get(b"gamma").is_some());
        assert!(p.get(b"missing").is_none());
    }

    #[test]
    fn round_trip_summary() {
        let summary = sample_summary();
        let primary = build_primary(&["a"]);

        let mut writer = Writer::new();
        writer.set_summary(pack(&summary, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.set_timestamps(empty_timestamps());
        writer.add_stream_batch(empty_stream_batch());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.has_summary());
        assert!(!reader.has_metadata());
        assert_eq!(reader.summary().unwrap(), summary);
    }

    #[test]
    fn round_trip_metadata() {
        let metadata = sample_metadata();
        let primary = build_primary(&["a", "b"]);

        let mut writer = Writer::new();
        writer.set_metadata(pack(&metadata, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.set_timestamps(empty_timestamps());
        writer.add_stream_batch(empty_stream_batch());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.has_metadata());

        let read = reader.metadata().unwrap();
        assert_eq!(read, &metadata);
    }

    #[test]
    fn round_trip_fields_and_secondary_chunks() {
        // Field table: 1 low, 2 mid, 1 high. Secondary chunks: 2 mid +
        // 1 high + 1 stream-batch.
        let fields = vec![
            FieldEntry {
                name: "level".into(),
                cardinality: 3,
                tier: FieldTier::Low,
            },
            FieldEntry {
                name: "host".into(),
                cardinality: 200,
                tier: FieldTier::Mid,
            },
            FieldEntry {
                name: "pod".into(),
                cardinality: 300,
                tier: FieldTier::Mid,
            },
            FieldEntry {
                name: "trace_id".into(),
                cardinality: 50_000,
                tier: FieldTier::High,
            },
        ];
        let metadata = Metadata {
            histogram: Histogram {
                timestamps: vec![1_700_000_000],
                counts: vec![2],
            },
            id_ranges: IdRanges {
                low_end: KvId(1),
                mid_end: KvId(6),
                high_end: KvId(7),
            },
            fields,
        };

        let primary = build_primary(&["level=info"]);
        let mid_host = build_primary(&["host=h1", "host=h2"]);
        let mid_pod = build_primary(&["pod=p1", "pod=p2", "pod=p3"]);
        // Bit 0 set: the value lives in the single stream batch we emit.
        let high_trace = HighField {
            keys: vec!["trace_id=abc".into()],
            // Bit 0: the value lives in the single stream batch we emit.
            masks: vec![0b0000_0001],
        };
        let stream_entries: Vec<Vec<KvId>> = vec![vec![KvId(0), KvId(1)], vec![KvId(2)]];
        let timestamps: Vec<i64> = vec![1_700_000_000_000_000_000, 1_700_000_000_500_000_000];

        let mut writer = Writer::new();
        writer.set_metadata(pack(&metadata, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        assert_eq!(writer.add_mid_field(pack(&mid_host, 1).unwrap()), 0);
        assert_eq!(writer.add_mid_field(pack(&mid_pod, 1).unwrap()), 1);
        assert_eq!(writer.add_high_field(pack(&high_trace, 1).unwrap()), 0);
        writer.set_timestamps(pack(&timestamps, 1).unwrap());
        assert_eq!(
            writer.add_stream_batch(pack(&stream_entries, 1).unwrap()),
            0
        );

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert_eq!(reader.num_mid().unwrap(), 2);
        assert_eq!(reader.num_high().unwrap(), 1);

        assert_eq!(reader.fields().unwrap().len(), 4);

        // Mid-card chunks.
        let m0 = reader.mid_field(0).unwrap();
        assert!(m0.get(b"host=h1").is_some());
        let m1 = reader.mid_field(1).unwrap();
        assert!(m1.get(b"pod=p2").is_some());

        // High-card chunk: columnar (keys, masks) round-trips.
        let h0 = reader.high_field(0).unwrap();
        assert_eq!(h0, high_trace);

        // Timestamps chunk.
        assert_eq!(reader.timestamps().unwrap(), timestamps);

        // Stream-batch chunk: the only batch (index 0) carries everything.
        let batch = reader.stream_batch(0).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], vec![KvId(0), KvId(1)]);
        // Asking for a non-existent batch yields ChunkNotFound.
        assert!(matches!(
            reader.stream_batch(1),
            Err(Error::ChunkNotFound(1))
        ));
    }

    #[test]
    fn mid_field_out_of_range_errors() {
        let primary = build_primary(&["k"]);
        let mid = build_primary(&["host=h"]);

        let mut writer = Writer::new();
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.add_mid_field(pack(&mid, 1).unwrap());
        writer.set_timestamps(empty_timestamps());
        writer.add_stream_batch(empty_stream_batch());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.mid_field(0).is_ok());
        assert!(matches!(reader.mid_field(1), Err(Error::ChunkNotFound(1))));
    }

    #[test]
    fn error_on_no_timestamps() {
        let primary = build_primary(&["k"]);
        let mut writer = Writer::new();
        writer.set_primary(pack(&primary, 1).unwrap());

        let mut buf = Vec::new();
        assert!(matches!(
            writer.write_to(&mut buf),
            Err(Error::NoTimestamps)
        ));
    }

    #[test]
    fn full_file_round_trip() {
        let summary = sample_summary();
        let mut metadata = sample_metadata();
        metadata.fields = vec![FieldEntry {
            name: "level".into(),
            cardinality: 3,
            tier: FieldTier::Low,
        }];
        let primary = build_primary(&["level=info"]);
        let stream_entries: Vec<Vec<KvId>> = vec![vec![KvId(0)]];
        let timestamps: Vec<i64> = vec![1_700_000_000_000_000_000];

        let mut writer = Writer::new();
        writer.set_summary(pack(&summary, 1).unwrap());
        writer.set_metadata(pack(&metadata, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.set_timestamps(pack(&timestamps, 1).unwrap());
        writer.add_stream_batch(pack(&stream_entries, 1).unwrap());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert_eq!(reader.summary().unwrap(), summary);
        assert_eq!(reader.fields().unwrap().len(), 1);
        assert_eq!(reader.num_mid().unwrap(), 0);
        assert_eq!(reader.num_high().unwrap(), 0);
        assert_eq!(reader.timestamps().unwrap(), timestamps);
        // A small `total_logs` puts everything in a single batch.
        assert_eq!(reader.num_stream_batches().unwrap(), 1);
        assert_eq!(reader.stream_batch(0).unwrap(), stream_entries);
    }

    #[test]
    fn round_trip_multi_batch_stream() {
        // 3072 logs → exactly 3 batches of 1024.
        let total_logs = 3072u32;
        assert_eq!(num_stream_batches(total_logs), 3);
        assert_eq!(stream_batch_size(total_logs), 1024);

        let summary = Summary {
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_071,
            total_logs,
            stream: ServiceStream::new("prod", "api"),
        };
        let metadata = Metadata {
            histogram: Histogram {
                timestamps: vec![1_700_000_000],
                counts: vec![total_logs],
            },
            id_ranges: IdRanges {
                low_end: KvId(1),
                mid_end: KvId(1),
                high_end: KvId(2),
            },
            fields: vec![
                FieldEntry {
                    name: "level".into(),
                    cardinality: 1,
                    tier: FieldTier::Low,
                },
                FieldEntry {
                    name: "trace_id".into(),
                    cardinality: 50_000,
                    tier: FieldTier::High,
                },
            ],
        };

        let primary = build_primary(&["level=info"]);
        // A high-card value that lives only in batches 0 and 2.
        let high_trace = HighField {
            keys: vec!["trace_id=abc".into()],
            masks: vec![0b0000_0101],
        };
        let entries: Vec<Vec<KvId>> = (0..total_logs).map(|i| vec![KvId(i)]).collect();
        let timestamps: Vec<i64> = (0..total_logs as i64).collect();

        let mut writer = Writer::new();
        writer.set_summary(pack(&summary, 1).unwrap());
        writer.set_metadata(pack(&metadata, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.add_high_field(pack(&high_trace, 1).unwrap());
        writer.set_timestamps(pack(&timestamps, 1).unwrap());
        for (i, batch) in entries.chunks(1024).enumerate() {
            // pack accepts &[T] thanks to its ?Sized bound — no Vec
            // materialisation needed.
            assert_eq!(writer.add_stream_batch(pack(batch, 1).unwrap()) as usize, i);
        }

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        // Reader sees three SB chunks, each holding its expected slice.
        let reader = Reader::open(&buf).unwrap();
        assert_eq!(reader.num_stream_batches().unwrap(), 3);
        for i in 0..3u8 {
            let batch = reader.stream_batch(i).unwrap();
            assert_eq!(batch.len(), 1024);
            assert_eq!(batch[0], vec![KvId(u32::from(i) * 1024)]);
            assert_eq!(batch[1023], vec![KvId(u32::from(i) * 1024 + 1023)]);
        }
        // Out-of-range batch fails cleanly.
        assert!(matches!(
            reader.stream_batch(3),
            Err(Error::ChunkNotFound(3))
        ));
        // High-card chunk survives bit-for-bit.
        assert_eq!(reader.high_field(0).unwrap(), high_trace);

        // IndexReader's convenience walks every batch in chronological order.
        let index = IndexReader::open(&buf).unwrap();
        assert_eq!(index.num_stream_batches(), 3);
        let all = index.load_all_stream_entries().unwrap();
        assert_eq!(all.len(), total_logs as usize);
        assert_eq!(all[0], vec![KvId(0)]);
        assert_eq!(all[total_logs as usize - 1], vec![KvId(total_logs - 1)]);
    }

    // ── Query API: evaluate, facets, timeline ────────────────────────

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
}
