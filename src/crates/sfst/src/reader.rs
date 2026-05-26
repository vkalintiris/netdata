//! SFST format reader.
//!
//! [`Reader`] opens a byte slice (typically an mmap) and exposes the
//! log-index file's chunks as typed values. Decompression and
//! deserialization happen lazily — `open` only parses the header and
//! TOC. The metadata chunk is cached on first access since it carries
//! the field table needed to bucket secondary chunks into mid/high
//! subtypes.

use std::cell::OnceCell;

use fst_index::FstIndex;
use serde::de::DeserializeOwned;

use crate::{
    BitmapValue, CHUNK_META, CHUNK_PRIMARY, CHUNK_STREAM, CHUNK_SUMMARY, CHUNK_TIMS, Error,
    FieldEntry, FieldTier, HEADER_SIZE, KvId, MAGIC, Metadata, Summary, VERSION, high_field_id,
    mid_field_id,
};

/// Decompress zstd, then deserialize with bincode.
pub fn unpack<T: DeserializeOwned>(data: &[u8]) -> Result<T, Error> {
    let decompressed = zstd::decode_all(data).map_err(|e| Error::Zstd(e.to_string()))?;
    let (val, _len) =
        bincode::serde::decode_from_slice(&decompressed, bincode::config::standard())?;
    Ok(val)
}

/// Zero-copy reader over a memory-mapped (or in-memory) SFST file.
///
/// `open` parses only the header and TOC. Typed accessors decode their
/// chunks on demand. The [`Metadata`] chunk (histogram, id ranges,
/// field table) is cached after first access so the bucketing of
/// secondary chunks doesn't repeatedly decompress META.
pub struct Reader<'a> {
    data: &'a [u8],
    toc: gix_chunk::file::Index,
    /// Lazily-decoded META payload. Populated on first call to any
    /// method that needs it (`metadata`, `fields`, `num_mid`,
    /// `num_high`).
    metadata: OnceCell<Metadata>,
}

impl<'a> Reader<'a> {
    /// Open an SFST file from a byte slice (typically an mmap).
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
        // Defense in depth: a corrupted header claiming u32::MAX chunks
        // would lead gix-chunk to attempt ~51 GiB of TOC allocation.
        // Each TOC entry is at least 12 bytes, so the on-disk body
        // bounds the legal value.
        let max_chunks = data.len().saturating_sub(HEADER_SIZE) / 12;
        if num_chunks as usize > max_chunks {
            return Err(Error::Toc(format!(
                "num_chunks ({num_chunks}) exceeds plausible maximum ({max_chunks})"
            )));
        }
        let toc = gix_chunk::file::Index::from_bytes(data, HEADER_SIZE, num_chunks)
            .map_err(|e| Error::Toc(format!("{e}")))?;

