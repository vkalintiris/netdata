use crate::JournalError;
use crate::registry::RegistryError;
use thiserror::Error;

/// Errors that can occur when working with the journal registry
#[derive(Debug, Error)]
pub enum IndexStateError {
    /// I/O error when reading or scanning directories
    #[error("Registry error: {0}")]
    Registry(#[from] RegistryError),

    #[error("Journal error: {0}")]
    Journal(#[from] JournalError),
}

/// A specialized Result type for journal registry operations
pub type Result<T> = std::result::Result<T, IndexStateError>;
