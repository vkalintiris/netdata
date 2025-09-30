use foyer::{BlockEngineBuilder, DeviceBuilder, FsDeviceBuilder, HybridCache, HybridCacheBuilder};
use journal_file::index::FileIndex;
use std::path::{Path, PathBuf};
use tracing::{info, warn};

/// A hybrid cache for storing JournalFileIndex with configurable disk storage limit
pub struct JournalIndexCache {
    cache: HybridCache<String, FileIndex>,
}

impl JournalIndexCache {
    /// Create a new JournalIndexCache with the specified disk capacity in bytes
    ///
    /// # Arguments
    /// * `disk_capacity` - Maximum disk space to use for the cache in bytes
    /// * `memory_capacity` - Memory cache size in bytes (default: 8 MiB)
    ///
    /// # Example
    /// ```rust
    /// let cache = JournalIndexCache::new(32 * 1024 * 1024, Some(8 * 1024 * 1024)).await?;
    /// ```
    pub async fn new(
        path: PathBuf,
        disk_capacity: u64,
        memory_capacity: Option<u64>,
    ) -> anyhow::Result<Self> {
        // let temp_dir = Arc::new(tempfile::tempdir()?);
        let memory_size = memory_capacity.unwrap_or(8 * 1024 * 1024); // Default 8 MiB

        info!(
            "Creating journal index cache with {}MB disk capacity",
            disk_capacity / (1024 * 1024)
        );

        // Create filesystem device with specified capacity
        let device = FsDeviceBuilder::new(path)
            .with_capacity(disk_capacity.try_into().unwrap())
            .build()?;

        // Build hybrid cache with block-based storage
        let cache: HybridCache<String, FileIndex> = HybridCacheBuilder::new()
            .with_name("journal-index-cache")
            .memory(memory_size.try_into().unwrap())
            .storage()
            .with_engine_config(BlockEngineBuilder::new(device).with_block_size(1024 * 1024))
            .build()
            .await?;

        Ok(Self { cache })
    }

    /// Insert a JournalFileIndex into the cache
    ///
    /// # Arguments
    /// * `file_path` - Path to the journal file (used as cache key)
    /// * `index` - The JournalFileIndex to cache
    pub fn insert<P: AsRef<Path>>(&self, file_path: P, index: FileIndex) {
        let key = file_path.as_ref().to_string_lossy().to_string();
        self.cache.insert(key, index);
    }

    /// Retrieve a JournalFileIndex from the cache
    ///
    /// # Arguments
    /// * `file_path` - Path to the journal file
    ///
    /// # Returns
    /// * `Some(JournalFileIndex)` if found in cache
    /// * `None` if not found
    pub async fn get<P: AsRef<Path>>(&self, file_path: P) -> anyhow::Result<Option<FileIndex>> {
        let key = file_path.as_ref().to_string_lossy().to_string();
        match self.cache.get(&key).await? {
            Some(entry) => Ok(Some(entry.value().clone())),
            None => Ok(None),
        }
    }

    /// Check if a file path exists in the cache
    pub async fn contains<P: AsRef<Path>>(&self, file_path: P) -> anyhow::Result<bool> {
        let key = file_path.as_ref().to_string_lossy().to_string();
        Ok(self.cache.get(&key).await?.is_some())
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        // Note: foyer doesn't expose detailed stats in a simple way,
        // so we'll implement basic tracking if needed
        CacheStats {
            // These would need to be tracked separately or extracted from foyer's metrics
            memory_entries: 0,
            disk_entries: 0,
            memory_bytes: 0,
            disk_bytes: 0,
        }
    }

    /// Clear the entire cache
    pub async fn clear(&self) -> anyhow::Result<()> {
        // foyer doesn't have a direct clear method, so we'd need to track keys
        // or recreate the cache instance
        warn!("Cache clear not implemented - would require recreating cache instance");
        Ok(())
    }

    /// Close the cache and clean up resources
    pub async fn close(self) -> anyhow::Result<()> {
        self.cache.close().await?;
        Ok(())
    }
}

/// Statistics about cache usage
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub memory_entries: usize,
    pub disk_entries: usize,
    pub memory_bytes: u64,
    pub disk_bytes: u64,
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//     use journal_file::index::{Histogram, JournalFileIndex};
//     use std::collections::HashMap;

//     #[tokio::test]
//     async fn test_cache_basic_operations() -> anyhow::Result<()> {
//         let cache = JournalIndexCache::new(1024 * 1024, Some(512 * 1024)).await?;

//         // Create a test index
//         let test_index = JournalFileIndex {
//             histogram: Histogram::default(),
//             entry_indices: HashMap::new(),
//         };

//         let test_path = "/test/journal/file.journal";

//         // Test insert and get
//         cache.insert(test_path, test_index.clone());

//         // Allow time for async operations
//         tokio::time::sleep(std::time::Duration::from_millis(10)).await;

//         let retrieved = cache.get(test_path).await?;
//         assert!(retrieved.is_some());

//         // Test contains
//         let exists = cache.contains(test_path).await?;
//         assert!(exists);

//         // Test non-existent key
//         let not_found = cache.get("/non/existent/path").await?;
//         assert!(not_found.is_none());

//         cache.close().await?;
//         Ok(())
//     }

//     #[tokio::test]
//     async fn test_cache_capacity_limit() -> anyhow::Result<()> {
//         // Create cache with very small capacity to test limits
//         let cache = JournalIndexCache::new(1024, Some(256)).await?;

//         // This test would need more sophisticated setup to actually test capacity limits
//         // For now, just verify the cache can be created with small limits

//         cache.close().await?;
//         Ok(())
//     }
// }
