use journal_file::index::FileIndexer;
use journal_file::{JournalFile, JournalFileOptions, JournalWriter, Mmap};
use serde::Deserialize;
use std::path::Path;
use tracing::info;
use rand::seq::SliceRandom;
use rand::thread_rng;

#[derive(Deserialize, Debug, Clone)]
struct JournalEntry {
    #[serde(rename = "_SOURCE_REALTIME_TIMESTAMP")]
    source_realtime_timestamp: u64,
    #[serde(rename = "PRIORITY")]
    priority: u32,
    #[serde(rename = "TEST_FIELD")]
    test_field: String,
    #[serde(rename = "MESSAGE")]
    message: String,
}

fn generate_uuid(seed: u8) -> [u8; 16] {
    // Generate different UUIDs for different purposes
    [seed; 16]
}

fn entry_data_to_string(data: &Vec<Vec<u8>>) -> String {
    data.iter()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
        .collect::<Vec<_>>()
        .join(", ")
}

fn load_journal_entries(json_path: &str) -> Result<Vec<JournalEntry>, Box<dyn std::error::Error>> {
    let json_content = std::fs::read_to_string(json_path)?;
    let journal_entries: Vec<JournalEntry> = serde_json::from_str(&json_content)?;
    info!(
        "Loaded {} entries from {}",
        journal_entries.len(),
        json_path
    );
    Ok(journal_entries)
}

fn create_journal_file_with_entries(
    journal_path: &Path,
    journal_entries: &[JournalEntry],
) -> Result<(), Box<dyn std::error::Error>> {
    if journal_path.exists() {
        std::fs::remove_file(journal_path)?;
    }

    let options = JournalFileOptions::new(
        generate_uuid(1),
        generate_uuid(2),
        generate_uuid(3),
        generate_uuid(4),
    );
    let mut journal_file = JournalFile::create(journal_path, options)?;
    let boot_id = [1; 16];
    let mut writer = JournalWriter::new(&mut journal_file)?;

    // Shuffle the entries randomly before inserting
    let mut shuffled_entries = journal_entries.to_vec();
    shuffled_entries.shuffle(&mut thread_rng());
    info!("Shuffled {} entries randomly", shuffled_entries.len());

    let mut entries = Vec::new();
    for entry in &shuffled_entries {
        let entry_data = vec![
            format!(
                "_SOURCE_REALTIME_TIMESTAMP={}",
                entry.source_realtime_timestamp
            )
            .into_bytes(),
            format!("PRIORITY={}", entry.priority).into_bytes(),
            format!("TEST_FIELD={}", entry.test_field).into_bytes(),
            format!("MESSAGE={}", entry.message).into_bytes(),
        ];

        let entry_refs: Vec<&[u8]> = entry_data.iter().map(|v| v.as_slice()).collect();

        writer.add_entry(
            &mut journal_file,
            &entry_refs,
            entry.source_realtime_timestamp,
            entry.source_realtime_timestamp,
            boot_id,
        )?;

        std::thread::sleep(std::time::Duration::from_millis(100));
        entries.push(entry_data_to_string(&entry_data));
    }

    entries.sort();
    for (idx, entry) in entries.iter().enumerate() {
        info!("[{idx}] {}", &entry);
    }

    Ok(())
}

fn demonstrate_file_index(journal_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let journal_file_read = JournalFile::<Mmap>::open(journal_path, 8 * 1024 * 1024)?;
    let mut file_indexer = FileIndexer::default();

    let field_names = vec![
        b"MESSAGE".as_slice(),
        b"PRIORITY".as_slice(),
        b"TEST_FIELD".as_slice(),
    ];

    let file_index = file_indexer.index(&journal_file_read, b"TEST_FIELD", &field_names)?;

    // Print some basic info about the file index
    let histogram = &file_index.file_histogram;
    info!("Histogram buckets: {}", histogram.len());
    info!("Total entries in histogram: {}", histogram.total_entries());

    if let Some((start_time, end_time)) = histogram.time_range() {
        info!("Time range: {} to {} microseconds", start_time, end_time);
        info!("Duration: {} seconds", (end_time - start_time) / 1_000_000);
    }

    println!("{:#?}", file_index);

    let key = "TEST_FIELD=value_1";
    let v = file_index
        .file_histogram
        .from_bitmap(file_index.entries_index.get(key).unwrap());
    println!("Sparse histogram for {}, {:#?}", key, v);

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let journal_path = Path::new("/tmp/foo.journal");
    let json_path = "/home/vk/repos/nd/sjr/src/crates/journal/journal_registry/examples/index.json";

    // Load journal entries from JSON file
    let journal_entries = load_journal_entries(json_path)?;

    // Create journal file with the loaded entries
    create_journal_file_with_entries(journal_path, &journal_entries)?;

    // Demonstrate file indexing operations
    demonstrate_file_index(journal_path)?;

    Ok(())
}
