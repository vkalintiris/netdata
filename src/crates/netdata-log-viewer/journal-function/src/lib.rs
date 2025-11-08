//! Systemd journal function implementation crate.
//!
//! This crate provides the function handler infrastructure for the systemd-journal function,
//! including error types, facets configuration, registry management with monitoring and metadata,
//! caching infrastructure, indexing infrastructure, histogram service,
//! and schema types for communicating with Netdata.

pub mod cache;
pub mod error;
pub mod facets;
pub mod histogram;
pub mod indexing;
pub mod registry;
pub mod schema;

// Re-export commonly used types
pub use cache::{Cache, FileIndexCache, FileIndexKey};
pub use error::{CatalogError, Result};
pub use facets::Facets;
pub use histogram::{
    BucketCompleteResponse, BucketRequest, BucketResponse, HistogramRequest, HistogramResponse,
    HistogramService,
};
pub use indexing::{
    FileIndexStream, FileIndexRequest, FileIndexResponse, IndexingService, IteratorError,
};
pub use journal::repository::File;
pub use registry::{FileInfo, Monitor, Registry, TimeRange};
