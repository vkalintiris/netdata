#![allow(dead_code)]

use journal_file::Direction;
use journal_file::JournalFile;
use journal_file::JournalReader;
use journal_file::Location;
use journal_file::Mmap;
use journal_registry::{JournalRegistry, SortBy, SortOrder, SourceType};
use std::time::Instant;
use std::time::{Duration, SystemTime};
use tracing::{info, warn};

fn count_data_objects(jf: &JournalFile<Mmap>) -> Result<usize, Box<dyn std::error::Error>> {
    let Some(data_hash_table) = jf.data_hash_table_ref() else {
        return Ok(0);
    };

    let mut count = 0;

    // Iterate through each bucket in the hash table
    for bucket in data_hash_table.items.iter() {
        let mut current_offset = bucket.head_hash_offset;

        // Follow the chain of data objects in this bucket
        while let Some(offset) = current_offset {
            count += 1;

            // Get the next object in the chain
            let data_guard = jf.data_ref(offset)?;
            current_offset = data_guard.header.next_hash_offset;
        }
    }

    Ok(count)
}

fn count_field_objects(jf: &JournalFile<Mmap>) -> Result<usize, Box<dyn std::error::Error>> {
    let Some(field_hash_table) = jf.field_hash_table_ref() else {
        return Ok(0);
    };

    let mut count = 0;

    // Iterate through each bucket in the hash table
    for bucket in field_hash_table.items.iter() {
        let mut current_offset = bucket.head_hash_offset;

        // Follow the chain of field objects in this bucket
        while let Some(offset) = current_offset {
            count += 1;

            // Get the next object in the chain
            let field_guard = jf.field_ref(offset)?;
            current_offset = field_guard.header.next_hash_offset;
        }
    }

    Ok(count)
}

use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;

/// Represents a single bucket in the histogram
#[derive(Debug, Clone, Default)]
pub struct Bucket {
    /// key = data object offset
    /// value = vector of entry object offsets that contain this data object offset
    pub items: HashMap<NonZeroU64, Vec<NonZeroU64>>,
}

/// Built up from each log entry in the journal file
#[derive(Debug, Clone)]
pub struct Histogram {
    pub buckets: Vec<Bucket>,
}

impl Histogram {
    fn calculate_bucket_index(
        entry_time: SystemTime,
        start_time: SystemTime,
        num_buckets: usize,
    ) -> Option<usize> {
        let elapsed = entry_time.duration_since(start_time).ok()?;

        // Each bucket represents 60 seconds
        let bucket_index = elapsed.as_secs() as usize / 60;

        // println!(
        //     "entry time: {:#?}, start_time: {:#?}, bucket_index: {:#?}",
        //     entry_time, start_time, bucket_index
        // );

        // Ensure we don't exceed the number of buckets
        if bucket_index < num_buckets {
            Some(bucket_index)
        } else {
            None
        }
    }

