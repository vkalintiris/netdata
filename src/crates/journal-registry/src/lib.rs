//! Journal file registry and repository
//!
//! This crate provides functionality for discovering, tracking, and monitoring
//! systemd journal files in directories.
//!
//! ## Key Components
//!
//! - **Repository**: Types for representing journal files and organizing them into chains
//! - **Registry**: High-level interface for watching directories and tracking file changes
//!
//! ## Usage
//!
//! ```no_run
//! use journal_registry::{Registry, Monitor};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let monitor = Monitor::new()?;
//! let mut registry = Registry::new(monitor);
//!
//! // Watch a directory for journal files
//! registry.watch_directory("/var/log/journal")?;
//!
//! // Find files in a time range (seconds since epoch)
//! let files = registry.find_files_in_range(1000000, 2000000);
//! # Ok(())
//! # }
//! ```

pub mod registry;
pub mod repository;

pub use registry::{Monitor, Registry, RegistryError};
pub use repository::{Chain, File, FileInfo, FileInner, Origin, Repository, RepositoryError, Source, Status, TimeRange};
