use chrono::{Local, TimeZone};
use clap::Parser;
use journal::file::JournalFileMap;
use journal::index::{Bitmap, FileIndex, FileIndexer};
use std::io;

mod ratatui_viz;

#[derive(Parser, Debug)]
#[command(
    name = "histogram-viz",
    about = "Visualize histograms from journal file indexes using interactive charts",
    version
)]
struct Args {
    /// Path to the journal file
    #[arg(value_name = "FILE")]
    file: String,

    /// Field names to index (comma-separated)
    #[arg(short = 'f', long = "fields", value_delimiter = ',')]
    fields: Vec<String>,

    /// Bucket size in seconds
    #[arg(short = 'b', long = "bucket-size", default_value = "1")]
    bucket_size: u64,

    /// Show histograms for specific field values (comma-separated)
    #[arg(short = 'v', long = "values", value_delimiter = ',')]
    values: Vec<String>,
}

fn format_timestamp(micros: u64) -> String {
    let secs = (micros / 1_000_000) as i64;
    let nanos = ((micros % 1_000_000) * 1000) as u32;

    if let Some(dt) = Local.timestamp_opt(secs, nanos).single() {
        dt.format("%Y-%m-%d %H:%M:%S").to_string()
    } else {
        "Invalid timestamp".to_string()
    }
}

fn format_duration(seconds: u64) -> String {
    let hours = seconds / 3600;
    let minutes = (seconds % 3600) / 60;
    let secs = seconds % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, secs)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, secs)
    } else {
        format!("{}s", secs)
    }
}

fn print_histogram_stats(
    file_index: &FileIndex,
    histogram_data: &[(u64, u32)],
    title: &str,
    total_entries: Option<usize>,
) {
    println!("\n{}", "=".repeat(80));
    println!("{}", title);
    println!("{}", "=".repeat(80));

    // Print histogram metadata
    if let Some((start, end)) = file_index.file_histogram.time_range() {
        println!(
            "Time range: {} to {}",
            format_timestamp(start),
            format_timestamp(end)
        );
    }

    if let Some(duration) = file_index.file_histogram.duration_seconds() {
        println!("Duration: {}", format_duration(duration));
    }

    println!("Bucket size: {}s", file_index.file_histogram.bucket_size());
    println!("Total buckets: {}", histogram_data.len());

    if let Some(total) = total_entries {
        println!("Total entries: {}", total);
    } else {
        println!(
            "Total entries: {}",
            file_index.file_histogram.total_entries()
        );
    }

    // Find max count for scaling
    let max_count = histogram_data
        .iter()
        .map(|(_, count)| *count)
        .max()
        .unwrap_or(0);
    println!("Max count per bucket: {}", max_count);

    // Print some statistics
    let top_n = 10.min(histogram_data.len());
    println!("\nTop {} buckets by count:", top_n);
    let mut sorted_data = histogram_data.to_vec();
    sorted_data.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

    for (i, (bucket_seconds, count)) in sorted_data.iter().take(top_n).enumerate() {
        let timestamp_micros = bucket_seconds * 1_000_000;
        println!(
            "  {}. {} - {} entries",
            i + 1,
            format_timestamp(timestamp_micros),
            count
        );
    }
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    // Open the journal file
    let window_size = 8 * 1024 * 1024;
    let journal_file = match JournalFileMap::open(&args.file, window_size) {
        Ok(jf) => jf,
        Err(e) => {
            eprintln!("Failed to open {}: {}", args.file, e);
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }
    };

    println!("Opened journal file: {}", args.file);
    println!("Building file index...");

    // Prepare field names for indexing
    let field_names: Vec<&[u8]> = args.fields.iter().map(|s| s.as_bytes()).collect();

    // Build the file index
    let mut file_indexer = FileIndexer::default();
    let file_index = match file_indexer.index(&journal_file, None, &field_names, args.bucket_size) {
        Ok(idx) => idx,
        Err(e) => {
            eprintln!("Failed to build file index: {}", e);
            return Err(io::Error::new(io::ErrorKind::Other, e));
        }
    };

    // println!("Index built successfully!");
    // println!("Index size: {} bytes", file_index.memory_size());
    // println!("Indexed fields: {}", file_index.entries_index.keys().len());

    // Show overall histogram
    let total_entries = file_index.file_histogram.total_entries();
    let all_entries_bitmap = Bitmap::from_sorted_iter(0..total_entries as u32).unwrap();
    let histogram_data = file_index.file_histogram.from_bitmap(&all_entries_bitmap);

    // print_histogram_stats(&file_index, &histogram_data, "Overall Histogram", None);

    println!("\nStarting interactive visualization...");
    ratatui_viz::visualize_histogram_interactive(
        &journal_file,
        &file_index,
        &histogram_data,
        "Overall Histogram".to_string(),
    )?;

    // Show histograms for specific field values if requested
    if !args.values.is_empty() {
        for value_spec in &args.values {
            if let Some(bitmap) = file_index.entries_index.get(value_spec) {
                let histogram_data = file_index.file_histogram.from_bitmap(bitmap);
                let title = format!("Histogram for: {}", value_spec);

                print_histogram_stats(
                    &file_index,
                    &histogram_data,
                    &title,
                    Some(bitmap.len() as usize),
                );

                println!("\nStarting interactive visualization...");
                ratatui_viz::visualize_histogram_interactive(
                    &journal_file,
                    &file_index,
                    &histogram_data,
                    title,
                )?;
            } else {
                println!("Field value '{}' not found in index", value_spec);
            }
        }
    } else {
        // If no specific values requested, show available field values
        println!("\n{}", "=".repeat(80));
        println!("Available field values (top 20 by occurrence):");
        println!("{}", "=".repeat(80));

        let mut field_counts: Vec<(&String, usize)> = file_index
            .entries_index
            .iter()
            .map(|(k, v)| (k, v.len() as usize))
            .collect();
        field_counts.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

        for (field, count) in field_counts.iter().take(20) {
            println!("  {} ({} entries)", field, count);
        }

        println!("\nUse -v/--values to visualize specific field values (e.g., -v 'PRIORITY=6')");
    }

    Ok(())
}
