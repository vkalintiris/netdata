//! Catalog data model: what the otel-plugin records about uploaded SFST files.
//!
//! This crate defines the types (`Catalog`, `CatalogEntry`, `StreamEntry`,
//! `CatalogQuery`) and their JSON serialization. It does not perform I/O —
//! writing, uploading, and reconciliation live in later phases of the catalog
//! implementation plan.

pub mod catalog;
pub mod entry;
pub mod query;
pub mod registry;

pub use catalog::Catalog;
pub use entry::{CatalogEntry, StreamEntry};
pub use query::CatalogQuery;

/// Current on-disk / on-wire catalog format version.
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("unsupported catalog format version: {0}")]
    UnsupportedVersion(u32),
}
