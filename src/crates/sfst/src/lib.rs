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
//!
//! // Build and pack
//! let fst: FstIndex<u64> = FstIndex::build([("key", 42u64)]).unwrap();
//! let packed = sfst::pack(&fst, 1).unwrap();
//!
//! // Write
//! let mut writer = sfst::Writer::new();
//! writer.set_primary(packed);
//! let mut buf = Vec::new();
//! writer.write_to(&mut buf).unwrap();
//!
//! // Read back
//! let reader = sfst::Reader::open(&buf).unwrap();
//! let fst_read: FstIndex<u64> = reader.primary().unwrap();
//! assert_eq!(fst_read.get(b"key"), Some(&42));
//! ```

mod error;
mod reader;
mod writer;

pub mod registry;

pub use error::Error;
pub use file_registry::StreamEntry;
pub use reader::{Reader, unpack};
pub use registry::{File, Registry};
pub use writer::{Writer, pack};

use serde::{Deserialize, Serialize};

// ── Format constants ─────────────────────────────────────────────

const MAGIC: &[u8; 4] = b"SFST";
const VERSION: u32 = 2;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)

const CHUNK_SUMMARY: gix_chunk::Id = *b"SUMR";
const CHUNK_META: gix_chunk::Id = *b"META";
const CHUNK_FLDS: gix_chunk::Id = *b"FLDS";
const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";

fn hc_chunk_id(index: u16) -> gix_chunk::Id {
    [b'H', b'C', (index >> 8) as u8, (index & 0xff) as u8]
}

// ── FileSummary ──────────────────────────────────────────────────

/// Cheap-to-read summary of an SFST file.
///
/// Stored in its own `SUMR` chunk so the registry can rebuild itself from
/// the file without decompressing the heavier `META` chunk (histogram +
/// id_ranges). All four fields are also held inline on [`File`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSummary {
    pub min_timestamp_s: u32,
    pub max_timestamp_s: u32,
    pub total_logs: u32,
    pub stream: StreamEntry,
}

#[cfg(test)]
mod tests {
    use super::*;
    use fst_index::FstIndex;

    #[test]
    fn round_trip_primary_only() {
        let fst: FstIndex<u64> =
            FstIndex::build([("alpha", 1u64), ("beta", 2), ("gamma", 3)]).unwrap();

        let packed = pack(&fst, 1).unwrap();
        let mut writer = Writer::new();
        writer.set_primary(packed);

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert_eq!(reader.chunk_count(), 0);

        let read: FstIndex<u64> = reader.primary().unwrap();
        assert_eq!(read.get(b"alpha"), Some(&1));
        assert_eq!(read.get(b"beta"), Some(&2));
        assert_eq!(read.get(b"gamma"), Some(&3));
        assert_eq!(read.get(b"missing"), None);
    }

    #[test]
    fn round_trip_with_chunks() {
        let primary: FstIndex<String> = FstIndex::build([
            ("field_a", "low".to_string()),
            ("field_b", "high".to_string()),
        ])
        .unwrap();

        let chunk0: FstIndex<u64> = FstIndex::build([("val1", 100u64), ("val2", 200)]).unwrap();
        let chunk1: FstIndex<u64> = FstIndex::build([("x", 10u64), ("y", 20), ("z", 30)]).unwrap();

        let mut writer = Writer::new();
        writer.set_primary(pack(&primary, 1).unwrap());
        let i0 = writer.add_chunk(pack(&chunk0, 1).unwrap());
        let i1 = writer.add_chunk(pack(&chunk1, 1).unwrap());
        assert_eq!(i0, 0);
        assert_eq!(i1, 1);

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert_eq!(reader.chunk_count(), 2);

        let p: FstIndex<String> = reader.primary().unwrap();
        assert_eq!(p.get(b"field_a"), Some(&"low".to_string()));

        let c0: FstIndex<u64> = reader.chunk(0).unwrap();
        assert_eq!(c0.get(b"val1"), Some(&100));

        let c1: FstIndex<u64> = reader.chunk(1).unwrap();
        assert_eq!(c1.get(b"z"), Some(&30));
    }

