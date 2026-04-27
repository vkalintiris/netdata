//! Container format for split FST indexes.
//!
//! A split-FST file stores one **primary** [`FstIndex`] and zero or more
//! **secondary** (high-cardinality) chunks, each individually compressed with
//! bincode + zstd.  A `gix-chunk` TOC provides O(1) random access to any chunk
//! via mmap.
//!
//! # File layout
//!
//! ```text
//! [Header: 12 bytes]          magic "SFST" + version u32 LE + num_chunks u32 LE
//! [TOC]                       gix-chunk (12 bytes × (num_chunks + 1))
//! [Summary chunk]             chunk ID: b"SUMR" — registry-cheap fields
//! [Metadata chunk]            chunk ID: b"META" — histogram + id_ranges
//! [Fields chunk]              chunk ID: b"FLDS"
//! [Primary chunk]             chunk ID: b"PRIM"
//! [Secondary chunk 0]         chunk ID: [b'H', b'C', hi, lo]
//! [Secondary chunk 1]         ...
//! ```
//!
//! The `SUMR` chunk is written first after the TOC so a reader that only needs
//! summary fields (min/max timestamp, total log count, stream list) can stop
//! after a small read. This is what `Registry::recover` does.
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

pub mod registry;

pub use registry::{File, Registry};

use fst_index::FstIndex;
use serde::{Deserialize, Serialize};
use serde::de::DeserializeOwned;
use std::io::Write;

const MAGIC: &[u8; 4] = b"SFST";
const VERSION: u32 = 2;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)
const CHUNK_SUMMARY: gix_chunk::Id = *b"SUMR";
const CHUNK_META: gix_chunk::Id = *b"META";
const CHUNK_FLDS: gix_chunk::Id = *b"FLDS";
const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";

// ── Summary ──────────────────────────────────────────────────────

/// `(namespace, name)` pair identifying a log stream.
///
/// Each SFST file contains exactly one stream — the WAL writer partitions
/// frames by `ns_hash = hash(namespace, name)`, and the indexer asserts
/// that all data in a single WAL file resolves to one `(namespace, name)`
/// pair. Hash collisions are detected at the ingestor (see
/// `NetdataLogsService`); a colliding WAL file is permanently
/// un-indexable until the operator removes it.
///
/// This is the canonical stream identifier across the codebase — the
/// registry, the catalog, and the indexer all use it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamEntry {
    pub namespace: String,
    pub name: String,
}

impl StreamEntry {
    pub fn new<N: Into<String>, M: Into<String>>(namespace: N, name: M) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
        }
    }
}

/// Cheap-to-read summary of an SFST file.
///
/// Stored in its own `SUMR` chunk so the registry can rebuild itself from
/// the file without decompressing the heavier `META` chunk (histogram +
/// id_ranges). All four fields are also held inline on `sfst::File`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSummary {
    pub min_timestamp_s: u32,
    pub max_timestamp_s: u32,
    pub total_logs: u32,
    pub stream: StreamEntry,
}

fn hc_chunk_id(index: u16) -> gix_chunk::Id {
    [b'H', b'C', (index >> 8) as u8, (index & 0xff) as u8]
}

