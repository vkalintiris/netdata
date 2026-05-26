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
mod reader;
mod schema;
mod writer;

pub mod registry;

pub use error::{Error, IndexError};
pub use file_registry::ServiceStream;
pub use indexer::{
    IndexReader, IndexResult, KeyValueId, build_and_write, index, index_with_options,
};
pub use reader::{Reader, unpack};
pub use registry::{File, Registry};
pub use schema::{
    BitmapValue, FieldEntry, FieldTier, Histogram, IdRanges, KvId, Metadata, Summary,
};
pub use writer::{Writer, pack};

// ── Format constants ─────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"SFST";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)

const CHUNK_SUMMARY: gix_chunk::Id = *b"SUMR";
const CHUNK_META: gix_chunk::Id = *b"META";
const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";
const CHUNK_TIMS: gix_chunk::Id = *b"TIMS";
const CHUNK_STREAM: gix_chunk::Id = *b"STRM";

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
        // 1 high + 1 stream-entries.
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
        let high_trace: Vec<(String, BitmapValue)> = vec![("trace_id=abc".into(), empty_bitmap())];
        let stream_entries: Vec<Vec<KvId>> = vec![vec![KvId(0), KvId(1)], vec![KvId(2)]];
        let timestamps: Vec<i64> = vec![1_700_000_000_000_000_000, 1_700_000_000_500_000_000];

        let mut writer = Writer::new();
        writer.set_metadata(pack(&metadata, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        assert_eq!(writer.add_mid_field(pack(&mid_host, 1).unwrap()), 0);
        assert_eq!(writer.add_mid_field(pack(&mid_pod, 1).unwrap()), 1);
        assert_eq!(writer.add_high_field(pack(&high_trace, 1).unwrap()), 0);
        writer.set_timestamps(pack(&timestamps, 1).unwrap());
        writer.set_stream_entries(pack(&stream_entries, 1).unwrap());

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

        // High-card chunk.
        let h0 = reader.high_field(0).unwrap();
        assert_eq!(h0.len(), 1);
        assert_eq!(h0[0].0, "trace_id=abc");

        // Timestamps chunk.
        assert_eq!(reader.timestamps().unwrap(), timestamps);

        // Stream-log-entries chunk.
        let entries = reader.stream_entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], vec![KvId(0), KvId(1)]);
    }

    #[test]
    fn mid_field_out_of_range_errors() {
        let primary = build_primary(&["k"]);
        let mid = build_primary(&["host=h"]);

        let mut writer = Writer::new();
        writer.set_primary(pack(&primary, 1).unwrap());
        writer.add_mid_field(pack(&mid, 1).unwrap());
        writer.set_timestamps(empty_timestamps());

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
        writer.set_stream_entries(pack(&stream_entries, 1).unwrap());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert_eq!(reader.summary().unwrap(), summary);
        assert_eq!(reader.fields().unwrap().len(), 1);
        assert_eq!(reader.num_mid().unwrap(), 0);
        assert_eq!(reader.num_high().unwrap(), 0);
        assert_eq!(reader.timestamps().unwrap(), timestamps);
        assert_eq!(reader.stream_entries().unwrap(), stream_entries);
    }
}
