//! Core functionality for working with systemd journal files.
//!
//! This crate provides:
//! - Low-level file I/O: [`mod@file`] module
//! - File tracking and monitoring: [`registry`] and [`repository`] modules
//!
//! For high-level journaling with rotation and retention, see the `journal-log-writer` crate.

#[macro_use]
extern crate static_assertions;

// Core error types used throughout the crate
pub mod error;

// Collection type aliases
pub mod collections;

// Low-level journal file format I/O
pub mod file;

// Journal file tracking and monitoring
pub mod repository;

// Journal file tracking and monitoring
pub mod registry;

// Re-export commonly used types for convenience
pub use error::{JournalError, Result};

// File module re-exports
pub use file::{
    BucketUtilization, Direction, JournalCursor, JournalFile, JournalFileOptions, JournalReader,
    JournalWriter, Location,
};