// ── Error ────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("bincode encode error: {0}")]
    BincodeEncode(#[from] bincode::error::EncodeError),

    #[error("bincode decode error: {0}")]
    BincodeDecode(#[from] bincode::error::DecodeError),

    #[error("zstd error (not std::io): {0}")]
    Zstd(String),

    #[error("invalid magic (expected \"SFST\")")]
    InvalidMagic,

    #[error("unsupported version: {0}")]
    UnsupportedVersion(u32),

    #[error("chunk not found: index {0}")]
    ChunkNotFound(u16),

    #[error("no primary chunk set")]
    NoPrimary,

    #[error("TOC error: {0}")]
    Toc(String),

    #[error("file too short ({0} bytes, need at least {1})")]
    FileTooShort(usize, usize),
}

// ── Pack / Unpack ────────────────────────────────────────────────

/// Serialize an [`FstIndex`] with bincode, then compress with zstd.
pub fn pack<T: Serialize + Clone>(fst: &FstIndex<T>, zstd_level: i32) -> Result<Vec<u8>, Error> {
    let serialized = bincode::serde::encode_to_vec(fst, bincode::config::standard())?;
    zstd::encode_all(&serialized[..], zstd_level).map_err(|e| Error::Zstd(e.to_string()))
}

/// Decompress zstd, then deserialize with bincode into an [`FstIndex`].
pub fn unpack<T: DeserializeOwned>(data: &[u8]) -> Result<FstIndex<T>, Error> {
    let decompressed = zstd::decode_all(data).map_err(|e| Error::Zstd(e.to_string()))?;
    let (val, _len) =
        bincode::serde::decode_from_slice(&decompressed, bincode::config::standard())?;
    Ok(val)
}

// ── Writer ───────────────────────────────────────────────────────

/// Builds a split-FST file from pre-packed (bincode + zstd) byte blobs.
///
/// Call [`pack()`] to produce the blobs, then feed them here.  Because `pack`
/// is a standalone function, callers can run it in parallel with rayon before
/// collecting results into the writer.
pub struct Writer {
    summary: Option<Vec<u8>>,
    metadata: Option<Vec<u8>>,
    fields: Option<Vec<u8>>,
    primary: Option<Vec<u8>>,
    chunks: Vec<Vec<u8>>,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            summary: None,
            metadata: None,
            fields: None,
            primary: None,
            chunks: Vec::new(),
        }
    }

    /// Set the summary chunk (pre-compressed bytes, e.g. bincode + zstd).
    pub fn set_summary(&mut self, packed: Vec<u8>) {
        self.summary = Some(packed);
    }

    /// Set the metadata chunk (pre-compressed bytes, e.g. bincode + zstd).
    pub fn set_metadata(&mut self, packed: Vec<u8>) {
        self.metadata = Some(packed);
    }

    /// Set the fields chunk (pre-compressed bytes, e.g. bincode + zstd).
    pub fn set_fields(&mut self, packed: Vec<u8>) {
        self.fields = Some(packed);
    }

    /// Set the primary chunk (packed bytes).
    pub fn set_primary(&mut self, packed: Vec<u8>) {
        self.primary = Some(packed);
    }

    /// Append a secondary chunk and return its assigned index.
    pub fn add_chunk(&mut self, packed: Vec<u8>) -> u16 {
        let idx = self.chunks.len() as u16;
        self.chunks.push(packed);
        idx
    }

    /// Serialize the entire split-FST file to `w`.
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let primary = self.primary.as_ref().ok_or(Error::NoPrimary)?;
        let num_chunks = self.summary.is_some() as usize
            + self.metadata.is_some() as usize
            + self.fields.is_some() as usize
            + 1 // primary
            + self.chunks.len();

        // Header
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&(num_chunks as u32).to_le_bytes())?;

        // Plan chunks. Order matters: SUMR first so a recovery-only reader
        // can stop after the summary without paging through META/PRIM.
        let mut index = gix_chunk::file::Index::for_writing();
        if let Some(sum) = &self.summary {
            index.plan_chunk(CHUNK_SUMMARY, sum.len() as u64);
        }
        if let Some(meta) = &self.metadata {
            index.plan_chunk(CHUNK_META, meta.len() as u64);
        }
        if let Some(flds) = &self.fields {
            index.plan_chunk(CHUNK_FLDS, flds.len() as u64);
        }
        index.plan_chunk(CHUNK_PRIMARY, primary.len() as u64);
        for (i, chunk) in self.chunks.iter().enumerate() {
            index.plan_chunk(hc_chunk_id(i as u16), chunk.len() as u64);
        }

        // Write TOC + data
        let mut chunk_writer = index
            .into_write(&mut *w, HEADER_SIZE)
            .map_err(|e| Error::Toc(format!("{e}")))?;

        if let Some(sum) = &self.summary {
            let id = chunk_writer.next_chunk().expect("expected SUMR chunk");
            assert_eq!(id, CHUNK_SUMMARY);
            chunk_writer.write_all(sum)?;
        }

        if let Some(meta) = &self.metadata {
            let id = chunk_writer.next_chunk().expect("expected META chunk");
            assert_eq!(id, CHUNK_META);
            chunk_writer.write_all(meta)?;
        }

        if let Some(flds) = &self.fields {
            let id = chunk_writer.next_chunk().expect("expected FLDS chunk");
            assert_eq!(id, CHUNK_FLDS);
            chunk_writer.write_all(flds)?;
        }

        let id = chunk_writer.next_chunk().expect("expected primary chunk");
        assert_eq!(id, CHUNK_PRIMARY);
        chunk_writer.write_all(primary)?;

        for (i, chunk) in self.chunks.iter().enumerate() {
            let id = chunk_writer.next_chunk().expect("expected HC chunk");
            assert_eq!(id, hc_chunk_id(i as u16));
            chunk_writer.write_all(chunk)?;
        }

        assert!(
            chunk_writer.next_chunk().is_none(),
            "unexpected extra chunk"
        );
        chunk_writer.into_inner();
        w.flush()?;
        Ok(())
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

/// Serialize a value with bincode, then compress with zstd.
pub fn pack_metadata<T: Serialize>(value: &T, zstd_level: i32) -> Result<Vec<u8>, Error> {
    let serialized = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
    zstd::encode_all(&serialized[..], zstd_level).map_err(|e| Error::Zstd(e.to_string()))
}

/// Decompress zstd, then deserialize with bincode.
pub fn unpack_metadata<T: DeserializeOwned>(data: &[u8]) -> Result<T, Error> {
    let decompressed = zstd::decode_all(data).map_err(|e| Error::Zstd(e.to_string()))?;
    let (val, _len) =
        bincode::serde::decode_from_slice(&decompressed, bincode::config::standard())?;
    Ok(val)
}

// ── Reader ───────────────────────────────────────────────────────

/// Zero-copy reader over a memory-mapped (or in-memory) split-FST file.
///
/// Decompression happens lazily when [`primary()`](Reader::primary),
/// [`chunk()`](Reader::chunk), or their `_raw` variants are called.
pub struct Reader<'a> {
    data: &'a [u8],
    toc: gix_chunk::file::Index,
    num_secondary: u16,
}

