//! Generic wrapper around foyer::HybridCache.
//!
//! This module provides a clean, domain-agnostic API over foyer's hybrid cache,
//! hiding implementation details and providing better error handling.

use foyer::{StorageKey, StorageValue};
use std::sync::Arc;
use std::time::Duration;

/// Result type for cache get operations, simplifying the nested Result/Option from foyer
#[derive(Debug)]
pub enum CacheGetResult<T> {
    /// Entry found in cache
    Hit(T),
    /// Entry not in cache
    Miss,
}

/// Generic wrapper around foyer::HybridCache.
///
/// This wrapper provides a clean, domain-agnostic API over foyer's cache,
/// hiding implementation details and providing better error handling.
///
/// # Type Parameters
/// * `K` - Key type (must implement StorageKey from foyer)
/// * `V` - Value type (must implement StorageValue from foyer)
pub struct HybridCache<K, V>
where
    K: StorageKey,
    V: StorageValue,
{
    inner: foyer::HybridCache<K, V>,
}

impl<K, V> HybridCache<K, V>
where
    K: StorageKey + Clone,
    V: StorageValue + Clone,
{
    /// Creates a new HybridCache wrapping the given foyer cache
    pub fn new(inner: foyer::HybridCache<K, V>) -> Self {
        Self { inner }
    }

    /// Attempts to get a value from cache with a timeout.
    ///
    /// This method wraps foyer's complex nested Result/Option into a simpler API.
    /// Returns Hit with the value if found, Miss if not in cache, or an error on failure.
    pub async fn get(
        &self,
        key: &K,
        timeout_duration: Duration,
    ) -> crate::error::Result<CacheGetResult<Arc<V>>> {
        use tokio::time::timeout;

        match timeout(timeout_duration, self.inner.obtain(key.clone())).await {
            Ok(Ok(Some(entry))) => Ok(CacheGetResult::Hit(Arc::new((*entry.value()).clone()))),
            Ok(Ok(None)) => Ok(CacheGetResult::Miss),
            Ok(Err(e)) => Err(e.into()),
            Err(_) => Ok(CacheGetResult::Miss), // Treat timeout as cache miss
        }
    }

    /// Gracefully closes the cache, flushing pending writes
    pub async fn close(&self) -> crate::error::Result<()> {
        self.inner.close().await?;
        Ok(())
    }

    /// Provides direct access to the underlying foyer cache.
    ///
    /// This is useful when you need to pass the cache to workers or other
    /// low-level components that work directly with foyer.
    pub(crate) fn inner(&self) -> &foyer::HybridCache<K, V> {
        &self.inner
    }
}
