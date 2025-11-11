use clap::{Parser, ValueEnum};
use journal::{
    JournalFile,
    file::Mmap,
    index::{Direction, FieldName, FieldValuePair, FileIndexer, Filter},
};
use std::path::PathBuf;
use tracing::info;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DirectionArg {
    Forward,
    Backward,
}

impl From<DirectionArg> for Direction {
    fn from(arg: DirectionArg) -> Self {
        match arg {
            DirectionArg::Forward => Direction::Forward,
            DirectionArg::Backward => Direction::Backward,
        }
    }
}

/// Retrieve and print sorted journal entries with optional filtering
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the journal file
    #[arg(short, long)]
    file: PathBuf,

    /// Starting timestamp in microseconds
    #[arg(short, long)]
    timestamp: u64,

    /// Direction to iterate (forward or backward)
    #[arg(short, long, value_enum)]
    direction: DirectionArg,

    /// Maximum number of entries to retrieve
    #[arg(short, long)]
    limit: usize,

    /// Optional filter expression (e.g., "PRIORITY=3" or "PRIORITY=error")
    #[arg(short = 'F', long)]
    filter: Option<String>,

    /// Bucket duration for histogram (in seconds)
    #[arg(short, long, default_value = "60")]
    bucket_duration: u32,

    /// Fields to index (comma-separated, e.g., "PRIORITY,SYSLOG_IDENTIFIER")
    #[arg(
        short,
        long,
        default_value = "PRIORITY,SYSLOG_IDENTIFIER,_SYSTEMD_UNIT"
    )]
    indexed_fields: String,

    /// Source timestamp field to use for sorting
    #[arg(short, long, default_value = "_SOURCE_REALTIME_TIMESTAMP")]
    source_field: String,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let args = Args::parse();

    info!("Opening journal file: {}", args.file.display());
    let file = journal::repository::File::from_str(args.file.to_str().unwrap()).unwrap();
    let journal_file = JournalFile::<Mmap>::open(&file, 8 * 1024 * 1024)?;

    // Parse indexed fields
    let indexed_fields: Vec<FieldName> = args
        .indexed_fields
        .split(',')
        .filter_map(|s| FieldName::new(s.trim().to_string()))
        .collect();

    info!("Indexed fields: {:?}", indexed_fields);

    // Create source field name
    let source_field = FieldName::new(args.source_field).ok_or("Invalid source field name")?;

    // Build the file index
    info!("Building file index...");
    let mut indexer = FileIndexer::default();
    let file_index = indexer.index(
        &journal_file,
        Some(&source_field),
        &indexed_fields,
        args.bucket_duration,
    )?;

    info!("File index built successfully!");
    info!("Total entries in index: {}", file_index.entry_offsets.len());
    info!("Indexed field-value pairs: {}", file_index.bitmaps().len());

    // Parse filter if provided
    let filter = if let Some(filter_str) = args.filter {
        info!("Parsing filter: {}", filter_str);

        // Try to parse as field=value pair
        if let Some(pair) = FieldValuePair::parse(&filter_str) {
            Some(Filter::match_field_value_pair(pair))
        } else if let Some(field_name) = FieldName::new(filter_str.clone()) {
            // Try as field name only
            Some(Filter::match_field_name(field_name))
        } else {
            return Err(format!("Invalid filter expression: {}", filter_str).into());
        }
    } else {
        None
    };

    if let Some(ref f) = filter {
        info!("Using filter: {}", f);
    } else {
        info!("No filter specified, retrieving all entries");
    }

    // Retrieve sorted entries
    info!(
        "Retrieving up to {} entries {} timestamp {} in {} direction...",
        args.limit,
        match args.direction {
            DirectionArg::Forward => "after",
            DirectionArg::Backward => "before",
        },
        args.timestamp,
        match args.direction {
            DirectionArg::Forward => "forward",
            DirectionArg::Backward => "backward",
        }
    );

    drop(journal_file);

    let results = file_index.retrieve_sorted_entries(
        &file,
        Some(&source_field),
        filter.as_ref(),
        args.timestamp,
        args.direction.into(),
        args.limit,
    )?;

    info!("Retrieved {} entries", results.len());
    println!(
        "\n{:<20} {:<12} {}",
        "Timestamp (μs)", "Offset", "Human-readable time"
    );
    println!("{}", "-".repeat(70));

    for (_file, timestamp, offset) in results {
        // Convert microseconds to seconds for human-readable format
        let seconds = timestamp / 1_000_000;
        let datetime = chrono::DateTime::from_timestamp(seconds as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| "Invalid time".to_string());

        println!("{:<20} {:<12} {}", timestamp, offset, datetime);
    }

    Ok(())
}
