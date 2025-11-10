use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use clap::Parser;
use journal::index::Filter;
use journal::index::{FieldName, FieldValuePair, FileIndex};
use journal_function::*;
use rt::StdPluginRuntime;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about = "Test log retrieval functionality")]
struct Args {
    /// Start time (e.g., "2025-11-10 12:50:15")
    #[arg(long)]
    since: String,

    /// End time (e.g., "2025-11-10 12:51:00")
    #[arg(long)]
    until: String,

    /// Journal directories (e.g., /var/log/journal). Can be specified multiple times.
    #[arg(long = "directory", required = true)]
    directories: Vec<PathBuf>,

    /// Direction: forward or backward
    #[arg(long, default_value = "forward")]
    direction: String,
}

fn parse_datetime(s: &str) -> std::result::Result<u64, String> {
    // Try parsing as "YYYY-MM-DD HH:MM:SS"
    let naive = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map_err(|e| format!("Failed to parse datetime: {}", e))?;

    // Convert to local timezone
    let local: DateTime<Local> = Local
        .from_local_datetime(&naive)
        .single()
        .ok_or("Ambiguous datetime")?;

    // Return as seconds since epoch
    Ok(local.timestamp() as u64)
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();

    // Parse timestamps
    let since_secs = parse_datetime(&args.since)? as u32;
    let until_secs = parse_datetime(&args.until)? as u32;

    info!(
        "Time range: {} - {} ({} - {} seconds since epoch)",
        args.since, args.until, since_secs, until_secs
    );

    // Initialize monitoring and registry
    info!("\n[1] Initializing file system monitoring and registry...");
    let (monitor, _notify_rx) = Monitor::new()?;
    let registry = Registry::new(monitor);

    // Watch all journal directories
    info!(
        "[2] Scanning {} journal directories...",
        args.directories.len()
    );
    for directory in &args.directories {
        info!("    Watching: {:?}", directory);
        registry
            .watch_directory(directory.to_str().unwrap())
            .map_err(|e| format!("Failed to watch directory {:?}: {}", directory, e))?;
    }

    // Find files in the time range
    let file_infos = registry
        .find_files_in_range(since_secs, until_secs)
        .map_err(|e| format!("Failed to find files in range: {}", e))?;

    info!(
        "[3] Found {} files in time range [{}, {})",
        file_infos.len(),
        since_secs,
        until_secs
    );

    if file_infos.is_empty() {
        println!("\nNo files found in the specified time range.");
        return Ok(());
    }

    // Create FileIndexCache with HashMap variant
    info!("[4] Creating in-memory file index cache...");
    let file_index_cache = FileIndexCache::with_hashmap(HashMap::new());

    // Initialize minimal runtime for metrics
    info!("[5] Initializing plugin runtime for metrics...");
    let mut runtime = StdPluginRuntime::new("log-retrieval-test");
    let file_indexing_metrics =
        runtime.register_chart(FileIndexingMetrics::default(), Duration::from_secs(1));

    // Create IndexingService
    info!("[6] Creating indexing service...");
    let indexing_service = IndexingService::new(
        file_index_cache.clone(),
        registry.clone(),
        4,   // 4 workers
        100, // queue capacity
        file_indexing_metrics.clone(),
    );

    // Build file index keys (with empty facets for now)
    let facets = Facets::new(&[]);
    let keys: Vec<FileIndexKey> = file_infos
        .iter()
        .map(|fi| FileIndexKey::new(&fi.file, &facets))
        .collect();

    info!("[7] Creating file index stream for {} files...", keys.len());

    // Create FileIndexStream
    let source_timestamp_field = journal::FieldName::new_unchecked("_SOURCE_REALTIME_TIMESTAMP");
    let bucket_duration = 15; // 15 seconds
    let time_budget = Duration::from_secs(30);

    let stream = FileIndexStream::new(
        indexing_service,
        file_index_cache,
        registry,
        keys,
        source_timestamp_field,
        bucket_duration,
        time_budget,
    );

    // Collect all indexes
    info!("[8] Indexing files (this may take a moment)...");
    let mut indexed_files = stream
        .collect_indexes()
        .await
        .map_err(|e| format!("Failed to collect indexes: {}", e))?;

    info!("[9] Successfully indexed {} files", indexed_files.len());

    if indexed_files.is_empty() {
        info!("\nNo files were successfully indexed.");
        return Ok(());
    }

    // Sort files by time range based on direction
    info!("[10] Sorting files by time range ({})...", args.direction);
    match args.direction.as_str() {
        "forward" => {
            // Sort by start time ascending (earliest first)
            indexed_files.sort_by_key(|fi| fi.histogram().time_range().0);
        }
        "backward" => {
            // Sort by end time descending (latest first)
            indexed_files.sort_by_key(|fi| std::cmp::Reverse(fi.histogram().time_range().1));
        }
        _ => {
            return Err(format!("Invalid direction: {}", args.direction).into());
        }
    }

    // Print results
    println!("\n{:=<80}", "");
    println!("{:<60} {:<20} {:<20}", "File", "Start Time", "End Time");
    println!("{:=<80}", "");

    for index in indexed_files.iter() {
        let (start, end) = index.histogram().time_range();
        let start_dt = Local.timestamp_opt(start as i64, 0).unwrap();
        let end_dt = Local.timestamp_opt(end as i64, 0).unwrap();

        println!(
            "{:<60} {:<20} {:<20}",
            index.file.path(),
            start_dt.format("%Y-%m-%d %H:%M:%S"),
            end_dt.format("%Y-%m-%d %H:%M:%S")
        );
    }

    println!("{:=<80}", "");

    process(indexed_files, since_secs as u64);

    Ok(())
}

