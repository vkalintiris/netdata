//! File index caching with background indexing workers.
//!
//! This module provides two main abstractions:
//! - [`HybridCache`]: A generic wrapper around foyer's hybrid cache
//! - [`IndexCache`]: High-level cache with background worker pool for concurrent indexing

pub mod hybrid_cache;
pub mod index_cache;

pub use hybrid_cache::{CacheGetResult, HybridCache};
pub use index_cache::{IndexCache, IndexingRequest};

// Type alias for our specific use case
use journal::index::FileIndex;
use journal::repository::File;

/// Specialized hybrid cache for file indexes
pub type FileIndexCache = HybridCache<File, FileIndex>;
