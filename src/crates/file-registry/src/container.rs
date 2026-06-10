//! Shared chunk-container file format: magic + version + [`gix_chunk`]
//! TOC + per-chunk crc32 trailers.
//!
//! Both the SFST index files and the otel catalog files are durable
//! artifacts uploaded to remote storage that is never garbage-collected,
//! so every one of them must be self-describing (magic + version) and
//! integrity-checked (CRC). This module is the single implementation of
//! that framing; consumers supply their own magic and format version and
//! own the meaning of their chunk payloads.
//!
//! On-disk layout:
//!
//! ```text
//! [ magic: 4 bytes (consumer-supplied, e.g. "SFST" / "NCAT") ]
//! [ version: u32 LE (consumer-supplied format version)       ]
//! [ num_chunks: u32 LE                                       ]
//! [ gix_chunk TOC (at HEADER_SIZE)                           ]
//! [ chunk payloads, each: <payload bytes> <crc32 u32 LE>     ]
//! ```
//!
//! The crc32 ([`crc32fast`]) covers the stored payload bytes only —
//! for compressed payloads that is the compressed form, which is where
//! at-rest / in-transit corruption happens; if the stored bytes verify,
//! decompression is deterministic. The TOC records each chunk's length
//! as `payload_len + 4` (CRC included in the span). TOC corruption is
//! caught indirectly: a corrupt offset reads the wrong span, whose CRC
//! then fails to match.

use std::io::Write;

/// Four-byte chunk identifier ([`gix_chunk::Id`]).
pub type ChunkId = gix_chunk::Id;

/// Fixed header size: magic(4) + version(4) + num_chunks(4).
pub const HEADER_SIZE: usize = 12;

/// Per-chunk crc32 trailer size.
const CRC_LEN: usize = 4;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The byte slice is shorter than the fixed header. First value is
    /// the actual length, second the required minimum.
    #[error("file too short ({0} bytes, need at least {1})")]
    TooShort(usize, usize),

    /// The first 4 bytes don't match the consumer's expected magic.
    #[error("invalid magic")]
    BadMagic,

    /// The header `version` field doesn't match the consumer's expected
    /// format version.
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u32),

    /// The TOC failed to parse or lay out, or a chunk span is malformed.
    #[error("TOC error: {0}")]
    Toc(String),

    /// No chunk with the requested id exists in the TOC.
    #[error("chunk '{}' not found", id_str(id))]
    ChunkNotFound { id: ChunkId },

    /// A chunk's stored crc32 trailer doesn't match the crc32 computed
    /// over its payload bytes — the chunk is corrupt.
    #[error(
        "chunk '{}' CRC mismatch: stored {expected:#010x}, computed {actual:#010x}",
        id_str(id)
    )]
    CrcMismatch {
        id: ChunkId,
        expected: u32,
        actual: u32,
    },
}

fn id_str(id: &ChunkId) -> String {
    id.escape_ascii().to_string()
}

/// Assembles a container file from pre-built chunk payloads.
///
/// Chunks are written in the exact order they were added — producers
/// are free to group hot chunks into a prefix; the builder never
/// reorders. Each payload gains a trailing crc32 on write.
pub struct ContainerBuilder<'a> {
    magic: [u8; 4],
    version: u32,
    chunks: Vec<(ChunkId, &'a [u8])>,
}

impl<'a> ContainerBuilder<'a> {
    pub fn new(magic: [u8; 4], version: u32) -> Self {
        Self {
            magic,
            version,
            chunks: Vec::new(),
        }
    }

    /// Append a chunk. Duplicate ids are a producer bug —
    /// [`write_to`](ContainerBuilder::write_to) panics on them (via
    /// `gix_chunk`'s plan assertion).
    pub fn add_chunk(&mut self, id: ChunkId, payload: &'a [u8]) {
        self.chunks.push((id, payload));
    }

    /// Number of chunks added so far.
    pub fn num_chunks(&self) -> usize {
        self.chunks.len()
    }

