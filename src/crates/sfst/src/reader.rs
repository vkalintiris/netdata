//! SFST format reader.
//!
//! [`Reader`] opens a byte slice (typically an mmap) and exposes chunks
//! by id. Decompression and deserialization are lazy — `open` only parses
//! the header and TOC.

use fst_index::FstIndex;
use serde::de::DeserializeOwned;

use crate::{
    CHUNK_FLDS, CHUNK_META, CHUNK_PRIMARY, CHUNK_SUMMARY, Error, FileSummary, HEADER_SIZE, MAGIC,
    VERSION, hc_chunk_id,
};

/// Decompress zstd, then deserialize with bincode.
pub fn unpack<T: DeserializeOwned>(data: &[u8]) -> Result<T, Error> {
    let decompressed = zstd::decode_all(data).map_err(|e| Error::Zstd(e.to_string()))?;
    let (val, _len) =
        bincode::serde::decode_from_slice(&decompressed, bincode::config::standard())?;
    Ok(val)
}

/// Zero-copy reader over a memory-mapped (or in-memory) split-FST file.
///
/// Decompression happens lazily when [`Reader::primary`], [`Reader::chunk`],
/// or their `_raw` variants are called.
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
        unpack(self.summary_raw()?)
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
        unpack(self.metadata_raw()?)
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
        unpack(self.fields_raw()?)
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
