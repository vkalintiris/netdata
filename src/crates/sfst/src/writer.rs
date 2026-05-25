//! SFST format writer.
//!
//! [`Writer`] assembles chunk payloads into the on-disk container.
//! Payloads are passed in pre-packed (typically via [`pack`]) so callers
//! can produce them in parallel.

use std::io::Write;

use serde::Serialize;

use crate::{
    CHUNK_FLDS, CHUNK_META, CHUNK_PRIMARY, CHUNK_SUMMARY, Error, HEADER_SIZE, MAGIC, VERSION,
    hc_chunk_id,
};

/// Serialize a value with bincode, then compress with zstd.
pub fn pack<T: Serialize>(value: &T, zstd_level: i32) -> Result<Vec<u8>, Error> {
    let serialized = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
    zstd::encode_all(&serialized[..], zstd_level).map_err(|e| Error::Zstd(e.to_string()))
}

/// Builds a split-FST file from pre-packed (bincode + zstd) byte blobs.
///
/// Call [`pack`] to produce the blobs, then feed them here. Because `pack`
/// is a standalone function, callers can run it in parallel with rayon
/// before collecting results into the writer.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_on_no_primary() {
        let writer = Writer::new();
        let mut buf = Vec::new();
        assert!(writer.write_to(&mut buf).is_err());
    }
}
