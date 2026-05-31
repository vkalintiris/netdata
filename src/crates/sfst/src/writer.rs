//! SFST format writer.
//!
//! [`Writer`] assembles chunk payloads into the on-disk container.
//! Payloads are passed in pre-packed (typically via [`pack`]) so callers
//! can produce them in parallel.

use std::io::Write;

use serde::Serialize;

use crate::{
    CHUNK_META, CHUNK_PRIMARY, CHUNK_SUMMARY, CHUNK_TIMS, Error, HEADER_SIZE, MAGIC,
    MAX_STREAM_BATCHES, VERSION, high_field_id, mid_field_id, stream_batch_id,
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
/// called: SUMR → META → PRIM → mid-card fields → high-card fields →
/// TIMS → stream-batch chunks (SB00..SB07) in append order.
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
    /// Fixed on-disk order: SUMR (if present), META (if present),
    /// PRIM, mid-card field chunks in append order, high-card field
    /// chunks in append order, TIMS, stream-batch chunks in append
    /// order (SB00..SB{N-1}).
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

        let num_chunks = self.summary.is_some() as usize
            + self.metadata.is_some() as usize
            + 1 // primary
            + self.mid_fields.len()
            + self.high_fields.len()
            + 1 // timestamps
            + num_stream_batches;

        // Header
        w.write_all(MAGIC)?;
        w.write_all(&VERSION.to_le_bytes())?;
        w.write_all(&(num_chunks as u32).to_le_bytes())?;

        // Plan chunks. The physical order is not part of the format
        // contract — readers resolve every chunk through the TOC — but the
        // producer deliberately groups the chunks a query's statistics
        // phase always reads (SUMR, META, TIMS, PRIM) into a hot prefix,
        // ahead of the touch-then-drop mid/high field chunks and the
        // stream batches. That lets a reader keep the prefix resident in
        // the page cache and advise the cold remainder away as one span.
        // SUMR stays first so a recovery-only reader can stop after the
        // summary without paging through the rest; PRIM sits last in the
        // prefix, next to the structurally-identical mid/high field FSTs.
        let mut index = gix_chunk::file::Index::for_writing();
        if let Some(sum) = &self.summary {
            index.plan_chunk(CHUNK_SUMMARY, sum.len() as u64);
        }
        if let Some(meta) = &self.metadata {
            index.plan_chunk(CHUNK_META, meta.len() as u64);
        }
        index.plan_chunk(CHUNK_TIMS, timestamps.len() as u64);
        index.plan_chunk(CHUNK_PRIMARY, primary.len() as u64);
        for (i, chunk) in self.mid_fields.iter().enumerate() {
            index.plan_chunk(mid_field_id(i as u16), chunk.len() as u64);
        }
        for (i, chunk) in self.high_fields.iter().enumerate() {
            index.plan_chunk(high_field_id(i as u16), chunk.len() as u64);
        }
        for (i, batch) in self.stream_batches.iter().enumerate() {
            index.plan_chunk(stream_batch_id(i as u8), batch.len() as u64);
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

        let id = chunk_writer
            .next_chunk()
            .expect("expected timestamps chunk");
        assert_eq!(id, CHUNK_TIMS);
        chunk_writer.write_all(timestamps)?;

        let id = chunk_writer.next_chunk().expect("expected primary chunk");
        assert_eq!(id, CHUNK_PRIMARY);
        chunk_writer.write_all(primary)?;

        for (i, chunk) in self.mid_fields.iter().enumerate() {
            let id = chunk_writer.next_chunk().expect("expected mid-field chunk");
            assert_eq!(id, mid_field_id(i as u16));
            chunk_writer.write_all(chunk)?;
        }
        for (i, chunk) in self.high_fields.iter().enumerate() {
            let id = chunk_writer
                .next_chunk()
                .expect("expected high-field chunk");
            assert_eq!(id, high_field_id(i as u16));
            chunk_writer.write_all(chunk)?;
        }
        for (i, batch) in self.stream_batches.iter().enumerate() {
            let id = chunk_writer
                .next_chunk()
                .expect("expected stream-batch chunk");
            assert_eq!(id, stream_batch_id(i as u8));
            chunk_writer.write_all(batch)?;
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

#[cfg(test)]
mod tests;
