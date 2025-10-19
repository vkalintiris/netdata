use crate::repository::RepositoryError;
use std::path::PathBuf;
use thiserror::Error;

/// Errors that can occur when working with the journal registry
#[derive(Debug, Error)]
pub enum RegistryError {
    /// Error from the file system watcher
    #[error("File system watcher error: {0}")]
    Notify(#[from] notify::Error),

    /// I/O error when reading or scanning directories
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Repository error: {0}")]
    Repository(#[from] RepositoryError),

    /// Error when parsing a journal file path
    #[error("Failed to parse journal file path: {path}")]
    InvalidPath { path: String },

    /// Error when a path contains invalid UTF-8
    #[error("Path contains invalid UTF-8: {}", .path.display())]
    InvalidUtf8 { path: PathBuf },

    /// Error when building foyer cache
    #[error("Failed to build cache: {0}")]
    CacheBuild(String),

    /// Error when converting numeric types
    #[error("Numeric conversion error: {0}")]
    NumericConversion(String),

    /// Error during foyer cache operations
    #[error("Foyer cache error: {0}")]
    Foyer(#[from] foyer::Error),

    /// Error from foyer's IoError type
    #[error("Foyer I/O error: {0}")]
    FoyerIo(#[from] foyer::IoError),
}

/// A specialized Result type for journal registry operations
pub type Result<T> = std::result::Result<T, RegistryError>;
