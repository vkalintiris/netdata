#![allow(unused_imports)]
use anyhow::{Context, Result};
use clap::Parser;
use fst::{MapBuilder, Set, SetBuilder};
use journal::file::{HashableObject, JournalFile, Mmap};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;
use tracing::{debug, info, warn};

/// Build an FST index of all data objects in a journal file
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Path to the journal file to index
    #[arg(short, long)]
    journal_file: PathBuf,

    /// Path to output the FST index
    #[arg(short, long)]
    output: PathBuf,

    /// Index type: "set" (just keys) or "map" (keys with offsets)
    #[arg(short = 't', long, default_value = "map")]
    index_type: IndexType,

    /// Only index specific field names (comma-separated, e.g., "MESSAGE,PRIORITY")
    #[arg(short = 'f', long)]
    field_filter: Option<String>,

    /// Maximum number of data objects to index (for testing)
    #[arg(short = 'l', long)]
    limit: Option<usize>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Debug, Clone, Copy)]
enum IndexType {
    Set,
    Map,
}

impl std::str::FromStr for IndexType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "set" => Ok(IndexType::Set),
            "map" => Ok(IndexType::Map),
            _ => Err(format!("Invalid index type: {}. Use 'set' or 'map'", s)),
        }
    }
}

/// Collects all data object payloads from a journal file
fn collect_data_objects(
    journal_file: &JournalFile<Mmap>,
    field_filter: Option<&[String]>,
    limit: Option<usize>,
) -> Result<HashSet<(Vec<u8>, u64)>> {
    info!("Starting data object collection from journal file");

    // Get the data hash table
    let Some(data_hash_table) = journal_file.data_hash_table_ref() else {
        warn!("No data hash table found in journal file");
        return Ok(HashSet::new());
    };

    // Step 1: Collect all data object offsets from the hash table
    info!("Collecting all data object offsets from hash table");
    let mut offsets = Vec::new();

    for hash_item in data_hash_table.items.iter() {
        // Skip empty buckets
        let Some(data_offset) = hash_item.head_hash_offset else {
            continue;
        };

        offsets.push(data_offset);
    }

    info!(
        "Collected {} offsets, now sorting for sequential access",
        offsets.len()
    );

    // Step 2: Sort offsets to enable linear scan of the file
    offsets.sort_unstable();

    info!("Reading data objects in sequential order");

    // Step 3: Read data objects in sorted order (sequential file access)
    let mut data_objects = HashSet::new();
    let mut count = 0;

    for (idx, offset) in offsets.iter().enumerate() {
        if let Some(max) = limit {
            if count >= max {
                info!("Reached limit of {} objects", max);
                break;
            }
        }

        match journal_file.data_ref(*offset) {
            Ok(data_object) => {
                let payload = data_object.get_payload();
                let hash = data_object.hash();

                if payload.starts_with(b"MESSAGE=") || payload.starts_with(b"AE_FATAL_STACK_TRACE")
                {
                    continue;
                }

                // If field filter is specified, check if this payload matches
                if let Some(filters) = field_filter {
                    let payload_str = String::from_utf8_lossy(payload);
                    let matches = filters.iter().any(|filter| {
                        payload_str.starts_with(filter)
                            || payload_str.starts_with(&format!("{}=", filter))
                    });

                    if !matches {
                        continue;
                    }
                }

                data_objects.insert((payload.to_vec(), hash));
                // data_objects.push((payload.to_vec(), hash));
                count += 1;
            }
            Err(e) => {
                warn!("Failed to read data object at offset {:?}: {:?}", offset, e);
            }
        }
    }

    info!("Collected {} data objects total", data_objects.len());
    Ok(data_objects)
}

