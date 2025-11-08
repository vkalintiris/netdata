//! Error types for catalog operations

use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur with catalog operations
#[derive(Debug, Error)]
pub enum CatalogError {
    /// Error from the file system watcher
    #[error("File system watcher error: {0}")]
    Notify(#[from] notify::Error),

    /// I/O error when reading or scanning directories
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Error from repository operations
    #[error("Repository error: {0}")]
    Repository(#[from] journal::repository::RepositoryError),

    /// Error when parsing a journal file path
    #[error("Failed to parse journal file path: {path}")]
    InvalidPath { path: String },

    /// Error when a path contains invalid UTF-8
    #[error("Path contains invalid UTF-8: {}", .path.display())]
    InvalidUtf8 { path: PathBuf },

    /// Channel closed error
    #[error("Channel closed")]
    ChannelClosed,

    /// Lock poisoning error
    #[error("Lock poisoned: {0}")]
    LockPoisoned(String),
}

/// A specialized Result type for catalog operations
pub type Result<T> = std::result::Result<T, CatalogError>;