    /// Create a histogram with the specified number of buckets for the given fields
    pub fn create(jf: &JournalFile<Mmap>, field_names: &[&str]) -> Self {
        let duration = jf.duration().unwrap_or(Duration::from_secs(60));
        println!(
            "Duration of journal file: {:#?} minutes",
            duration.as_secs() / 60
        );
        let num_buckets = (duration.as_secs() as usize + 60) / 60;

        let mut histogram = Self {
            buckets: vec![Bucket::default(); num_buckets],
        };

        let mut field_offsets = HashSet::with_capacity(field_names.len());
        for f in jf.fields() {
            let f = f.unwrap();
            f.offset();
            field_offsets.insert(f.offset());
        }

        let start_time = jf.head_entry_time().unwrap();
        let mut data_offsets = Vec::with_capacity(64);

        let mut reader = JournalReader::default();
        while reader.step(jf, Direction::Forward).unwrap() {
            let entry_offset = reader.get_entry_offset().unwrap();
            let entry_time = std::time::UNIX_EPOCH
                + std::time::Duration::from_micros(reader.get_realtime_usec(jf).unwrap());

            data_offsets.clear();
            reader.entry_data_offsets(jf, &mut data_offsets).unwrap();

            let bucket_index =
                Histogram::calculate_bucket_index(entry_time, start_time, num_buckets).unwrap();

            let bucket = &mut histogram.buckets[bucket_index];

            // For each data offset in this entry, add the entry offset to its vector
            for &data_offset in &data_offsets {
                let data_object = bucket
                    .items
                    .entry(data_offset)
                    .or_insert_with(Vec::new)
                    .push(entry_offset);
            }
        }

        histogram
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    for dir in ["/var/log/journal", "/run/log/journal"] {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut files = registry.query().execute();
    files.sort_by_key(|x| x.path.clone());
    files.sort_by_key(|x| x.size);

    let mut total_entries = 0;
    let mut my_total = 0;

    let mut offsets = Vec::new();
    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        offsets.clear();

        println!("Processing file: {:#?}", file);
        let window_size = 8 * 1024 * 1024;
        let jf = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        let jh = jf.journal_header_ref();
        total_entries += jh.n_entries;

        println!("header entries: {:#?}", jh.n_entries);

        if let Some(e) = jf.entry_list() {
            e.collect_offsets(&jf, &mut offsets).unwrap();

            my_total += offsets.len();

            println!("collected entries: {:#?}", offsets.len());
        }

        // let start = Instant::now();
        // let histogram = Histogram::create(&jf, &["PRIORITY"]);
        // let duration = start.elapsed();
        // info!(
        //     "histogram built in {:#?} seconds with {:#?} buckets",
        //     duration.as_secs_f32(),
        //     histogram.buckets.len()
        // );

        // tokio::time::sleep(Duration::from_secs(3600)).await;
    }

    println!("Total entries: {:#?}", total_entries);
    println!("My total entries: {:#?}", my_total);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    if false {
        // Display initial statistics
        println!("\n=== Journal Files Statistics ===");
        println!("Total files: {}", registry.query().count());
        println!(
            "Total size: {:.2} MB",
            registry.query().total_size() as f64 / (1024.0 * 1024.0)
        );

        // Get system journal files sorted by size
        println!("\n=== System Journal Files (sorted by size) ===");
        let system_files = registry
            .query()
            .source(SourceType::System)
            .sort_by(SortBy::Size(SortOrder::Descending))
            .execute();

        println!("Found {} system journal files:", system_files.len());
        for (idx, file) in system_files.iter().take(5).enumerate() {
            println!(
                "  [{}] {} ({:.2} MB) - modified: {:?}",
                idx,
                file.path.display(),
                file.size as f64 / (1024.0 * 1024.0),
                file.modified
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|_| {
                        format!(
                            "{} hours ago",
                            (SystemTime::now()
                                .duration_since(file.modified)
                                .unwrap_or_default()
                                .as_secs()
                                / 3600)
                        )
                    })
                    .unwrap_or_else(|_| "unknown".to_string())
            );
        }

        // Recent large files (modified in last 24 hours, > 1MB)
        println!("\n=== Recent Large Files (last 24h, >1MB) ===");
        let recent_large = registry
            .query()
            .modified_after(SystemTime::now() - Duration::from_secs(24 * 60 * 60))
            .min_size(1024 * 1024) // 1MB
            .sort_by(SortBy::Modified(SortOrder::Descending))
            .limit(10)
            .execute();

        if recent_large.is_empty() {
            println!("No large files modified in the last 24 hours");
        } else {
            println!(
                "Found {} large files modified recently:",
                recent_large.len()
            );
            for file in &recent_large {
                println!(
                    "  {} ({:.2} MB) - {}",
                    file.path.file_name().unwrap_or_default().to_string_lossy(),
                    file.size as f64 / (1024.0 * 1024.0),
                    file.source_type
                );
            }
        }

        // Group files by source type
        println!("\n=== Files by Source Type ===");
        for source_type in &[
            SourceType::System,
            SourceType::User,
            SourceType::Remote,
            SourceType::Namespace,
            SourceType::Other,
        ] {
            let files = registry.query().source(*source_type).execute();
            let total_size = registry.query().source(*source_type).total_size();

            if !files.is_empty() {
                println!(
                    "  {:10} - {} files, {:.2} MB total",
                    source_type.to_string(),
                    files.len(),
                    total_size as f64 / (1024.0 * 1024.0)
                );
            }
        }

        // Find files by machine ID (if any exist)
        println!("\n=== Files by Machine ID ===");
        let all_files = registry.query().execute();
        let machine_ids: std::collections::HashSet<_> = all_files
            .iter()
            .filter_map(|f| f.machine_id.as_ref())
            .cloned()
            .collect();

        if machine_ids.is_empty() {
            println!("No files with machine IDs found");
        } else {
            for (idx, machine_id) in machine_ids.iter().take(3).enumerate() {
                let machine_files = registry
                    .query()
                    .machine(machine_id)
                    .sort_by(SortBy::Sequence(SortOrder::Ascending))
                    .execute();

                println!(
                    "  Machine {} ({}...): {} files",
                    idx + 1,
                    &machine_id[..8.min(machine_id.len())],
                    machine_files.len()
                );
            }
        }
    }

    Ok(())
}
