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
//! [Primary chunk]             chunk ID: b"PRIM"
//! [Secondary chunk 0]         chunk ID: [b'H', b'C', hi, lo]
//! [Secondary chunk 1]         ...
//! ```
//!
//! # Example
//!
//! ```no_run
//! use fst_index::FstIndex;
//!
//! // Build and pack
//! let fst: FstIndex<u64> = FstIndex::build([("key", 42u64)]).unwrap();
//! let packed = split_fst::pack(&fst, 1).unwrap();
//!
//! // Write
//! let mut writer = split_fst::Writer::new();
//! writer.set_primary(packed);
//! let mut buf = Vec::new();
//! writer.write_to(&mut buf).unwrap();
//!
//! // Read back
//! let reader = split_fst::Reader::open(&buf).unwrap();
//! let fst_read: FstIndex<u64> = reader.primary().unwrap();
//! assert_eq!(fst_read.get(b"key"), Some(&42));
//! ```

use fst_index::FstIndex;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::Write;

const MAGIC: &[u8; 4] = b"SFST";
const VERSION: u32 = 1;
const HEADER_SIZE: usize = 12; // magic(4) + version(4) + num_chunks(4)
const CHUNK_PRIMARY: gix_chunk::Id = *b"PRIM";

fn hc_chunk_id(index: u16) -> gix_chunk::Id {
    [b'H', b'C', (index >> 8) as u8, (index & 0xff) as u8]
}

// ── Error ────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("bincode error: {0}")]
    Bincode(#[from] bincode::Error),

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
    let serialized = bincode::serialize(fst)?;
    zstd::encode_all(&serialized[..], zstd_level).map_err(|e| Error::Zstd(e.to_string()))
}

/// Decompress zstd, then deserialize with bincode into an [`FstIndex`].
pub fn unpack<T: DeserializeOwned>(data: &[u8]) -> Result<FstIndex<T>, Error> {
    let decompressed = zstd::decode_all(data).map_err(|e| Error::Zstd(e.to_string()))?;
    Ok(bincode::deserialize(&decompressed)?)
}

// ── Writer ───────────────────────────────────────────────────────

/// Builds a split-FST file from pre-packed (bincode + zstd) byte blobs.
///
/// Call [`pack()`] to produce the blobs, then feed them here.  Because `pack`
/// is a standalone function, callers can run it in parallel with rayon before
/// collecting results into the writer.
pub struct Writer {
    primary: Option<Vec<u8>>,
    chunks: Vec<Vec<u8>>,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            primary: None,
            chunks: Vec::new(),
        }
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
        let num_chunks = 1 + self.chunks.len(); // primary + secondaries

        // Header
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&(num_chunks as u32).to_le_bytes())?;

        // Plan chunks
        let mut index = gix_chunk::file::Index::for_writing();
        index.plan_chunk(CHUNK_PRIMARY, primary.len() as u64);
        for (i, chunk) in self.chunks.iter().enumerate() {
            index.plan_chunk(hc_chunk_id(i as u16), chunk.len() as u64);
        }

        // Write TOC + data
        let mut chunk_writer = index
            .into_write(&mut *w, HEADER_SIZE)
            .map_err(|e| Error::Toc(format!("{e}")))?;

        // Primary
        let id = chunk_writer.next_chunk().expect("expected primary chunk");
        assert_eq!(id, CHUNK_PRIMARY);
        chunk_writer.write_all(primary)?;

        // Secondaries
        for (i, chunk) in self.chunks.iter().enumerate() {
            let id = chunk_writer.next_chunk().expect("expected HC chunk");
            assert_eq!(id, hc_chunk_id(i as u16));
            chunk_writer.write_all(chunk)?;
        }

        assert!(chunk_writer.next_chunk().is_none(), "unexpected extra chunk");
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

        let num_secondary = if num_chunks > 0 {
            (num_chunks - 1) as u16
        } else {
            0
        };

        Ok(Self {
            data,
            toc,
            num_secondary,
        })
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

        let chunk0: FstIndex<u64> =
            FstIndex::build([("val1", 100u64), ("val2", 200)]).unwrap();
        let chunk1: FstIndex<u64> =
            FstIndex::build([("x", 10u64), ("y", 20), ("z", 30)]).unwrap();

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
        assert!(matches!(Reader::open(data), Err(Error::FileTooShort(4, 12))));
    }
}
