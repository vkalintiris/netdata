//! Benchmark for batch_compute_file_indexes.
//!
//! Points at a directory of journal files, indexes them, and prints bitmap
//! statistics useful for comparing the roaring vs treight backends.
//!
//! Run with roaring (default):
//!     cargo run --release -p journal-engine --example index -- /var/log/journal
//!
//! Run with treight:
//!     cargo run --release -p journal-engine --example index \
//!         --no-default-features --features bitmap-treight -- /var/log/journal

// # 1. Create a mount point
//
// dd if=/dev/zero of=/tmp/slow-disk.img bs=1G count=100
// LOOP=$(sudo losetup -f --show /tmp/slow-disk.img)
// sudo mkfs.ext4 $LOOP
// sudo mkdir -p /mnt/slow-disk
// sudo mount $LOOP /mnt/slow-disk
// sudo chown $USER:$USER /mnt/slow-disk
//
// # 2. Copy journal files
// cp -r ~/repos/tmp/otel-aws /mnt/slow-disk/
//
// # 3. Unmount and recreate with delay
// sudo umount /mnt/slow-disk
// SIZE=$(sudo blockdev --getsz $LOOP)
// sudo dmsetup create slow-disk --table "0 $SIZE delay $LOOP 0 50 $LOOP 0 50"
// sudo mount /dev/mapper/slow-disk /mnt/slow-disk
//
// # 4. Now /mnt/slow-disk/otel-aws has your journals on a "slow" disk
//
// # 5. Create slow-io cgroup
// sudo mkdir -p /sys/fs/cgroup/slow-io
// echo "+io" | sudo tee /sys/fs/cgroup/cgroup.controllers
// # Find your device's major:minor (e.g., for nvme0n1)
// cat /sys/block/nvme0n1/dev
// # Let's say it's 259:0, Set a 10MB/s read and write limit
// echo "259:0 rbps=10485760 wbps=10485760" | sudo tee /sys/fs/cgroup/slow-io/io.max

use journal_engine::{
    Facets, FileIndexCacheBuilder, FileIndexKey, IndexingLimits, QueryTimeRange,
    batch_compute_file_indexes,
};
use journal_index::{FieldName, FileIndex};
use journal_registry::{Monitor, Registry};
use std::env;
use std::path::PathBuf;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[allow(unused_imports)]
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Stats collection
// ---------------------------------------------------------------------------

/// Per-file bitmap statistics.
#[derive(Debug, Clone, Default)]
#[allow(dead_code)]
struct FileStats {
    /// Path of the journal file.
    path: String,
    /// Total journal entries in this file.
    entries: usize,
    /// Number of field=value bitmaps.
    bitmap_count: usize,
    /// Total heap bytes across all bitmaps.
    heap_bytes: u64,
    /// Number of bitmaps using inverted (complement) representation.
    /// Always 0 under the roaring backend.
    inverted_count: usize,
}

/// Aggregated statistics for the full indexing run.
#[derive(Debug, Clone, Default)]
struct RunStats {
    /// Which backend was used.
    backend: String,
    /// Wall-clock time for the batch indexing call.
    index_duration: Duration,
    /// Process RSS after indexing (bytes).
    rss_bytes: Option<u64>,
    /// Per-file breakdown.
    files: Vec<FileStats>,
}

impl RunStats {
    fn total_files(&self) -> usize {
        self.files.len()
    }

    fn total_entries(&self) -> usize {
        self.files.iter().map(|f| f.entries).sum()
    }

    fn total_bitmaps(&self) -> usize {
        self.files.iter().map(|f| f.bitmap_count).sum()
    }

    fn total_heap_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.heap_bytes).sum()
    }

    fn total_inverted(&self) -> usize {
        self.files.iter().map(|f| f.inverted_count).sum()
    }
}

fn backend_name() -> &'static str {
    #[cfg(feature = "bitmap-treight")]
    {
        "treight"
    }
    #[cfg(not(feature = "bitmap-treight"))]
    {
        "roaring"
    }
}

fn collect_file_stats(path: &str, file_index: &FileIndex) -> FileStats {
    let mut stats = FileStats {
        path: path.to_string(),
        entries: file_index.total_entries(),
        ..Default::default()
    };

    for (_fv, bitmap) in file_index.bitmaps() {
        stats.bitmap_count += 1;
        let _ = &bitmap;

        #[cfg(feature = "allocative")]
        {
            stats.heap_bytes += allocative::size_of_unique_allocated_data(bitmap) as u64;
        }

        #[cfg(feature = "bitmap-treight")]
        if bitmap.0.is_inverted() {
            stats.inverted_count += 1;
        }
    }

    stats
}

