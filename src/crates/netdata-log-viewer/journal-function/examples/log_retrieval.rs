use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
use clap::Parser;
use journal_function::logs::{LogQuery, entry_data_to_table, systemd_transformations};
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

    /// End time (e.g., "2025-11-10 12:51:00"). If not specified, will retrieve up to the limit.
    #[arg(long)]
    until: Option<String>,

    /// Maximum number of log entries to retrieve
    #[arg(long, default_value = "20")]
    limit: usize,

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
    let until_secs = if let Some(until_str) = &args.until {
        parse_datetime(until_str)? as u32
    } else {
        u32::MAX
    };

    if let Some(until_str) = &args.until {
        info!(
            "Time range: {} - {} ({} - {} seconds since epoch)",
            args.since, until_str, since_secs, until_secs
        );
    } else {
        info!(
            "Time range: {} - end ({} seconds since epoch, limit: {})",
            args.since, since_secs, args.limit
        );
    }

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
    info!("\n{:=<80}", "");
    info!("{:<60} {:<20} {:<20}", "File", "Start Time", "End Time");
    info!("{:=<80}", "");

    for index in indexed_files.iter() {
        let (start, end) = index.histogram().time_range();
        let start_dt = Local.timestamp_opt(start as i64, 0).unwrap();
        let end_dt = Local.timestamp_opt(end as i64, 0).unwrap();

        info!(
            "{:<60} {:<20} {:<20}",
            index.file.path(),
            start_dt.format("%Y-%m-%d %H:%M:%S"),
            end_dt.format("%Y-%m-%d %H:%M:%S")
        );
    }

    info!("{:=<80}", "");

    // Convert seconds to microseconds for entry timestamp comparison
    // (log entries use microseconds, but histogram/filtering uses seconds)
    let anchor_usec = (since_secs as u64) * 1_000_000;

    info!("\n[11] Querying log entries...");

    // Query log entries using the builder
    let log_entries = LogQuery::new(&indexed_files)
        .with_anchor_usec(anchor_usec)
        .with_limit(args.limit)
        .execute()?;

    info!(
        "[12] Converting {} log entries to table...",
        log_entries.len()
    );

    // Define the columns we want to extract (timestamp is always the first column)
    let columns = vec![
        "PRIORITY".to_string(),
        "MESSAGE".to_string(),
        "_HOSTNAME".to_string(),
        "SYSLOG_IDENTIFIER".to_string(),
        "_UID".to_string(),
        "_GID".to_string(),
    ];

    // Create transformation registry with systemd journal transformations
    let transformations = systemd_transformations();

    // Convert log entries to formatted table
    let table = entry_data_to_table(&log_entries, columns, &transformations)?;

    info!(
        "[13] Successfully created table with {} rows and {} columns",
        table.row_count(),
        table.column_count()
    );

    // Print the table using Display implementation
    info!("\n{}", table);

    Ok(())
}
