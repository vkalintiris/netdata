//! Error type for SFST format operations.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Underlying I/O failed during read/write/flush/sync. Auto-lifted
    /// from [`std::io::Error`].
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// `bincode::encode_to_vec` rejected a value while packing a chunk
    /// payload.
    #[error("bincode encode error: {0}")]
    BincodeEncode(#[from] bincode::error::EncodeError),

    /// `bincode::decode_from_slice` failed while unpacking a chunk
    /// payload — the bytes don't match the expected shape, or are
    /// truncated.
    #[error("bincode decode error: {0}")]
    BincodeDecode(#[from] bincode::error::DecodeError),

    /// `zstd` compression or decompression failed without surfacing as
    /// [`std::io::Error`] — e.g., invalid frame header, truncated
    /// frame, checksum mismatch.
    #[error("zstd error (not std::io): {0}")]
    Zstd(String),

    /// [`Reader::open`](crate::Reader::open) found the first 4 bytes
    /// aren't `"SFST"`. The byte stream is either not an SFST file or
    /// has been corrupted before the header.
    #[error("invalid magic (expected \"SFST\")")]
    InvalidMagic,

    /// [`Reader::open`](crate::Reader::open) found a header `version`
    /// field this build of `sfst` doesn't recognize.
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u32),

    /// A chunk lookup by index found no matching id — e.g.,
    /// [`Reader::mid_field`](crate::Reader::mid_field) called with an
    /// index past the file's mid-card field count.
    #[error("chunk not found: index {0}")]
    ChunkNotFound(u16),

    /// [`Writer::write_to`](crate::Writer::write_to) was called without
    /// [`Writer::set_primary`](crate::Writer::set_primary) having been
    /// called first. The `PRIM` chunk is mandatory.
    #[error("no primary chunk set")]
    NoPrimary,

    /// [`Writer::write_to`](crate::Writer::write_to) was called without
    /// [`Writer::set_timestamps`](crate::Writer::set_timestamps) having
    /// been called first. The `TIMS` chunk is mandatory.
    #[error("no timestamps chunk set")]
    NoTimestamps,

    /// `gix-chunk` failed to parse the TOC (on open) or lay it out
    /// (on write). Carries `gix-chunk`'s own error message.
    #[error("TOC error: {0}")]
    Toc(String),

    /// The byte slice handed to [`Reader::open`](crate::Reader::open)
    /// is shorter than the 12-byte fixed header. First value is the
    /// actual length, second is the required minimum.
    #[error("file too short ({0} bytes, need at least {1})")]
    FileTooShort(usize, usize),
}
