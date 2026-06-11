//! SFST format writer.
//!
//! [`Writer`] assembles chunk payloads into the on-disk container.
//! Payloads are passed in pre-packed (typically via [`pack`]) so callers
//! can produce them in parallel.

use std::io::Write;

use chunk_file::container::ContainerBuilder;
use serde::Serialize;

use crate::{
    CHUNK_META, CHUNK_PRIMARY, CHUNK_SUMMARY, CHUNK_TIMS, Error, MAGIC, MAX_STREAM_BATCHES,
    VERSION, high_field_id, mid_field_id, stream_batch_id,
};

/// Serialize a value with bincode, then compress with zstd.
///
/// The `?Sized` bound lets callers pass slice references directly
/// (e.g. `pack(batch, 1)` where `batch: &[T]`) instead of materialising
/// an owned `Vec`.
pub fn pack<T: Serialize + ?Sized>(value: &T, zstd_level: i32) -> Result<Vec<u8>, Error> {
    let serialized = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
    zstd::encode_all(&serialized[..], zstd_level).map_err(|e| Error::Zstd(e.to_string()))
}

/// Builds an SFST file from pre-packed (bincode + zstd) byte blobs.
///
/// Callers supply already-compressed bytes — typically produced via
/// [`pack`] — and the writer concatenates them into the on-disk
/// container with the right TOC. Pre-packing means callers can build
/// chunk payloads in parallel (e.g., one per field) before collecting
/// results into a single sequential writer.
///
/// On-disk ordering is fixed regardless of the order setters are
/// called: SUMR → META → TIMS → PRIM → mid-card fields → high-card
/// fields → stream-batch chunks (SB00..SB07) in append order. The
/// always-read chunks (SUMR/META/TIMS/PRIM) lead so they form a hot
/// page-cache prefix ahead of the touch-then-drop field chunks; see the
/// note in [`write_to`](Writer::write_to).
pub struct Writer {
    summary: Option<Vec<u8>>,
    metadata: Option<Vec<u8>>,
    primary: Option<Vec<u8>>,
    mid_fields: Vec<Vec<u8>>,
    high_fields: Vec<Vec<u8>>,
    timestamps: Option<Vec<u8>>,
    stream_batches: Vec<Vec<u8>>,
}

impl Writer {
    pub fn new() -> Self {
        Self {
            summary: None,
            metadata: None,
            primary: None,
            mid_fields: Vec::new(),
            high_fields: Vec::new(),
            timestamps: None,
            stream_batches: Vec::new(),
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

    /// Set the primary chunk (pre-compressed bytes, e.g. bincode + zstd).
    pub fn set_primary(&mut self, packed: Vec<u8>) {
        self.primary = Some(packed);
    }

    /// Append a mid-cardinality field FST chunk and return its index.
    ///
    /// Panics if more than `u16::MAX` (65,535) mid-card chunks are added —
    /// the chunk-id encoding only has 2 bytes for the index, so wrap-around
    /// would silently collide with `MF{0,0}`.
    pub fn add_mid_field(&mut self, packed: Vec<u8>) -> u16 {
        let idx =
            u16::try_from(self.mid_fields.len()).expect("mid-card field count exceeds u16::MAX");
        self.mid_fields.push(packed);
        idx
    }

    /// Append a high-cardinality field chunk and return its index.
    ///
    /// Panics if more than `u16::MAX` (65,535) high-card chunks are added —
    /// same chunk-id encoding constraint as [`Writer::add_mid_field`].
    pub fn add_high_field(&mut self, packed: Vec<u8>) -> u16 {
        let idx =
            u16::try_from(self.high_fields.len()).expect("high-card field count exceeds u16::MAX");
        self.high_fields.push(packed);
        idx
    }

    /// Set the per-log timestamps chunk (pre-compressed bytes). Mandatory:
    /// every SFST must carry per-log nanosecond timestamps parallel-indexed
    /// to the stream-batch chunks.
    pub fn set_timestamps(&mut self, packed: Vec<u8>) {
        self.timestamps = Some(packed);
    }

    /// Append a stream-batch chunk (pre-compressed bytes) and return its
    /// index. Callers add batches in chronological order; the index ends
    /// up encoded in the chunk id (`SB00` through `SB07`).
    ///
    /// Panics if more than [`MAX_STREAM_BATCHES`] batches are added — the
    /// chunk-id encoding allows only one ASCII digit and the
    /// `(String, u8)` mask in each high-card chunk can only address eight
    /// batches.
    pub fn add_stream_batch(&mut self, packed: Vec<u8>) -> u8 {
        assert!(
            self.stream_batches.len() < MAX_STREAM_BATCHES as usize,
            "stream-batch count exceeds MAX_STREAM_BATCHES ({MAX_STREAM_BATCHES})",
        );
        let idx = self.stream_batches.len() as u8;
        self.stream_batches.push(packed);
        idx
    }

    /// Serialize the entire SFST file to `w`.
    ///
    /// Fixed on-disk order: SUMR (if present), META (if present), TIMS,
    /// PRIM, mid-card field chunks in append order, high-card field
    /// chunks in append order, stream-batch chunks in append order
    /// (SB00..SB{N-1}).
    ///
    /// Returns [`Error::InvalidStreamBatchCount`] if the number of
    /// stream batches isn't in `1..=`[`MAX_STREAM_BATCHES`].
    pub fn write_to<W: Write>(&self, w: &mut W) -> Result<(), Error> {
        let primary = self.primary.as_ref().ok_or(Error::NoPrimary)?;
        let timestamps = self.timestamps.as_ref().ok_or(Error::NoTimestamps)?;
        let num_stream_batches = self.stream_batches.len();
        if num_stream_batches == 0 || num_stream_batches > MAX_STREAM_BATCHES as usize {
            return Err(Error::InvalidStreamBatchCount(num_stream_batches));
        }

        // Chunk order. The physical order is not part of the format
        // contract — readers resolve every chunk through the TOC — but the
        // producer deliberately groups the chunks a query's statistics
        // phase always reads (SUMR, META, TIMS, PRIM) into a hot prefix,
        // ahead of the touch-then-drop mid/high field chunks and the
        // stream batches. That lets a reader keep the prefix resident in
        // the page cache and advise the cold remainder away as one span.
        // SUMR stays first so a recovery-only reader can stop after the
        // summary without paging through the rest; PRIM sits last in the
        // prefix, next to the structurally-identical mid/high field FSTs.
        // The container preserves add order and appends each chunk's
        // crc32 trailer.
        let mut container = ContainerBuilder::new(*MAGIC, VERSION);
        if let Some(sum) = &self.summary {
            container.add_chunk(CHUNK_SUMMARY, sum);
        }
        if let Some(meta) = &self.metadata {
            container.add_chunk(CHUNK_META, meta);
        }
        container.add_chunk(CHUNK_TIMS, timestamps);
        container.add_chunk(CHUNK_PRIMARY, primary);
        for (i, chunk) in self.mid_fields.iter().enumerate() {
            container.add_chunk(mid_field_id(i as u16), chunk);
        }
        for (i, chunk) in self.high_fields.iter().enumerate() {
            container.add_chunk(high_field_id(i as u16), chunk);
        }
        for (i, batch) in self.stream_batches.iter().enumerate() {
            container.add_chunk(stream_batch_id(i as u8), batch);
        }
        container.write_to(w)?;
        Ok(())
    }
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