    /// Serialize the container (header + TOC + payloads with crc32
    /// trailers) to `w`. At least one chunk must have been added —
    /// `gix_chunk` rejects empty chunk files on read.
    pub fn write_to<W: Write>(&self, w: &mut W) -> std::io::Result<()> {
        if self.chunks.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "container must have at least one chunk",
            ));
        }

        w.write_all(&self.magic)?;
        w.write_all(&self.version.to_le_bytes())?;
        w.write_all(&(self.chunks.len() as u32).to_le_bytes())?;

        let mut index = gix_chunk::file::Index::for_writing();
        for (id, payload) in &self.chunks {
            index.plan_chunk(*id, (payload.len() + CRC_LEN) as u64);
        }

        let mut chunk_writer = index.into_write(&mut *w, HEADER_SIZE)?;
        for (id, payload) in &self.chunks {
            let next = chunk_writer.next_chunk().expect("planned chunk");
            assert_eq!(next, *id, "chunk write order must match plan order");
            chunk_writer.write_all(payload)?;
            chunk_writer.write_all(&crc32fast::hash(payload).to_le_bytes())?;
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

/// Zero-copy view over a container file (typically an mmap).
///
/// [`open`](Container::open) parses only the header and TOC; payloads
/// are resolved — and CRC-verified — lazily, per chunk, so an mmap'd
/// file is only paged in where it is actually read.
pub struct Container<'a> {
    data: &'a [u8],
    toc: gix_chunk::file::Index,
}

impl<'a> Container<'a> {
    /// Open a container, validating length, magic, version and the TOC.
    pub fn open(data: &'a [u8], magic: &[u8; 4], version: u32) -> Result<Self, Error> {
        if data.len() < HEADER_SIZE {
            return Err(Error::TooShort(data.len(), HEADER_SIZE));
        }
        if &data[0..4] != magic {
            return Err(Error::BadMagic);
        }
        let file_version = u32::from_le_bytes(data[4..8].try_into().unwrap());
        if file_version != version {
            return Err(Error::UnsupportedVersion(file_version));
        }

        let num_chunks = u32::from_le_bytes(data[8..12].try_into().unwrap());
        // Defense in depth: a corrupted header claiming u32::MAX chunks
        // would lead gix-chunk to attempt ~51 GiB of TOC allocation.
        // Each TOC entry is 12 bytes, so the on-disk body bounds the
        // legal value.
        let max_chunks =
            data.len().saturating_sub(HEADER_SIZE) / gix_chunk::file::Index::ENTRY_SIZE;
        if num_chunks as usize > max_chunks {
            return Err(Error::Toc(format!(
                "num_chunks ({num_chunks}) exceeds plausible maximum ({max_chunks})"
            )));
        }
        let toc = gix_chunk::file::Index::from_bytes(data, HEADER_SIZE, num_chunks)
            .map_err(|e| Error::Toc(format!("{e}")))?;

        Ok(Self { data, toc })
    }

