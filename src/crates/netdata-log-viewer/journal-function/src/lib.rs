//! Systemd journal function implementation crate.
//!
//! This crate provides the function handler infrastructure for the systemd-journal function,
//! including error types, facets configuration, registry management with monitoring and metadata,
//! caching infrastructure, indexing infrastructure, histogram service,
//! and Netdata-specific formatting and protocol types.

pub mod cache;
pub mod charts;
pub mod error;
pub mod facets;
pub mod histogram;
pub mod indexing;
pub mod logs;
pub mod netdata;

// Re-export commonly used types
pub use cache::{Cache, FileIndexCache, FileIndexKey};
pub use charts::{
    BucketCacheMetrics, BucketOperationsMetrics, FileIndexingMetrics, JournalMetrics,
};
pub use error::{CatalogError, Result};
pub use facets::Facets;
pub use histogram::{
    BucketCompleteResponse, BucketRequest, BucketResponse, HistogramRequest, HistogramResponse,
    HistogramService,
};
pub use indexing::{
    FileIndexStream, FileIndexRequest, FileIndexResponse, IndexingService, IteratorError,
};
// Re-export registry types from journal_registry
pub use journal_registry::{File, FileInfo, Monitor, Registry, TimeRange};
