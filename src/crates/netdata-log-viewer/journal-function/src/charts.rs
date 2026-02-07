//! Chart definitions for journal-function metrics
//!
//! This module contains Netdata chart metric structures that track
//! file indexing performance and cache utilization.

use rt::{ChartHandle, NetdataChart, StdPluginRuntime};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Container for all journal-function metrics chart handles
pub struct JournalMetrics {
    pub file_indexing: ChartHandle<FileIndexingMetrics>,
    pub bucket_cache: ChartHandle<BucketCacheMetrics>,
    pub bucket_operations: ChartHandle<BucketOperationsMetrics>,
    pub foyer_memory_state: ChartHandle<FoyerMemoryStateMetrics>,
    pub foyer_memory_events: ChartHandle<FoyerMemoryEventsMetrics>,
    pub foyer_disk_io: ChartHandle<FoyerDiskIoMetrics>,
    pub foyer_disk_ops: ChartHandle<FoyerDiskOpsMetrics>,
}

impl JournalMetrics {
    /// Register all metric charts with the plugin runtime
    pub fn new(runtime: &mut StdPluginRuntime) -> Self {
        Self {
            file_indexing: runtime
                .register_chart(FileIndexingMetrics::default(), Duration::from_secs(1)),
            bucket_cache: runtime
                .register_chart(BucketCacheMetrics::default(), Duration::from_secs(1)),
            bucket_operations: runtime
                .register_chart(BucketOperationsMetrics::default(), Duration::from_secs(1)),
            foyer_memory_state: runtime
                .register_chart(FoyerMemoryStateMetrics::default(), Duration::from_secs(1)),
            foyer_memory_events: runtime
                .register_chart(FoyerMemoryEventsMetrics::default(), Duration::from_secs(1)),
            foyer_disk_io: runtime
                .register_chart(FoyerDiskIoMetrics::default(), Duration::from_secs(1)),
            foyer_disk_ops: runtime
                .register_chart(FoyerDiskOpsMetrics::default(), Duration::from_secs(1)),
        }
    }
}

/// Metrics for tracking file indexing operations
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.file_indexing"),
    extend("x-chart-title" = "Journal File Indexing Operations"),
    extend("x-chart-units" = "indexes/s"),
    extend("x-chart-type" = "line"),
    extend("x-chart-family" = "indexing"),
    extend("x-chart-context" = "journal.file_indexing"),
)]
pub struct FileIndexingMetrics {
    /// Number of new file indexes computed (cache miss)
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub computed: u64,
    /// Number of file indexes retrieved from cache (cache hit)
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub cached: u64,
}

/// Metrics for tracking bucket response cache state
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.bucket_cache"),
    extend("x-chart-title" = "Bucket Response Cache"),
    extend("x-chart-units" = "buckets"),
    extend("x-chart-type" = "stacked"),
    extend("x-chart-family" = "cache"),
    extend("x-chart-context" = "journal.bucket_cache"),
)]
pub struct BucketCacheMetrics {
    /// Number of partial bucket responses in cache (still indexing)
    #[schemars(extend("x-dimension-algorithm" = "absolute"))]
    pub partial: u64,
    /// Number of complete bucket responses in cache (fully indexed)
    #[schemars(extend("x-dimension-algorithm" = "absolute"))]
    pub complete: u64,
}

/// Metrics for tracking bucket response operations
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.bucket_operations"),
    extend("x-chart-title" = "Bucket Response Operations"),
    extend("x-chart-units" = "buckets/s"),
    extend("x-chart-type" = "line"),
    extend("x-chart-family" = "operations"),
    extend("x-chart-context" = "journal.bucket_operations"),
)]
pub struct BucketOperationsMetrics {
    /// Buckets served as complete from cache
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub served_complete: u64,
    /// Buckets served as partial (still indexing)
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub served_partial: u64,
    /// Partial buckets promoted to complete
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub promoted: u64,
    /// Buckets created (new partial responses)
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub created: u64,
    /// Buckets invalidated (removed because covering current time)
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub invalidated: u64,
}

// ============================================================================
// Foyer Cache Metrics
// ============================================================================

/// Foyer in-memory cache usage and capacity
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.foyer_memory_state"),
    extend("x-chart-title" = "Foyer In-Memory Cache State"),
    extend("x-chart-units" = "bytes"),
    extend("x-chart-type" = "stacked"),
    extend("x-chart-family" = "foyer_cache"),
    extend("x-chart-context" = "journal.foyer_memory_state"),
)]
pub struct FoyerMemoryStateMetrics {
    /// Current memory usage
    #[schemars(extend("x-dimension-algorithm" = "absolute"))]
    pub usage: u64,
    /// Maximum memory capacity
    #[schemars(extend("x-dimension-algorithm" = "absolute"))]
    pub capacity: u64,
}

/// Foyer in-memory cache eviction events
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.foyer_memory_events"),
    extend("x-chart-title" = "Foyer In-Memory Cache Events"),
    extend("x-chart-units" = "events/s"),
    extend("x-chart-type" = "line"),
    extend("x-chart-family" = "foyer_cache"),
    extend("x-chart-context" = "journal.foyer_memory_events"),
)]
pub struct FoyerMemoryEventsMetrics {
    /// Entries evicted to make room for new ones
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub evictions: u64,
    /// Entries replaced on insertion
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub replacements: u64,
    /// Entries explicitly removed
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub removals: u64,
}

/// Foyer disk cache I/O throughput
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.foyer_disk_io"),
    extend("x-chart-title" = "Foyer Disk Cache I/O"),
    extend("x-chart-units" = "bytes/s"),
    extend("x-chart-type" = "area"),
    extend("x-chart-family" = "foyer_cache"),
    extend("x-chart-context" = "journal.foyer_disk_io"),
)]
pub struct FoyerDiskIoMetrics {
    /// Bytes written to disk cache
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub write_bytes: u64,
    /// Bytes read from disk cache
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub read_bytes: u64,
}

/// Foyer disk cache I/O operations
#[derive(JsonSchema, NetdataChart, Default, Clone, PartialEq, Serialize, Deserialize)]
#[schemars(
    extend("x-chart-id" = "journal.foyer_disk_ops"),
    extend("x-chart-title" = "Foyer Disk Cache Operations"),
    extend("x-chart-units" = "ops/s"),
    extend("x-chart-type" = "line"),
    extend("x-chart-family" = "foyer_cache"),
    extend("x-chart-context" = "journal.foyer_disk_ops"),
)]
pub struct FoyerDiskOpsMetrics {
    /// Write I/O operations
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub write_ops: u64,
    /// Read I/O operations
    #[schemars(extend("x-dimension-algorithm" = "incremental"))]
    pub read_ops: u64,
}
