//! Cache types for journal file indexes

use crate::facets::Facets;
use foyer::HybridCache;
use journal_index::{FieldName, FileIndex};
use journal_registry::File;
use serde::{Deserialize, Serialize};

/// Cache version number. Increment this when the FileIndex or FileIndexKey
/// schema changes to automatically invalidate old cache entries.
const CACHE_VERSION: u32 = 1;

/// Cache key for file indexes that includes the file, facets, source timestamp
/// field, and cache version. Different facet configurations or timestamp fields
/// produce different indexes, so all are needed to uniquely identify a cached
/// index. The version ensures that schema changes automatically invalidate old
/// cache entries.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIndexKey {
    version: u32,
    pub file: File,
    pub(crate) facets: Facets,
    pub(crate) source_timestamp_field: Option<FieldName>,
}

impl FileIndexKey {
    pub fn new(file: &File, facets: &Facets, source_timestamp_field: Option<FieldName>) -> Self {
        Self {
            version: CACHE_VERSION,
            file: file.clone(),
            facets: facets.clone(),
            source_timestamp_field,
        }
    }
}

/// Type alias for the inner foyer HybridCache.
type InnerCache = HybridCache<FileIndexKey, FileIndex>;

/// File index cache wrapping foyer's HybridCache.
#[derive(Clone)]
pub struct FileIndexCache {
    inner: InnerCache,
}

impl FileIndexCache {
    /// Creates a new cache from the inner foyer cache.
    pub(crate) fn new(inner: InnerCache) -> Self {
        Self { inner }
    }

    /// Inserts a key-value pair into the cache.
    pub fn insert(&self, key: FileIndexKey, value: FileIndex) {
        self.inner.insert(key, value);
    }

    /// Gets a value from the cache by key.
    pub async fn get(&self, key: &FileIndexKey) -> foyer::Result<Option<FileIndex>> {
        self.inner
            .get(key)
            .await
            .map(|entry| entry.map(|e| e.value().clone()))
    }

    /// Returns the memory usage in bytes (sum of all cached item weights).
    pub fn memory_usage(&self) -> usize {
        self.inner.memory().usage()
    }

    /// Returns the memory capacity in bytes.
    pub fn memory_capacity(&self) -> usize {
        self.inner.memory().capacity()
    }

    /// Closes the cache, flushing pending writes and shutting down I/O tasks.
    pub async fn close(&self) -> foyer::Result<()> {
        self.inner.close().await
    }
}
