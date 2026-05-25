//! Error type for SFST format operations.

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
