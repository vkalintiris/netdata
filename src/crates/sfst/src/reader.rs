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
    BitmapValue, CHUNK_META, CHUNK_PRIMARY, CHUNK_SUMMARY, CHUNK_TIMS, Error, FieldTable,
    FieldTier, HEADER_SIZE, HighField, MAGIC, MAX_STREAM_BATCHES, Metadata, StreamBatch, Summary,
    VERSION, high_field_id, mid_field_id, num_stream_batches, stream_batch_id,
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
    pub fn fields(&self) -> Result<&FieldTable, Error> {
        Ok(&self.metadata()?.fields)
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

    /// Byte span `(offset, len)` of the **cold suffix** — everything after
    /// the hot prefix (`SUMR`/`META`/`TIMS`/`PRIM`): the mid/high field
    /// chunks and the stream batches. A query keeps the hot prefix
    /// resident in the page cache and releases this region once done.
    ///
    /// Offsets are relative to the start of the slice, so the span is
    /// usable directly with an mmap's `advise_range`. In the canonical
    /// layout PRIM is the last hot-prefix chunk and the chunk bodies run to
    /// EOF, so the span is `[end of PRIM, end of file)`. Returns `None`
    /// only if the primary chunk is absent. The span is **not**
    /// page-aligned — a caller advising the kernel should align it inward
    /// to avoid touching the primary's edge page.
    pub fn cold_region(&self) -> Option<(usize, usize)> {
        let base = self.data.as_ptr() as usize;
        let primary = self.primary_raw().ok()?;
        let start = (primary.as_ptr() as usize - base) + primary.len();
        let end = self.data.len();
        if end > start {
            Some((start, end - start))
        } else {
            None
        }
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

    // ── High-card per-field columnar chunks ──────────────────────────

    /// Decompress and deserialize a high-card field's sorted columnar
    /// data: parallel `keys` and `masks` vectors.
    ///
    /// `masks[j]` is a bitmask over the file's stream batches (see
    /// [`crate::num_stream_batches`]): bit `b` is set iff the value
    /// `keys[j]` appears in stream batch `b`. Callers walk the set
    /// bits to decide which [`stream_batch`](Self::stream_batch)
    /// chunks to decompress when materialising matching log positions.
    pub(crate) fn high_field(&self, index: u16) -> Result<HighField, Error> {
        let mut high: HighField = unpack(self.high_field_raw(index)?)?;
        // `offsets` is `#[serde(skip)]`, so it deserializes empty — derive it
        // from the decoded `key_lens` before the chunk is used.
        high.rebuild_offsets();
        Ok(high)
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
    /// order, parallel-indexed to the concatenation of every
    /// [`stream_batch`](Self::stream_batch) chunk: `timestamps[i]` is
    /// the timestamp of the log whose attribute list lives at global
    /// position `i` in the concatenated stream.
    pub fn timestamps(&self) -> Result<Vec<i64>, Error> {
        unpack(self.timestamps_raw()?)
    }

    /// Raw compressed bytes of the timestamps chunk.
    pub fn timestamps_raw(&self) -> Result<&'a [u8], Error> {
        self.chunk_raw_by_id(CHUNK_TIMS)
    }

    // ── Stream-batch chunks ──────────────────────────────────────────

    /// Decompress and deserialize one stream-batch chunk by index.
    ///
    /// `index` must be in `0..num_stream_batches(summary.total_logs)`
    /// (see [`crate::num_stream_batches`]). The returned [`StreamBatch`]
    /// holds the attribute lists for the logs in that batch, in
    /// chronological order; concatenating all batches in order yields the
    /// full chronological log stream.
    pub fn stream_batch(&self, index: u8) -> Result<StreamBatch, Error> {
        let mut batch: StreamBatch = unpack(self.stream_batch_raw(index)?)?;
        // `row_offsets` is `#[serde(skip)]`, so it deserializes empty —
        // derive it from the decoded `row_lens` before the batch is used.
        batch.rebuild_offsets();
        Ok(batch)
    }

    /// Raw compressed bytes of one stream-batch chunk.
    pub fn stream_batch_raw(&self, index: u8) -> Result<&'a [u8], Error> {
        if index >= MAX_STREAM_BATCHES {
            return Err(Error::ChunkNotFound(index as u16));
        }
        self.toc
            .data_by_id(self.data, stream_batch_id(index))
            .map_err(|_| Error::ChunkNotFound(index as u16))
    }

    /// Number of stream-batch chunks in this file, derived from
    /// `summary.total_logs` via [`crate::num_stream_batches`].
    ///
    /// Reads the `SUMR` chunk; callers that already hold a [`Summary`]
    /// should call [`crate::num_stream_batches`] directly.
    pub fn num_stream_batches(&self) -> Result<u8, Error> {
        Ok(num_stream_batches(self.summary()?.total_logs))
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
mod tests;
