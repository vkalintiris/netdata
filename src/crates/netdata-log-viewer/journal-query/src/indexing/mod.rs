//! File indexing infrastructure with background indexing workers.
//!
//! This module provides two main abstractions:
//! - [`HybridCache`]: A generic wrapper around foyer's hybrid cache
//! - [`IndexingService`]: High-level service with background worker pool for concurrent indexing

pub mod hybrid_cache;
pub mod indexing_service;

pub use hybrid_cache::{CacheGetResult, HybridCache};
pub use indexing_service::{IndexingService, IndexingRequest};

// Type alias for our specific use case
#[cfg(feature = "allocative")]
use allocative::Allocative;
use journal::index::FileIndex;
use journal::repository::File;

use crate::request::HistogramFacets;

/// Cache key for file indexes that includes both the file and the facets.
/// Different facet configurations produce different indexes, so both are needed
/// to uniquely identify a cached index.
///
/// The HistogramFacets struct uses Arc<Vec<String>> for cheap cloning and implements
/// Hash using a precomputed hash, making this key both memory-efficient and fast to hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndexKey {
    pub file: File,
    pub facets: HistogramFacets,
}

impl FileIndexKey {
    pub fn new(file: File, facets: HistogramFacets) -> Self {
        Self { file, facets }
    }
}

impl serde::Serialize for FileIndexKey {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("FileIndexKey", 2)?;
        state.serialize_field("file", &self.file)?;
        // Serialize the facets vector
        state.serialize_field("facets", self.facets.as_slice())?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for FileIndexKey {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct FileIndexKeyHelper {
            file: File,
            facets: Vec<String>,
        }

        let helper = FileIndexKeyHelper::deserialize(deserializer)?;
        // Reconstruct HistogramFacets from the deserialized facets
        let histogram_facets = HistogramFacets::new(&helper.facets);

        Ok(FileIndexKey {
            file: helper.file,
            facets: histogram_facets,
        })
    }
}

/// Specialized hybrid cache wrapper for file indexes.
pub(crate) type FileIndexCache = HybridCache<FileIndexKey, FileIndex>;