    #[test]
    fn round_trip_with_metadata() {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        struct TestMeta {
            name: String,
            count: u32,
        }

        let meta = TestMeta {
            name: "test-file".to_string(),
            count: 42,
        };
        let meta_packed = pack(&meta, 1).unwrap();

        let fst: FstIndex<u64> = FstIndex::build([("a", 1u64), ("b", 2)]).unwrap();
        let fst_packed = pack(&fst, 1).unwrap();

        let mut writer = Writer::new();
        writer.set_metadata(meta_packed);
        writer.set_primary(fst_packed);

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.has_metadata());
        assert_eq!(reader.chunk_count(), 0);

        let meta_read: TestMeta = reader.metadata().unwrap();
        assert_eq!(meta_read, meta);

        let fst_read: FstIndex<u64> = reader.primary().unwrap();
        assert_eq!(fst_read.get(b"a"), Some(&1));
        assert_eq!(fst_read.get(b"b"), Some(&2));
    }

    #[test]
    fn round_trip_metadata_with_chunks() {
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        struct TestMeta {
            fields: Vec<String>,
        }

        let meta = TestMeta {
            fields: vec!["MESSAGE".to_string(), "PRIORITY".to_string()],
        };

        let primary: FstIndex<u64> = FstIndex::build([("low=a", 1u64), ("low=b", 2)]).unwrap();
        let hc0: FstIndex<u64> = FstIndex::build([("val1", 10u64), ("val2", 20)]).unwrap();

        let mut writer = Writer::new();
        writer.set_metadata(pack(&meta, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());
        let idx = writer.add_chunk(pack(&hc0, 1).unwrap());
        assert_eq!(idx, 0);

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.has_metadata());
        assert_eq!(reader.chunk_count(), 1);

        let meta_read: TestMeta = reader.metadata().unwrap();
        assert_eq!(meta_read, meta);

        let p: FstIndex<u64> = reader.primary().unwrap();
        assert_eq!(p.get(b"low=a"), Some(&1));

        let c0: FstIndex<u64> = reader.chunk(0).unwrap();
        assert_eq!(c0.get(b"val1"), Some(&10));
    }

    #[test]
    fn round_trip_summary() {
        let summary = FileSummary {
            min_timestamp_s: 1_700_000_000,
            max_timestamp_s: 1_700_003_600,
            total_logs: 1234,
            stream: StreamEntry::new("prod", "api"),
        };

        let fst: FstIndex<u64> = FstIndex::build([("a", 1u64)]).unwrap();
        let mut writer = Writer::new();
        writer.set_summary(pack(&summary, 1).unwrap());
        writer.set_primary(pack(&fst, 1).unwrap());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.has_summary());
        assert!(!reader.has_metadata());
        assert_eq!(reader.chunk_count(), 0);

        let read: FileSummary = reader.summary().unwrap();
        assert_eq!(read, summary);
    }

    #[test]
    fn round_trip_summary_alongside_metadata() {
        // Mixed file with both SUMR and META plus PRIM.
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        struct HeavyMeta {
            histogram: Vec<u32>,
        }

        let summary = FileSummary {
            min_timestamp_s: 100,
            max_timestamp_s: 200,
            total_logs: 50,
            stream: StreamEntry::new("a", "b"),
        };
        let heavy = HeavyMeta {
            histogram: vec![100, 150, 200],
        };

        let primary: FstIndex<u64> = FstIndex::build([("k", 1u64)]).unwrap();

        let mut writer = Writer::new();
        writer.set_summary(pack(&summary, 1).unwrap());
        writer.set_metadata(pack(&heavy, 1).unwrap());
        writer.set_primary(pack(&primary, 1).unwrap());

        let mut buf = Vec::new();
        writer.write_to(&mut buf).unwrap();

        let reader = Reader::open(&buf).unwrap();
        assert!(reader.has_summary());
        assert!(reader.has_metadata());
        assert_eq!(reader.chunk_count(), 0);

        assert_eq!(reader.summary().unwrap(), summary);
        let h: HeavyMeta = reader.metadata().unwrap();
        assert_eq!(h, heavy);
    }
}