impl<'a> Reader<'a> {
    /// Open a split-FST file from a byte slice (typically an mmap).
    pub fn open(data: &'a [u8]) -> Result<Self, Error> {
        if data.len() < HEADER_SIZE {
            return Err(Error::FileTooShort(data.len(), HEADER_SIZE));
        }

        if &data[0..4] != MAGIC {
            return Err(Error::InvalidMagic);
        }

        let version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }

        let num_chunks = u32::from_le_bytes(data[8..12].try_into().unwrap());
        let toc = gix_chunk::file::Index::from_bytes(data, HEADER_SIZE, num_chunks)
            .map_err(|e| Error::Toc(format!("{e}")))?;

        // Determine how many non-secondary chunks exist (SUMR? + META? + FLDS? + PRIM)
        let has_summary = toc.data_by_id(data, CHUNK_SUMMARY).is_ok();
        let has_meta = toc.data_by_id(data, CHUNK_META).is_ok();
        let has_flds = toc.data_by_id(data, CHUNK_FLDS).is_ok();
        let non_secondary = has_summary as u32 + has_meta as u32 + has_flds as u32 + 1;
        let num_secondary = num_chunks.saturating_sub(non_secondary) as u16;

        Ok(Self {
            data,
            toc,
            num_secondary,
        })
    }

    /// Decompress and deserialize the summary chunk.
    pub fn summary(&self) -> Result<FileSummary, Error> {
        unpack_metadata(self.summary_raw()?)
    }

    /// Raw compressed bytes of the summary chunk.
    pub fn summary_raw(&self) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, CHUNK_SUMMARY)
            .map_err(|e| Error::Toc(format!("{e}")))
    }

    /// Whether a summary chunk is present.
    pub fn has_summary(&self) -> bool {
        self.toc.data_by_id(self.data, CHUNK_SUMMARY).is_ok()
    }

    /// Decompress and deserialize the metadata chunk.
    pub fn metadata<T: DeserializeOwned>(&self) -> Result<T, Error> {
        unpack_metadata(self.metadata_raw()?)
    }

    /// Raw compressed bytes of the metadata chunk.
    pub fn metadata_raw(&self) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, CHUNK_META)
            .map_err(|e| Error::Toc(format!("{e}")))
    }

    /// Whether a metadata chunk is present.
    pub fn has_metadata(&self) -> bool {
        self.toc.data_by_id(self.data, CHUNK_META).is_ok()
    }

    /// Decompress and deserialize the fields chunk.
    pub fn fields<T: DeserializeOwned>(&self) -> Result<T, Error> {
        unpack_metadata(self.fields_raw()?)
    }

    /// Raw compressed bytes of the fields chunk.
    pub fn fields_raw(&self) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, CHUNK_FLDS)
            .map_err(|e| Error::Toc(format!("{e}")))
    }

    /// Whether a fields chunk is present.
    pub fn has_fields(&self) -> bool {
        self.toc.data_by_id(self.data, CHUNK_FLDS).is_ok()
    }

    /// Decompress and deserialize the primary chunk.
    pub fn primary<P: DeserializeOwned>(&self) -> Result<FstIndex<P>, Error> {
        unpack(self.primary_raw()?)
    }

    /// Decompress and deserialize a secondary chunk by index.
    pub fn chunk<S: DeserializeOwned>(&self, index: u16) -> Result<FstIndex<S>, Error> {
        unpack(self.chunk_raw(index)?)
    }

    /// Raw compressed bytes of the primary chunk.
    pub fn primary_raw(&self) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, CHUNK_PRIMARY)
            .map_err(|e| Error::Toc(format!("{e}")))
    }

    /// Raw compressed bytes of a secondary chunk.
    pub fn chunk_raw(&self, index: u16) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, hc_chunk_id(index))
            .map_err(|_| Error::ChunkNotFound(index))
    }

    /// Number of secondary (high-cardinality) chunks.
    pub fn chunk_count(&self) -> u16 {
        self.num_secondary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let meta_packed = pack_metadata(&meta, 1).unwrap();

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
        writer.set_metadata(pack_metadata(&meta, 1).unwrap());
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
        writer.set_summary(pack_metadata(&summary, 1).unwrap());
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
        writer.set_summary(pack_metadata(&summary, 1).unwrap());
        writer.set_metadata(pack_metadata(&heavy, 1).unwrap());
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

    #[test]
    fn error_on_no_primary() {
        let writer = Writer::new();
        let mut buf = Vec::new();
        assert!(writer.write_to(&mut buf).is_err());
    }

    #[test]
    fn error_on_bad_magic() {
        let data = b"BADXxxxxxxxxxxxxxxxxxxxxxxxxxxxx";
        assert!(matches!(Reader::open(data), Err(Error::InvalidMagic)));
    }

    #[test]
    fn error_on_short_file() {
        let data = b"SFST";
        assert!(matches!(
            Reader::open(data),
            Err(Error::FileTooShort(4, 12))
        ));
    }
}