        Ok(Self {
            data,
            toc,
            metadata: OnceCell::new(),
        })
    }

    // ── SUMR ─────────────────────────────────────────────────────────

    /// Decompress and deserialize the summary chunk.
    pub fn summary(&self) -> Result<Summary, Error> {
        unpack(self.summary_raw()?)
    }

    /// Raw compressed bytes of the summary chunk.
    pub fn summary_raw(&self) -> Result<&'a [u8], Error> {
        self.chunk_raw_by_id(CHUNK_SUMMARY)
    }

    /// Whether a summary chunk is present.
    pub fn has_summary(&self) -> bool {
        self.toc.data_by_id(self.data, CHUNK_SUMMARY).is_ok()
    }

    // ── META ─────────────────────────────────────────────────────────

    /// Index metadata (histogram + id ranges + field table). Decoded on
    /// first access; cached for the lifetime of this `Reader`.
    pub fn metadata(&self) -> Result<&Metadata, Error> {
        if let Some(m) = self.metadata.get() {
            return Ok(m);
        }
        let decoded = unpack::<Metadata>(self.metadata_raw()?)?;
        Ok(self.metadata.get_or_init(|| decoded))
    }

    /// Raw compressed bytes of the metadata chunk.
    pub fn metadata_raw(&self) -> Result<&'a [u8], Error> {
        self.chunk_raw_by_id(CHUNK_META)
    }

    /// Whether a metadata chunk is present.
    pub fn has_metadata(&self) -> bool {
        self.toc.data_by_id(self.data, CHUNK_META).is_ok()
    }

    /// Field table — convenience accessor for `metadata().fields`.
    pub fn fields(&self) -> Result<&[FieldEntry], Error> {
        Ok(self.metadata()?.fields.as_slice())
    }

    /// Number of mid-cardinality fields (one secondary chunk per mid
    /// field, sitting at positions `0..num_mid`).
    pub fn num_mid(&self) -> Result<u16, Error> {
        let count = self
            .metadata()?
            .fields
            .iter()
            .filter(|f| f.tier == FieldTier::Mid)
            .count();
        Ok(u16::try_from(count).expect("mid-card field count exceeds u16::MAX"))
    }

    /// Number of high-cardinality fields (one secondary chunk per high
    /// field, sitting at positions `num_mid..num_mid + num_high`).
    pub fn num_high(&self) -> Result<u16, Error> {
        let count = self
            .metadata()?
            .fields
            .iter()
            .filter(|f| f.tier == FieldTier::High)
            .count();
        Ok(u16::try_from(count).expect("high-card field count exceeds u16::MAX"))
    }

    // ── PRIM ─────────────────────────────────────────────────────────

    /// Decompress and deserialize the primary FST.
    pub fn primary(&self) -> Result<FstIndex<BitmapValue>, Error> {
        unpack(self.primary_raw()?)
    }

    /// Raw compressed bytes of the primary chunk.
    pub fn primary_raw(&self) -> Result<&'a [u8], Error> {
        self.chunk_raw_by_id(CHUNK_PRIMARY)
    }

    // ── Mid-card per-field FSTs ──────────────────────────────────────

    /// Decompress and deserialize a mid-card field FST by index.
    pub fn mid_field(&self, index: u16) -> Result<FstIndex<BitmapValue>, Error> {
        unpack(self.mid_field_raw(index)?)
    }

    /// Raw compressed bytes of a mid-card field chunk.
    pub fn mid_field_raw(&self, index: u16) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, mid_field_id(index))
            .map_err(|_| Error::ChunkNotFound(index))
    }

    // ── High-card per-field sorted lists ─────────────────────────────

    /// Decompress and deserialize a high-card field's sorted list of
    /// `(key=value, bitmap)` pairs.
    pub fn high_field(&self, index: u16) -> Result<Vec<(String, BitmapValue)>, Error> {
        unpack(self.high_field_raw(index)?)
    }

    /// Raw compressed bytes of a high-card field chunk.
    pub fn high_field_raw(&self, index: u16) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, high_field_id(index))
            .map_err(|_| Error::ChunkNotFound(index))
    }

    // ── Per-log timestamps ───────────────────────────────────────────

    /// Decompress and deserialize the per-log timestamps chunk.
    ///
    /// Returns a `Vec<i64>` of nanosecond timestamps in chronological
    /// order, parallel-indexed to [`stream_entries`](Self::stream_entries):
    /// `timestamps[i]` is the timestamp of the log whose attribute
    /// list lives at `entries[i]`.
    pub fn timestamps(&self) -> Result<Vec<i64>, Error> {
        unpack(self.timestamps_raw()?)
    }

    /// Raw compressed bytes of the timestamps chunk.
    pub fn timestamps_raw(&self) -> Result<&'a [u8], Error> {
        self.chunk_raw_by_id(CHUNK_TIMS)
    }

    // ── Stream-log-entries chunk ─────────────────────────────────────

    /// Decompress and deserialize the stream-log-entries chunk.
    pub fn stream_entries(&self) -> Result<Vec<Vec<KvId>>, Error> {
        unpack(self.stream_entries_raw()?)
    }

    /// Raw compressed bytes of the stream-log-entries chunk.
    pub fn stream_entries_raw(&self) -> Result<&'a [u8], Error> {
        self.chunk_raw_by_id(CHUNK_STREAM)
    }

    // ── Positional secondary-chunk access (escape hatch) ─────────────

    /// Raw compressed bytes of the secondary chunk at absolute
    /// `position` (0-based). Most callers should prefer the typed
    /// accessors ([`Reader::mid_field`], [`Reader::high_field`]).
    /// This method is used by tooling that walks secondary chunks
    /// position-by-position; the calling convention is to use the
    /// `(mid_idx)` then `(high_idx)` indices via the typed methods
    /// instead.
    fn chunk_raw_by_id(&self, id: gix_chunk::Id) -> Result<&'a [u8], Error> {
        self.toc
            .data_by_id(self.data, id)
            .map_err(|e| Error::Toc(format!("{e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
