//! High-level and low-level APIs for working with systemd journal files.
//!
//! This crate provides:
//! - Low-level file I/O: [`file`] module
//! - High-level journaling with rotation: [`log`] module
//! - File tracking and monitoring: [`registry`] module
//!
//! # Examples
//!
//! ```no_run
//! use journal::log::{JournalLog, JournalLogConfig, RotationPolicy};
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//!
//! let config = JournalLogConfig::new("/var/log/journal")
//!     .with_rotation_policy(
//!         RotationPolicy::default()
//!             .with_size_of_journal_file(100 * 1024 * 1024)
//!     );
//!
//! let mut journal = JournalLog::new(config)?;
//! journal.write_entry(&[b"MESSAGE=Hello, world!"])?;
//! # Ok(())
//! # }
//! ```

#[macro_use]
extern crate static_assertions;

// Core error types used throughout the crate
pub mod error;

// Internal collection type aliases (not re-exported)
mod collections;

// Low-level journal file format I/O
pub mod file;

// High-level journal API with rotation and retention
pub mod log;

// Journal file tracking and monitoring
pub mod repository;

// Journal file tracking and monitoring
pub mod registry;

// Journal file indexing
pub mod index;

// Indexing state
pub mod index_state;

// Re-export commonly used types for convenience
pub use error::{JournalError, Result};

// File module re-exports
pub use file::{
    BucketUtilization, Direction, JournalCursor, JournalFile, JournalFileOptions, JournalReader,
    JournalWriter, Location,
};