/// Load facet names from a JSON file containing a `columns` object.
fn load_facets_from_json(path: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let content = std::fs::read_to_string(path)?;
    let data: serde_json::Value = serde_json::from_str(&content)?;
    let columns = data.get("columns").ok_or("missing 'columns' key in JSON")?;

    let names: Vec<String> = match columns {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        serde_json::Value::Array(arr) => arr
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect(),
        _ => return Err("'columns' must be an object or array".into()),
    };

    Ok(names)
}

fn rss_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            let kb: u64 = value.trim().trim_end_matches(" kB").trim().parse().ok()?;
            return Some(kb * 1024);
        }
    }
    None
}

fn fmt_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

fn print_stats(stats: &RunStats) {
    let total_bitmaps = stats.total_bitmaps();

    println!();
    println!("=== Bitmap stats ({}) ===", stats.backend);
    println!("  files:        {}", stats.total_files());
    println!("  entries:      {}", stats.total_entries());
    println!("  bitmaps:      {total_bitmaps}");
    println!("  heap total:   {}", fmt_bytes(stats.total_heap_bytes()));

    if total_bitmaps > 0 {
        println!(
            "  heap/bitmap:  {:.0} B",
            stats.total_heap_bytes() as f64 / total_bitmaps as f64
        );

        let inverted = stats.total_inverted();
        if inverted > 0 {
            println!(
                "  inverted:     {} ({:.1}%)",
                inverted,
                inverted as f64 / total_bitmaps as f64 * 100.0
            );
        }
    }

    println!("  index time:   {:.2?}", stats.index_duration);
    if let Some(rss) = stats.rss_bytes {
        println!("  process RSS:  {}", fmt_bytes(rss));
    }
    println!();
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .init();

    // Parse args: [DIR] [--facets-json PATH]
    let args: Vec<String> = env::args().skip(1).collect();
    let mut dir = PathBuf::from("/mnt/slow-disk/otel-aws");
    let mut facets_json: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        if args[i] == "--facets-json" {
            i += 1;
            facets_json = Some(args[i].clone());
        } else {
            dir = PathBuf::from(&args[i]);
        }
        i += 1;
    }

    info!("scanning directory: {}", dir.display());
    info!("bitmap backend: {}", backend_name());

    // Create registry and scan directory
    let (monitor, _event_receiver) = Monitor::new()?;
    let registry = Registry::new(monitor);

    registry.watch_directory(dir.to_str().unwrap())?;

    // Find all files
    let files = registry.find_files_in_range(
        journal_common::Seconds(0),
        journal_common::Seconds(u32::MAX),
    )?;

    info!("found {} journal files", files.len());
    if files.is_empty() {
        return Ok(());
    }
    // files.truncate(1);

    // Create file index cache with a fresh temp directory to avoid cross-backend contamination
    let cache_dir = tempfile::tempdir()?;
    let cache = FileIndexCacheBuilder::new()
        .with_cache_path(cache_dir.path().to_str().unwrap())
        .with_memory_capacity(1)
        .with_disk_capacity(8 * 1024 * 1024)
        .with_block_size(4 * 1024 * 1024)
        .build()
        .await?;

    info!("created file index cache");

    // Load facets from JSON file or use defaults
    let facet_names = if let Some(ref path) = facets_json {
        let names = load_facets_from_json(path)?;
        info!("loaded {} facets from {}", names.len(), path);
        names
    } else {
        vec!["PRIORITY".to_string(), "SYSLOG_IDENTIFIER".to_string()]
    };
    let facets = Facets::new(&facet_names);
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();

    let keys: Vec<FileIndexKey> = files
        .iter()
        .map(|file_info| {
            FileIndexKey::new(
                &file_info.file,
                &facets,
                Some(source_timestamp_field.clone()),
            )
        })
        .collect();

    // Create a time range for indexing (24 hours)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as u32;
    let time_range = QueryTimeRange::new(now - 3600 * 24 * 365, now)?;
    let cancellation = CancellationToken::new();

    info!(
        "computing {} file indexes, bucket duration: {}s",
        keys.len(),
        time_range.bucket_duration()
    );

    let indexing_limits = IndexingLimits {
        max_unique_values_per_field: 100,
        ..Default::default()
    };

    // Run batch indexing
    let start = std::time::Instant::now();
    let responses = batch_compute_file_indexes(
        &cache,
        &registry,
        keys,
        &time_range,
        cancellation,
        indexing_limits,
        None,
    )
    .await?;

    let index_duration = start.elapsed();

    // Collect stats.
    let mut run_stats = RunStats {
        backend: backend_name().to_string(),
        index_duration,
        rss_bytes: rss_bytes(),
        ..Default::default()
    };

    for (key, file_index) in &responses {
        run_stats
            .files
            .push(collect_file_stats(key.file.path(), file_index));
    }

    print_stats(&run_stats);

    // Close the cache to flush and shut down I/O tasks gracefully
    cache.close().await?;

    Ok(())
}
