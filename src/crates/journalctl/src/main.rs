use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use journal_core::file::{JournalFile, Mmap};
use journal_registry::File as RepositoryFile;
use serde_json::{Value as JsonValue, json};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, ValueEnum)]
enum OutputFormat {
    Json,
    Line,
    JsonLine,
}

#[derive(Parser, Debug)]
#[command(name = "journalctl")]
#[command(about = "A tool to work with systemd journal files", long_about = None)]
struct Args {
    /// List all fields present in the journal file
    #[arg(long)]
    fields: bool,

    /// Output format for log entries
    #[arg(long, value_enum)]
    format: Option<OutputFormat>,

    /// Path to the journal file
    #[arg(value_name = "FILE")]
    file: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();

    if args.fields {
        list_fields(&args.file)?;
    } else if let Some(format) = args.format {
        print_entries(&args.file, format)?;
    }

    Ok(())
}

fn print_entries(file_path: &PathBuf, format: OutputFormat) -> Result<()> {
    // Open the journal file with a reasonable window size
    let window_size = 8 * 1024 * 1024; // 8 MiB

    // Convert PathBuf to repository::File
    let repo_file = RepositoryFile::from_path(file_path)
        .with_context(|| format!("Failed to parse journal file path: {}", file_path.display()))?;

    let journal_file = JournalFile::<Mmap>::open(&repo_file, window_size)
        .with_context(|| format!("Failed to open journal file: {}", file_path.display()))?;

    // Get the entry list
    let Some(entry_list) = journal_file.entry_list() else {
        return Ok(()); // No entries in the journal
    };

    // Collect all entry offsets
    let mut entry_offsets = Vec::new();
    entry_list
        .collect_offsets(&journal_file, &mut entry_offsets)
        .with_context(|| "Failed to collect entry offsets")?;

    // Process each entry
    for entry_offset in entry_offsets {
        // Parse the entry into a map of key-value pairs
        let entry_map = parse_entry(&journal_file, entry_offset)
            .with_context(|| format!("Failed to parse entry at offset {}", entry_offset))?;

        // Print according to the requested format
        match format {
            OutputFormat::Json => print_entry_json(&entry_map)?,
            OutputFormat::Line => print_entry_line(&entry_map),
            OutputFormat::JsonLine => print_entry_json_line(&entry_map)?,
        }
    }

    Ok(())
}

fn parse_entry(
    journal_file: &JournalFile<Mmap>,
    entry_offset: std::num::NonZeroU64,
) -> Result<BTreeMap<String, String>> {
    let mut entry_map = BTreeMap::new();

    // Iterate through all data objects for this entry
    for data_result in journal_file.entry_data_objects(entry_offset)? {
        let data = data_result.with_context(|| "Failed to read data object")?;

        // Get the raw KEY=VALUE bytes
        let payload = data.payload_bytes();

        // Parse KEY=VALUE
        if let Some(equals_pos) = payload.iter().position(|&b| b == b'=') {
            let key = &payload[..equals_pos];
            let value = &payload[equals_pos + 1..];

            let key_str = std::str::from_utf8(key).unwrap_or("<invalid UTF-8>");
            let value_str = std::str::from_utf8(value).unwrap_or("<invalid UTF-8>");

            entry_map.insert(key_str.to_string(), value_str.to_string());
        }
    }

    Ok(entry_map)
}

fn print_entry_json(entry: &BTreeMap<String, String>) -> Result<()> {
    let json_obj: JsonValue = entry.iter().map(|(k, v)| (k.clone(), json!(v))).collect();

    let pretty_json =
        serde_json::to_string_pretty(&json_obj).with_context(|| "Failed to serialize to JSON")?;

    println!("{}", pretty_json);

    Ok(())
}

fn print_entry_line(entry: &BTreeMap<String, String>) {
    let line: Vec<String> = entry.iter().map(|(k, v)| format!("{}={}", k, v)).collect();

    println!("{}", line.join(" "));
}

fn print_entry_json_line(entry: &BTreeMap<String, String>) -> Result<()> {
    let json_obj: JsonValue = entry.iter().map(|(k, v)| (k.clone(), json!(v))).collect();

    let compact_json =
        serde_json::to_string(&json_obj).with_context(|| "Failed to serialize to JSON")?;

    println!("{}", compact_json);

    Ok(())
}

fn list_fields(file_path: &PathBuf) -> Result<()> {
    // Open the journal file with a reasonable window size
    let window_size = 8 * 1024 * 1024; // 8 MiB

    // Convert PathBuf to repository::File
    let repo_file = RepositoryFile::from_path(file_path)
        .with_context(|| format!("Failed to parse journal file path: {}", file_path.display()))?;

    let journal_file = JournalFile::<Mmap>::open(&repo_file, window_size)
        .with_context(|| format!("Failed to open journal file: {}", file_path.display()))?;

    // Iterate through all fields and collect them with their cardinality
    let mut field_info = Vec::new();

    for field_result in journal_file.fields() {
        let field_name = {
            let field = field_result.with_context(|| "Failed to read field from journal file")?;

            // The payload contains the field name as bytes
            std::str::from_utf8(field.payload)
                .unwrap_or("<invalid UTF-8>")
                .to_string()
        };

        // Count the number of data objects for this field (cardinality)
        // We need to manually iterate and explicitly drop guards to avoid issues
        let mut cardinality = 0;
        let mut iter = journal_file.field_data_objects(field_name.as_bytes())?;

        while let Some(data_result) = iter.next() {
            match data_result {
                Ok(_) => {
                    // Explicitly drop the guard before getting the next one
                    cardinality += 1;
                }
                Err(e) => {
                    eprintln!(
                        "Warning: Failed to read data object for field {}: {}",
                        field_name, e
                    );
                    break;
                }
            }
        }

        field_info.push((field_name, cardinality));
    }

    // Sort by field name for consistent output
    field_info.sort_by(|a, b| a.0.cmp(&b.0));

    // Find the maximum field name length for formatting
    let max_name_len = field_info
        .iter()
        .map(|(name, _)| name.len())
        .max()
        .unwrap_or(0);

    // Print each field with its cardinality
    for (field_name, cardinality) in field_info {
        println!(
            "{:<width$}  {}",
            field_name,
            cardinality,
            width = max_name_len
        );
    }

    Ok(())
}
