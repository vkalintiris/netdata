//! File index cache integration types.
//!
//! This module defines the cache key and type alias for the file index cache,
//! which stores indexed journal files keyed by (File, Facets) pairs.

use super::Facets;
use super::hybrid_cache::HybridCache;
use journal::index::FileIndex;
use journal::repository::File;

#[cfg(feature = "allocative")]
use allocative::Allocative;

use serde::{Deserialize, Serialize};

/// Cache key for file indexes that includes both the file and the facets.
/// Different facet configurations produce different indexes, so both are needed
/// to uniquely identify a cached index.
///
/// The Facets struct uses an atomic reference counter for cheap cloning and implements
/// Hash using a precomputed hash, making this key both memory-efficient and fast to hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub(crate) struct FileIndexKey {
    pub(crate) file: File,
    pub(crate) facets: Facets,
}

impl FileIndexKey {
    pub(crate) fn new(file: File, facets: Facets) -> Self {
        Self { file, facets }
    }
}

/// Specialized hybrid cache wrapper for file indexes.
pub(crate) type FileIndexCache = HybridCache<FileIndexKey, FileIndex>;