/// Builds an FST Set from the collected data objects (just the keys)
fn build_fst_set(data_objects: HashSet<(Vec<u8>, u64)>, output_path: &PathBuf) -> Result<()> {
    info!("Building FST Set index");

    // Sort the data objects by their payload (FST requires sorted keys)
    let mut sorted_keys = Vec::with_capacity(data_objects.len());
    sorted_keys.extend(data_objects.into_iter().map(|(bytes, _)| bytes));
    sorted_keys.sort();

    info!("Building index from {} unique keys", sorted_keys.len());

    // Build the FST Set
    let file = File::create(output_path)
        .context(format!("Failed to create output file: {:?}", output_path))?;
    let writer = BufWriter::new(file);
    let mut builder = SetBuilder::new(writer).context("Failed to create FST SetBuilder")?;

    for (idx, key) in sorted_keys.iter().enumerate() {
        builder
            .insert(key)
            .context(format!("Failed to insert key at index {}", idx))?;

        if idx % 10000 == 0 && idx > 0 {
            debug!("Inserted {} keys into FST", idx);
        }
    }

    builder
        .finish()
        .context("Failed to finish building FST Set")?;

    info!("FST Set index written to {:?}", output_path);
    Ok(())
}

/// Builds an FST Map from the collected data objects (keys with their offsets)
fn build_fst_map(data_objects: HashSet<(Vec<u8>, u64)>, output_path: &PathBuf) -> Result<()> {
    info!("Building FST Map index");

    let mut key_map = Vec::with_capacity(data_objects.len());
    key_map.extend(data_objects);
    key_map.sort_by(|l, r| l.0.cmp(&r.0));

    info!("Building index from {} unique keys", key_map.len());

    // Build the FST Map
    let file = File::create(output_path)
        .context(format!("Failed to create output file: {:?}", output_path))?;
    let writer = BufWriter::new(file);
    let mut builder = MapBuilder::new(writer).context("Failed to create FST MapBuilder")?;

    for (idx, (key, offset)) in key_map.iter().enumerate() {
        builder
            .insert(key, *offset)
            .context(format!("Failed to insert key-value pair at index {}", idx))?;

        if idx % 10000 == 0 && idx > 0 {
            debug!("Inserted {} key-value pairs into FST", idx);
        }
    }

    builder
        .finish()
        .context("Failed to finish building FST Map")?;

    info!("FST Map index written to {:?}", output_path);
    Ok(())
}

/// Prints statistics about the generated FST index
fn print_statistics(output_path: &PathBuf, index_type: IndexType) -> Result<()> {
    let metadata = std::fs::metadata(output_path).context("Failed to read output file metadata")?;

    info!("Index statistics:");
    info!("  Type: {:?}", index_type);
    info!(
        "  Size: {} bytes ({:.2} MB)",
        metadata.len(),
        metadata.len() as f64 / 1_048_576.0
    );

    // Load and verify the index
    match index_type {
        IndexType::Set => {
            let index_data = std::fs::read(output_path).context("Failed to read FST Set file")?;
            let set = Set::new(index_data).context("Failed to load FST Set")?;
            info!("  Keys: {}", set.len());
        }
        IndexType::Map => {
            let index_data = std::fs::read(output_path).context("Failed to read FST Map file")?;
            let map = fst::Map::new(index_data).context("Failed to load FST Map")?;
            info!("  Keys: {}", map.len());
        }
    }

    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let log_level = if args.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(log_level)),
        )
        .init();

    info!("Journal FST Indexer");
    info!("Journal file: {:?}", args.journal_file);
    info!("Output file: {:?}", args.output);
    info!("Index type: {:?}", args.index_type);

    // Parse field filter if provided
    let field_filter: Option<Vec<String>> = args
        .field_filter
        .as_ref()
        .map(|f| f.split(',').map(|s| s.trim().to_string()).collect());

    if let Some(ref filters) = field_filter {
        info!("Field filter: {:?}", filters);
    }

    // Open the journal file
    info!("Opening journal file...");
    let journal_file =
        JournalFile::open(&args.journal_file, 64 * 1024).context("Failed to open journal file")?;

    info!("Journal file header:");
    let header = journal_file.journal_header_ref();
    info!("  Entries: {}", header.n_entries);
    info!("  Objects: {}", header.n_objects);

    // Collect data objects
    let data_objects = collect_data_objects(&journal_file, field_filter.as_deref(), args.limit)?;

    if data_objects.is_empty() {
        warn!("No data objects found matching criteria");
        return Ok(());
    }

    // Build the appropriate FST index
    match args.index_type {
        IndexType::Set => build_fst_set(data_objects, &args.output)?,
        IndexType::Map => build_fst_map(data_objects, &args.output)?,
    }

    // Print statistics
    print_statistics(&args.output, args.index_type)?;

    info!("Indexing complete!");
    Ok(())
}
