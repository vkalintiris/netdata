//! File indexing infrastructure with background indexing workers.

pub mod facets;
pub(crate) use facets::Facets;

pub mod file_metadata;
pub(crate) use file_metadata::{FileInfo, TimeRange};

pub mod file_index_cache;
pub(crate) use file_index_cache::{FileIndexCache, FileIndexKey};

pub mod hybrid_cache;

pub mod indexing_service;
pub(crate) use indexing_service::IndexingRequest;
pub use indexing_service::IndexingService;