// Test WIP implementation, will be extended once we verify correctness.
// Forward direction only for the time being
// Returns a vector containing `(File, timestamp, entry-offset)` tuple items.
// sorted by timestamp
fn process(file_indexes: Vec<FileIndex>, anchor: u64) -> Vec<(File, u64, u64)> {
    // We will parameterize the function later
    let source_timestamp_field = Some(journal::FieldName::new_unchecked(
        "_SOURCE_REALTIME_TIMESTAMP",
    ));
    let filter = Some(Filter::match_field_value_pair(
        FieldValuePair::parse("PRIORITY=3").unwrap(),
    ));
    let limit = 10;

    // Handle edge cases
    if limit == 0 || file_indexes.is_empty() {
        return Vec::new();
    }

    // Filter to FileIndex instances that could contain relevant entries
    // (i.e., those whose end timestamp is at or after the anchor)
    let mut relevant_indices: Vec<&FileIndex> = file_indexes
        .iter()
        .filter(|fi| fi.histogram.end_time() as u64 >= anchor)
        .collect();

    if relevant_indices.is_empty() {
        return Vec::new();
    }

    // Sort by start timestamp to process files in temporal order
    relevant_indices.sort_by_key(|fi| fi.histogram.start_time());

    // Initialize result vector with capacity for efficiency
    let mut result: Vec<(File, u64, u64)> = Vec::with_capacity(limit);

    for file_index in relevant_indices {
        // Pruning optimization: if we have a full result set and this FileIndex
        // starts after the latest timestamp in our results, we can skip all
        // remaining files since they cannot contribute earlier entries
        if result.len() >= limit {
            let max_timestamp = result.last().unwrap().1;

            if file_index.histogram.start_time() as u64 > max_timestamp {
                break;
            }
        }

        // Perform I/O to retrieve entries from this FileIndex
        let file = file_index.file();
        let window_size = 8 * 1024 * 1024;
        let journal_file =
            journal::JournalFile::<journal::file::Mmap>::open(file, window_size).unwrap();
        let new_entries = file_index
            .retrieve_sorted_entries(
                &journal_file,
                source_timestamp_field.as_ref(),
                filter.as_ref(),
                anchor,
                journal::index::Direction::Forward,
                limit,
            )
            .unwrap();
        if new_entries.is_empty() {
            continue;
        }

        // Merge the new entries with our existing results, maintaining
        // sorted order and respecting the limit constraint
        result = merge_sorted_limited(result, new_entries, limit);
    }

    result
}

/// Merges two sorted vectors into a single sorted vector with at most `limit` elements.
///
/// This function performs a two-pointer merge, which is efficient for combining
/// sorted sequences. It only retains the smallest `limit` entries by timestamp.
///
/// Each vector contains items that are `(File, timestamp, entry_offset)` tuples,
/// sorted by timestamp
///
/// # Arguments
/// * `a` - First sorted vector
/// * `b` - Second sorted vector
/// * `limit` - Maximum number of elements in the result
///
/// # Returns
/// A new vector containing the merged and limited results
fn merge_sorted_limited(
    a: Vec<(File, u64, u64)>,
    b: Vec<(File, u64, u64)>,
    limit: usize,
) -> Vec<(File, u64, u64)> {
    // Handle simple cases
    if a.is_empty() {
        return b.into_iter().take(limit).collect();
    }
    if b.is_empty() {
        return a.into_iter().take(limit).collect();
    }

    // Allocate result vector with appropriate capacity
    let mut result = Vec::with_capacity(limit);
    let mut i = 0;
    let mut j = 0;

    // Two-pointer merge: always take the smaller element
    while result.len() < limit {
        let take_from_a = match (i < a.len(), j < b.len()) {
            (true, false) => true,
            (false, true) => false,
            (false, false) => break,
            (true, true) => a[i].1 <= b[j].1,
        };

        if take_from_a {
            result.push(a[i].clone());
            i += 1;
        } else {
            result.push(b[j].clone());
            j += 1;
        }
    }
    result
}