    /// Resolve a chunk's payload bytes, verifying its crc32 trailer.
    /// This is the single verified chokepoint — every consumer read
    /// goes through here, so corruption can't reach a deserializer.
    pub fn chunk(&self, id: ChunkId) -> Result<&'a [u8], Error> {
        let range = self
            .toc
            .offset_by_id(id)
            .map_err(|_| Error::ChunkNotFound { id })?;
        let range = gix_chunk::range::into_usize(range)
            .ok_or_else(|| Error::Toc("chunk offsets don't fit in usize".to_string()))?;
        // `from_bytes` validated offsets against the file length, so the
        // slice is in-bounds; `.get` keeps this panic-free regardless.
        let span = self
            .data
            .get(range)
            .ok_or_else(|| Error::Toc("chunk span out of bounds".to_string()))?;
        if span.len() < CRC_LEN {
            return Err(Error::Toc(format!(
                "chunk '{}' shorter than its CRC trailer",
                id_str(&id)
            )));
        }
        let (payload, crc_bytes) = span.split_at(span.len() - CRC_LEN);
        let expected = u32::from_le_bytes(crc_bytes.try_into().unwrap());
        let actual = crc32fast::hash(payload);
        if actual != expected {
            return Err(Error::CrcMismatch {
                id,
                expected,
                actual,
            });
        }
        Ok(payload)
    }

    /// Whether a chunk with `id` exists in the TOC (no CRC check).
    pub fn has_chunk(&self, id: ChunkId) -> bool {
        self.toc.offset_by_id(id).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAGIC: [u8; 4] = *b"TEST";
    const VERSION: u32 = 7;

    fn build(chunks: &[(ChunkId, &[u8])]) -> Vec<u8> {
        let mut b = ContainerBuilder::new(MAGIC, VERSION);
        for (id, payload) in chunks {
            b.add_chunk(*id, payload);
        }
        let mut out = Vec::new();
        b.write_to(&mut out).unwrap();
        out
    }

    #[test]
    fn roundtrip_multiple_chunks_preserves_order_and_payloads() {
        let bytes = build(&[
            (*b"AAAA", b"alpha payload"),
            (*b"BBBB", b""),
            (*b"CCCC", b"gamma"),
        ]);
        let c = Container::open(&bytes, &MAGIC, VERSION).unwrap();
        assert_eq!(c.chunk(*b"AAAA").unwrap(), b"alpha payload");
        assert_eq!(c.chunk(*b"BBBB").unwrap(), b"");
        assert_eq!(c.chunk(*b"CCCC").unwrap(), b"gamma");
        assert!(c.has_chunk(*b"AAAA"));
        assert!(!c.has_chunk(*b"ZZZZ"));

        // Physical order matches add order: AAAA's payload sits before
        // CCCC's in the file.
        let a_off = bytes
            .windows(b"alpha payload".len())
            .position(|w| w == b"alpha payload")
            .unwrap();
        let c_off = bytes.windows(5).position(|w| w == b"gamma").unwrap();
        assert!(a_off < c_off);
    }

    #[test]
    fn missing_chunk_is_not_found() {
        let bytes = build(&[(*b"AAAA", b"x")]);
        let c = Container::open(&bytes, &MAGIC, VERSION).unwrap();
        match c.chunk(*b"NOPE") {
            Err(Error::ChunkNotFound { id }) => assert_eq!(id, *b"NOPE"),
            other => panic!("expected ChunkNotFound, got {other:?}"),
        }
    }

    #[test]
    fn flipping_any_payload_or_crc_byte_fails_crc() {
        let clean = build(&[(*b"AAAA", b"some payload bytes"), (*b"BBBB", b"other")]);
        let c = Container::open(&clean, &MAGIC, VERSION).unwrap();
        let payload_start = clean
            .windows(b"some payload bytes".len())
            .position(|w| w == b"some payload bytes")
            .unwrap();
        // Flip every byte of AAAA's payload + 4-byte CRC trailer, one at
        // a time; each flip must be caught.
        for i in payload_start..payload_start + b"some payload bytes".len() + CRC_LEN {
            let mut corrupt = clean.clone();
            corrupt[i] ^= 0x01;
            let cc = Container::open(&corrupt, &MAGIC, VERSION).unwrap();
            match cc.chunk(*b"AAAA") {
                Err(Error::CrcMismatch { id, .. }) => assert_eq!(id, *b"AAAA"),
                other => panic!("flip at {i}: expected CrcMismatch, got {other:?}"),
            }
            // The untouched chunk still verifies.
            assert_eq!(cc.chunk(*b"BBBB").unwrap(), b"other");
        }
        let _ = c;
    }

    #[test]
    fn open_rejects_bad_magic_version_and_short_input() {
        let bytes = build(&[(*b"AAAA", b"x")]);

        match Container::open(&bytes, b"OTHR", VERSION).err() {
            Some(Error::BadMagic) => {}
            other => panic!("expected BadMagic, got {other:?}"),
        }
        match Container::open(&bytes, &MAGIC, VERSION + 1).err() {
            Some(Error::UnsupportedVersion(v)) => assert_eq!(v, VERSION),
            other => panic!("expected UnsupportedVersion, got {other:?}"),
        }
        match Container::open(&bytes[..HEADER_SIZE - 1], &MAGIC, VERSION).err() {
            Some(Error::TooShort(..)) => {}
            other => panic!("expected TooShort, got {other:?}"),
        }
    }

    #[test]
    fn open_rejects_overlarge_num_chunks() {
        let mut bytes = build(&[(*b"AAAA", b"x")]);
        bytes[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        match Container::open(&bytes, &MAGIC, VERSION).err() {
            Some(Error::Toc(msg)) => assert!(msg.contains("plausible maximum"), "{msg}"),
            other => panic!("expected Toc, got {other:?}"),
        }
    }

    #[test]
    fn empty_builder_is_rejected_at_write() {
        let b = ContainerBuilder::new(MAGIC, VERSION);
        let mut out = Vec::new();
        assert!(b.write_to(&mut out).is_err());
    }
}
