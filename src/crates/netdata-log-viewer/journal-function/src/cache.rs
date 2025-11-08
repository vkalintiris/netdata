//! Generic cache abstraction supporting both in-memory and disk-backed storage
//!
//! This module provides a generic `Cache<K, V>` enum that can use either a Foyer
//! HybridCache (with memory + disk eviction) or a simple HashMap (no eviction).
//! It also defines the specialized FileIndexCache type for caching journal file indexes.

use foyer::{HybridCache, StorageKey, StorageValue};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use thiserror::Error;

/// Errors that can occur with cache operations
#[derive(Debug, Error)]
pub enum CacheError {
    /// Error from the foyer cache
    #[error("Cache error: {0}")]
    Foyer(#[from] foyer::Error),

    /// Lock poisoning error
    #[error("Lock poisoned: {0}")]
    LockPoisoned(String),
}

/// A specialized Result type for cache operations
pub type Result<T> = std::result::Result<T, CacheError>;

/// An enum that can hold either a foyer HybridCache or a standard HashMap.
/// This allows runtime selection between an evicting cache and a non-evicting store.
pub enum Cache<K, V>
where
    K: StorageKey,
    V: StorageValue,
{
    /// Evicting cache backed by memory and optionally disk
    Foyer(HybridCache<K, V>),
    /// Non-evicting in-memory store
    HashMap(Arc<RwLock<HashMap<K, V>>>),
}

impl<K, V> Cache<K, V>
where
    K: StorageKey + Clone,
    V: StorageValue + Clone,
{
    /// Create a cache backed by a foyer HybridCache instance
    pub fn with_foyer(cache: HybridCache<K, V>) -> Self {
        Cache::Foyer(cache)
    }

    /// Create a cache backed by a HashMap instance
    pub fn with_hashmap(map: HashMap<K, V>) -> Self {
        Cache::HashMap(Arc::new(RwLock::new(map)))
    }

    /// Get a value from the cache
    pub async fn get(&self, key: &K) -> Result<Option<V>> {
        match self {
            Cache::Foyer(cache) => Ok(cache.get(key).await?.map(|entry| entry.value().clone())),
            Cache::HashMap(map) => {
                let guard = map
                    .read()
                    .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                Ok(guard.get(key).cloned())
            }
        }
    }

    /// Synchronously get a value from the cache.
    ///
    /// For HashMap cache: performs a synchronous read.
    /// For Foyer cache: returns None (cannot do sync reads from async cache).
    pub fn get_sync(&self, key: &K) -> Result<Option<V>> {
        match self {
            Cache::Foyer(_) => {
                // Cannot do sync reads from Foyer cache - return None
                Ok(None)
            }
            Cache::HashMap(map) => {
                let guard = map
                    .read()
                    .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                Ok(guard.get(key).cloned())
            }
        }
    }

    /// Insert a key-value pair into the cache
    pub fn insert(&self, key: K, value: V) -> Result<()> {
        match self {
            Cache::Foyer(cache) => {
                cache.insert(key, value);
                Ok(())
            }
            Cache::HashMap(map) => {
                let mut guard = map
                    .write()
                    .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                guard.insert(key, value);
                Ok(())
            }
        }
    }

    /// Remove a key from the cache
    pub fn remove(&self, key: &K) -> Result<()> {
        match self {
            Cache::Foyer(cache) => {
                cache.remove(key);
                Ok(())
            }
            Cache::HashMap(map) => {
                let mut guard = map
                    .write()
                    .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                guard.remove(key);
                Ok(())
            }
        }
    }

    /// Check if the cache contains a key
    pub fn contains(&self, key: &K) -> Result<bool> {
        match self {
            Cache::Foyer(cache) => Ok(cache.contains(key)),
            Cache::HashMap(map) => {
                let guard = map
                    .read()
                    .map_err(|e| CacheError::LockPoisoned(e.to_string()))?;
                Ok(guard.contains_key(key))
            }
        }
    }
}

impl<K, V> Clone for Cache<K, V>
where
    K: StorageKey,
    V: StorageValue,
{
    fn clone(&self) -> Self {
        match self {
            Cache::Foyer(cache) => Cache::Foyer(cache.clone()),
            Cache::HashMap(map) => Cache::HashMap(map.clone()),
        }
    }
}

// ============================================================================
// File Index Cache
// ============================================================================

use super::{Facets, File};
use journal::index::FileIndex;

/// Cache key for file indexes that includes both the file and the facets.
/// Different facet configurations produce different indexes, so both are
/// needed to uniquely identify a cached index.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileIndexKey {
    pub(crate) file: File,
    pub(crate) facets: Facets,
}

impl FileIndexKey {
    pub(crate) fn new(file: &File, facets: &Facets) -> Self {
        Self {
            file: file.clone(),
            facets: facets.clone(),
        }
    }
}

/// Type alias for file index cache.
pub type FileIndexCache = Cache<FileIndexKey, FileIndex>;
