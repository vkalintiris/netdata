//! Generic wrapper around foyer::HybridCache.
//!
//! This module provides a clean, domain-agnostic API over foyer's hybrid cache,
//! hiding implementation details and providing better error handling.

use foyer::{StorageKey, StorageValue};
use std::hash::Hash;
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

/// Statistics about the hybrid cache
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub memory_usage_bytes: usize,
    pub memory_capacity_bytes: usize,
    pub disk_write_bytes: usize,
    pub disk_read_bytes: usize,
    pub disk_write_ios: usize,
    pub disk_read_ios: usize,
    pub is_hybrid: bool,
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
    K: StorageKey + Clone,
    V: StorageValue + Clone,
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
    pub async fn get_with_timeout(
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

    /// Attempts to get a value from cache without timeout.
    pub async fn get(&self, key: &K) -> crate::error::Result<CacheGetResult<Arc<V>>> {
        match self.inner.get(key).await {
            Ok(Some(entry)) => Ok(CacheGetResult::Hit(Arc::new((*entry.value()).clone()))),
            Ok(None) => Ok(CacheGetResult::Miss),
            Err(e) => Err(e.into()),
        }
    }

    /// Returns statistics about the cache
    pub fn statistics(&self) -> CacheStats {
        let is_hybrid = self.inner.is_hybrid();
        let stats = self.inner.statistics();

        CacheStats {
            memory_usage_bytes: self.inner.memory().usage(),
            memory_capacity_bytes: self.inner.memory().capacity(),
            disk_write_bytes: if is_hybrid { stats.disk_write_bytes() } else { 0 },
            disk_read_bytes: if is_hybrid { stats.disk_read_bytes() } else { 0 },
            disk_write_ios: if is_hybrid { stats.disk_write_ios() } else { 0 },
            disk_read_ios: if is_hybrid { stats.disk_read_ios() } else { 0 },
            is_hybrid,
        }
    }

    /// Checks if the cache is running in hybrid mode (memory + disk)
    pub fn is_hybrid(&self) -> bool {
        self.inner.is_hybrid()
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
