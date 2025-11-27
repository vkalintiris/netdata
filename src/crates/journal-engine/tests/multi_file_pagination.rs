//! Integration tests for multi-file pagination with PaginationState.

use journal_common::Seconds;
use journal_core::file::{JournalFile, JournalFileOptions, JournalWriter};
use journal_core::repository::File;
use journal_engine::logs::query::LogQuery;
use journal_index::{Anchor, Direction, FieldName, FileIndexer, Microseconds};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use uuid::Uuid;

/// Test journal entry specification
struct TestEntry {
    timestamp: Microseconds,
    fields: Vec<(String, String)>,
}

impl TestEntry {
    fn new(timestamp: Microseconds) -> Self {
        Self {
            timestamp,
            fields: Vec::new(),
        }
    }

    fn with_field(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.fields.push((name.into(), value.into()));
        self
    }
}

/// Create a test journal file path with a specific name
fn create_test_journal_path(temp_dir: &TempDir, filename: &str) -> PathBuf {
    let machine_id = Uuid::from_u128(0x12345678_1234_1234_1234_123456789abc);
    let machine_dir = temp_dir.path().join(machine_id.to_string());
    fs::create_dir_all(&machine_dir).expect("create machine dir");
    machine_dir.join(filename)
}

/// Helper to create a test journal file with specified entries
fn create_test_journal(
    temp_dir: &TempDir,
    filename: &str,
    entries: Vec<TestEntry>,
) -> Result<File, Box<dyn std::error::Error>> {
    let journal_path = create_test_journal_path(temp_dir, filename);

    let file =
        File::from_path(&journal_path).ok_or("Failed to create repository File from path")?;

    let machine_id = Uuid::from_u128(0x12345678_1234_1234_1234_123456789abc);
    let boot_id = Uuid::from_u128(0x11111111_1111_1111_1111_111111111111);
    let seqnum_id = Uuid::from_u128(0x22222222_2222_2222_2222_222222222222);

    let options = JournalFileOptions::new(machine_id, boot_id, seqnum_id);

    let mut journal_file = JournalFile::create(&file, options)?;
    let mut writer = JournalWriter::new(&mut journal_file, 1, boot_id)?;

    for entry in entries {
        let mut entry_data = Vec::new();

        // Add _SOURCE_REALTIME_TIMESTAMP first
        entry_data.push(format!("_SOURCE_REALTIME_TIMESTAMP={}", entry.timestamp.0).into_bytes());

        // Add all other fields
        for (field, value) in entry.fields {
            entry_data.push(format!("{}={}", field, value).into_bytes());
        }

        let entry_refs: Vec<&[u8]> = entry_data.iter().map(|v| v.as_slice()).collect();

        writer.add_entry(
            &mut journal_file,
            &entry_refs,
            entry.timestamp.0,
            entry.timestamp.0,
        )?;
    }

    Ok(file)
}

#[test]
fn test_multi_file_pagination_forward_non_overlapping() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 microseconds (100 entries)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("FILE", "1")
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=200..300 microseconds (100 entries)
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("FILE", "2")
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let file_field = FieldName::new("FILE").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[file_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[file_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // First page: limit=150, should get all 100 from file1 + 50 from file2
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(150)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        150,
        "First page should contain exactly 150 entries"
    );

    // Verify timestamps are in ascending order
    for i in 1..first_page.len() {
        assert!(
            first_page[i - 1].timestamp <= first_page[i].timestamp,
            "Entries should be in ascending timestamp order"
        );
    }

    // First entry should be at timestamp 100
    assert_eq!(
        first_page.first().unwrap().timestamp,
        100,
        "First entry should be at timestamp 100"
    );

    // Last entry should be at timestamp 249 (100-199 from file1, then 200-249 from file2)
    assert_eq!(
        first_page.last().unwrap().timestamp,
        249,
        "Last entry of first page should be at timestamp 249"
    );

    // State should track positions for files we read from
    assert!(
        !state1.file_positions.is_empty(),
        "State should track positions"
    );

    // Second page: use state to get remaining 50 entries
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(150)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        50,
        "Second page should contain remaining 50 entries"
    );

    // Verify timestamps continue from where first page left off
    assert_eq!(
        second_page.first().unwrap().timestamp,
        250,
        "Second page should start at timestamp 250"
    );

    assert_eq!(
        second_page.last().unwrap().timestamp,
        299,
        "Second page should end at timestamp 299"
    );

    // Verify no duplicates across both pages
    let mut all_timestamps = HashSet::new();
    for entry in &first_page {
        assert!(
            all_timestamps.insert(entry.timestamp),
            "Found duplicate timestamp: {}",
            entry.timestamp
        );
    }
    for entry in &second_page {
        assert!(
            all_timestamps.insert(entry.timestamp),
            "Found duplicate timestamp: {}",
            entry.timestamp
        );
    }

    // Verify we got all 200 unique entries
    assert_eq!(
        all_timestamps.len(),
        200,
        "Should have retrieved all 200 unique entries"
    );

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(150)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_same_timestamps() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 150 entries all at timestamp 1000
    let entries_file1: Vec<TestEntry> = (0..150)
        .map(|i| {
            TestEntry::new(Microseconds(1000))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
                .with_field("FILE", "1")
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: 150 entries all at timestamp 1000
    let entries_file2: Vec<TestEntry> = (0..150)
        .map(|i| {
            TestEntry::new(Microseconds(1000))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
                .with_field("FILE", "2")
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let file_field = FieldName::new("FILE").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[file_field.clone(), entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[file_field, entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // First page: limit=200, should get all 150 from file1 + 50 from file2
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(200)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        200,
        "First page should contain exactly 200 entries"
    );

    // All timestamps should be 1000
    for entry in &first_page {
        assert_eq!(
            entry.timestamp, 1000,
            "All entries should have timestamp 1000"
        );
    }

    // State should track positions for both files
    assert_eq!(
        state1.file_positions.len(),
        2,
        "State should track positions for both files"
    );

    // Second page: use state to get remaining 100 entries
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(200)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        100,
        "Second page should contain remaining 100 entries"
    );

    // All timestamps should still be 1000
    for entry in &second_page {
        assert_eq!(
            entry.timestamp, 1000,
            "All entries should have timestamp 1000"
        );
    }

    // Collect all ENTRY_ID values to verify uniqueness
    let mut all_entry_ids = HashSet::new();

    for entry in &first_page {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    for entry in &second_page {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Verify we got all 300 unique entries
    assert_eq!(
        all_entry_ids.len(),
        300,
        "Should have retrieved all 300 unique entries"
    );

    // Verify we have entries from both files
    let file1_entries: usize = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file1_"))
        .count();
    let file2_entries: usize = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file2_"))
        .count();

    assert_eq!(file1_entries, 150, "Should have 150 entries from file1");
    assert_eq!(file2_entries, 150, "Should have 150 entries from file2");

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(200)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_overlapping_timestamps() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 (100 entries)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry at {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
                .with_field("FILE", "1")
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=150..250 (100 entries) - overlaps with file1 from 150-199
    let entries_file2: Vec<TestEntry> = (150..250)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry at {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
                .with_field("FILE", "2")
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let file_field = FieldName::new("FILE").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[file_field.clone(), entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[file_field, entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // First page: limit=120
    // Expected: 50 from file1 (100-149) + 50 interleaved from both (150-199) + 20 from file2 (200-219)
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(120)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        120,
        "First page should contain exactly 120 entries"
    );

    // Verify timestamps are in ascending order
    for i in 1..first_page.len() {
        assert!(
            first_page[i - 1].timestamp <= first_page[i].timestamp,
            "Entries should be in ascending timestamp order"
        );
    }

    // First entry should be at timestamp 100
    assert_eq!(
        first_page.first().unwrap().timestamp,
        100,
        "First entry should be at timestamp 100"
    );

    // State should track positions for both files
    assert!(
        !state1.file_positions.is_empty(),
        "State should track positions"
    );

    // Second page: get remaining entries
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(120)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        80,
        "Second page should contain remaining 80 entries"
    );

    // Verify timestamps continue in order
    for i in 1..second_page.len() {
        assert!(
            second_page[i - 1].timestamp <= second_page[i].timestamp,
            "Entries should be in ascending timestamp order"
        );
    }

    // Verify no timestamp gap between pages
    if !first_page.is_empty() && !second_page.is_empty() {
        assert!(
            first_page.last().unwrap().timestamp <= second_page.first().unwrap().timestamp,
            "Second page should continue from first page timestamp"
        );
    }

    // Last entry should be at timestamp 249
    assert_eq!(
        second_page.last().unwrap().timestamp,
        249,
        "Last entry should be at timestamp 249"
    );

    // Collect all ENTRY_ID values to verify uniqueness and completeness
    let mut all_entry_ids = HashSet::new();

    for entry in &first_page {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    for entry in &second_page {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Verify we got all 200 unique entries (100 from each file)
    assert_eq!(
        all_entry_ids.len(),
        200,
        "Should have retrieved all 200 unique entries"
    );

    // Verify we have entries from both files
    let file1_entries: usize = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file1_"))
        .count();
    let file2_entries: usize = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file2_"))
        .count();

    assert_eq!(file1_entries, 100, "Should have 100 entries from file1");
    assert_eq!(file2_entries, 100, "Should have 100 entries from file2");

    // Verify all timestamps from 100-249 are represented
    let mut all_timestamps = HashSet::new();
    for entry in first_page.iter().chain(second_page.iter()) {
        all_timestamps.insert(entry.timestamp);
    }

    // We should have entries at all timestamps from 100-249 (150 unique timestamps)
    for ts in 100..250 {
        assert!(
            all_timestamps.contains(&ts),
            "Missing timestamp: {}",
            ts
        );
    }

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(120)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_three_files() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 (100 entries)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
                .with_field("FILE", "1")
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=200..300 (100 entries)
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
                .with_field("FILE", "2")
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // File 3: entries at t=300..400 (100 entries)
    let entries_file3: Vec<TestEntry> = (300..400)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File3 Entry {}", i))
                .with_field("ENTRY_ID", format!("file3_{}", i))
                .with_field("FILE", "3")
        })
        .collect();

    let file3 = create_test_journal(&temp_dir, "file3.journal", entries_file3).unwrap();

    // Index all three files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let file_field = FieldName::new("FILE").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[file_field.clone(), entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[file_field.clone(), entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index3 = indexer
        .index(
            &file3,
            Some(&source_timestamp_field),
            &[file_field, entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2, index3];

    // First page: limit=125, should get all 100 from file1 + 25 from file2
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(125)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        125,
        "First page should contain exactly 125 entries"
    );

    assert_eq!(first_page.first().unwrap().timestamp, 100);
    assert_eq!(first_page.last().unwrap().timestamp, 224);

    // State should track positions for file1 and file2
    assert_eq!(
        state1.file_positions.len(),
        2,
        "State should track positions for 2 files"
    );

    // Second page: limit=125, should get 75 from file2 + 50 from file3
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(125)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        125,
        "Second page should contain exactly 125 entries"
    );

    assert_eq!(second_page.first().unwrap().timestamp, 225);
    assert_eq!(second_page.last().unwrap().timestamp, 349);

    // State should now track all 3 files
    assert_eq!(
        state2.file_positions.len(),
        3,
        "State should track positions for all 3 files"
    );

    // Third page: remaining 50 from file3
    let (third_page, state3) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(125)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        50,
        "Third page should contain remaining 50 entries"
    );

    assert_eq!(third_page.first().unwrap().timestamp, 350);
    assert_eq!(third_page.last().unwrap().timestamp, 399);

    // Collect all ENTRY_ID values to verify uniqueness
    let mut all_entry_ids = HashSet::new();

    for entry in first_page
        .iter()
        .chain(second_page.iter())
        .chain(third_page.iter())
    {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Verify we got all 300 unique entries
    assert_eq!(
        all_entry_ids.len(),
        300,
        "Should have retrieved all 300 unique entries"
    );

    // Verify distribution: 100 from each file
    let file1_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file1_"))
        .count();
    let file2_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file2_"))
        .count();
    let file3_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file3_"))
        .count();

    assert_eq!(file1_count, 100, "Should have 100 entries from file1");
    assert_eq!(file2_count, 100, "Should have 100 entries from file2");
    assert_eq!(file3_count, 100, "Should have 100 entries from file3");

    // Fourth page should be empty
    let (fourth_page, _state4) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(125)
        .execute_page(Some(&state3))
        .unwrap();

    assert_eq!(
        fourth_page.len(),
        0,
        "Fourth page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_small_limit() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 100 entries at t=100..200
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: 100 entries at t=200..300
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // Use very small limit=30, need multiple pages for file1 alone
    let mut all_entry_ids = HashSet::new();
    let mut state = None;
    let mut page_count = 0;

    // Paginate through all entries with small page size
    loop {
        let (page, new_state) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
            .with_limit(30)
            .execute_page(state.as_ref())
            .unwrap();

        if page.is_empty() {
            break;
        }

        page_count += 1;

        // Verify order within page
        for i in 1..page.len() {
            assert!(
                page[i - 1].timestamp <= page[i].timestamp,
                "Page {} entries should be in ascending order",
                page_count
            );
        }

        // Collect ENTRY_IDs
        for entry in &page {
            for field in &entry.fields {
                if field.field() == "ENTRY_ID" {
                    assert!(
                        all_entry_ids.insert(field.value().to_string()),
                        "Found duplicate ENTRY_ID: {}",
                        field.value()
                    );
                }
            }
        }

        state = Some(new_state);
    }

    // Should need 7 pages: 30+30+30+30+30+30+20 = 200 entries
    assert_eq!(page_count, 7, "Should need exactly 7 pages");

    // Verify we got all 200 unique entries
    assert_eq!(
        all_entry_ids.len(),
        200,
        "Should have retrieved all 200 unique entries"
    );

    // Verify distribution
    let file1_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file1_"))
        .count();
    let file2_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file2_"))
        .count();

    assert_eq!(file1_count, 100, "Should have 100 entries from file1");
    assert_eq!(file2_count, 100, "Should have 100 entries from file2");
}

#[test]
fn test_multi_file_pagination_limit_one() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 10 entries at t=100..110
    let entries_file1: Vec<TestEntry> = (100..110)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: 10 entries at t=110..120
    let entries_file2: Vec<TestEntry> = (110..120)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // Paginate with limit=1 (extreme case)
    let mut all_entry_ids = HashSet::new();
    let mut all_timestamps = Vec::new();
    let mut state = None;
    let mut page_count = 0;

    loop {
        let (page, new_state) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
            .with_limit(1)
            .execute_page(state.as_ref())
            .unwrap();

        if page.is_empty() {
            break;
        }

        page_count += 1;

        // Each page should have exactly 1 entry
        assert_eq!(page.len(), 1, "Each page should have exactly 1 entry");

        // Collect ENTRY_ID and timestamp
        for entry in &page {
            all_timestamps.push(entry.timestamp);
            for field in &entry.fields {
                if field.field() == "ENTRY_ID" {
                    assert!(
                        all_entry_ids.insert(field.value().to_string()),
                        "Found duplicate ENTRY_ID: {}",
                        field.value()
                    );
                }
            }
        }

        state = Some(new_state);
    }

    // Should need 20 pages for 20 entries
    assert_eq!(page_count, 20, "Should need exactly 20 pages");

    // Verify we got all 20 unique entries
    assert_eq!(
        all_entry_ids.len(),
        20,
        "Should have retrieved all 20 unique entries"
    );

    // Verify timestamps are in ascending order
    for i in 1..all_timestamps.len() {
        assert!(
            all_timestamps[i - 1] <= all_timestamps[i],
            "Timestamps should be in ascending order"
        );
    }

    // Verify we got all timestamps from 100-119
    let unique_timestamps: HashSet<_> = all_timestamps.into_iter().collect();
    assert_eq!(
        unique_timestamps.len(),
        20,
        "Should have 20 unique timestamps"
    );
    for ts in 100..120 {
        assert!(
            unique_timestamps.contains(&ts),
            "Missing timestamp: {}",
            ts
        );
    }
}

#[test]
fn test_multi_file_pagination_with_empty_file() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 50 entries at t=100..150
    let entries_file1: Vec<TestEntry> = (100..150)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: Empty (0 entries) - created but not indexed since empty files cannot be indexed
    let entries_file2: Vec<TestEntry> = vec![];
    let _file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // File 3: 50 entries at t=150..200
    let entries_file3: Vec<TestEntry> = (150..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File3 Entry {}", i))
                .with_field("ENTRY_ID", format!("file3_{}", i))
        })
        .collect();

    let file3 = create_test_journal(&temp_dir, "file3.journal", entries_file3).unwrap();

    // Index files - note that empty file cannot be indexed (returns EmptyHistogramInput error)
    // In practice, the system would skip files with no entries
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    // Skip empty file - it cannot be indexed
    // let index2 = indexer.index(&file2, ...) would fail with EmptyHistogramInput

    let index3 = indexer
        .index(
            &file3,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index3];

    // First page: limit=60, should get 50 from file1 + 10 from file3 (skipping empty file2)
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(60)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        60,
        "First page should contain 60 entries"
    );

    assert_eq!(first_page.first().unwrap().timestamp, 100);
    assert_eq!(first_page.last().unwrap().timestamp, 159);

    // Second page: remaining 40 from file3
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(60)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        40,
        "Second page should contain remaining 40 entries"
    );

    assert_eq!(second_page.first().unwrap().timestamp, 160);
    assert_eq!(second_page.last().unwrap().timestamp, 199);

    // Collect all ENTRY_IDs
    let mut all_entry_ids = HashSet::new();
    for entry in first_page.iter().chain(second_page.iter()) {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Verify we got all 100 unique entries (none from empty file)
    assert_eq!(
        all_entry_ids.len(),
        100,
        "Should have retrieved all 100 unique entries"
    );

    let file1_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file1_"))
        .count();
    let file3_count = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file3_"))
        .count();

    assert_eq!(file1_count, 50, "Should have 50 entries from file1");
    assert_eq!(file3_count, 50, "Should have 50 entries from file3");

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(60)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_reverse_file_order() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 (oldest)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=200..300 (middle)
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // File 3: entries at t=300..400 (newest)
    let entries_file3: Vec<TestEntry> = (300..400)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File3 Entry {}", i))
                .with_field("ENTRY_ID", format!("file3_{}", i))
        })
        .collect();

    let file3 = create_test_journal(&temp_dir, "file3.journal", entries_file3).unwrap();

    // Index all three files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index3 = indexer
        .index(
            &file3,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    // Pass files in REVERSE chronological order (newest first)
    // The query should still process them in correct temporal order
    let file_indexes = vec![index3, index2, index1];

    // Query should still return entries in ascending timestamp order
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(150)
        .execute_page(None)
        .unwrap();

    assert_eq!(first_page.len(), 150);

    // First entry should be from file1 (oldest timestamp)
    assert_eq!(
        first_page.first().unwrap().timestamp,
        100,
        "First entry should be at timestamp 100 from file1"
    );

    // Verify ascending order
    for i in 1..first_page.len() {
        assert!(
            first_page[i - 1].timestamp <= first_page[i].timestamp,
            "Entries should be in ascending order"
        );
    }

    // Continue pagination
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(150)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(second_page.len(), 150);

    for i in 1..second_page.len() {
        assert!(
            second_page[i - 1].timestamp <= second_page[i].timestamp,
            "Entries should be in ascending order"
        );
    }

    // Verify continuity between pages
    assert!(
        first_page.last().unwrap().timestamp <= second_page.first().unwrap().timestamp,
        "Second page should continue from first page"
    );

    // Collect all ENTRY_IDs
    let mut all_entry_ids = HashSet::new();
    for entry in first_page.iter().chain(second_page.iter()) {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Should have 300 unique entries
    assert_eq!(
        all_entry_ids.len(),
        300,
        "Should have retrieved all 300 unique entries"
    );

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Head, Direction::Forward)
        .with_limit(150)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(third_page.len(), 0, "Third page should be empty");
}

#[test]
fn test_multi_file_pagination_backward_non_overlapping() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 (100 entries)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
                .with_field("FILE", "1")
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=200..300 (100 entries)
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
                .with_field("FILE", "2")
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let file_field = FieldName::new("FILE").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[file_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[file_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // First page: limit=150, backward from tail, should get all 100 from file2 + 50 from file1
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
        .with_limit(150)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        150,
        "First page should contain exactly 150 entries"
    );

    // Verify timestamps are in descending order
    for i in 1..first_page.len() {
        assert!(
            first_page[i - 1].timestamp >= first_page[i].timestamp,
            "Entries should be in descending timestamp order"
        );
    }

    // First entry should be at timestamp 299 (highest)
    assert_eq!(
        first_page.first().unwrap().timestamp,
        299,
        "First entry should be at timestamp 299"
    );

    // Last entry should be at timestamp 150 (100 from file2: 299-200, then 50 from file1: 199-150)
    assert_eq!(
        first_page.last().unwrap().timestamp,
        150,
        "Last entry of first page should be at timestamp 150"
    );

    // State should track positions for files we read from
    assert!(
        !state1.file_positions.is_empty(),
        "State should track positions"
    );

    // Second page: use state to get remaining 50 entries
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
        .with_limit(150)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        50,
        "Second page should contain remaining 50 entries"
    );

    // Verify timestamps continue in descending order
    assert_eq!(
        second_page.first().unwrap().timestamp,
        149,
        "Second page should start at timestamp 149"
    );

    assert_eq!(
        second_page.last().unwrap().timestamp,
        100,
        "Second page should end at timestamp 100"
    );

    // Verify no duplicates across both pages
    let mut all_timestamps = HashSet::new();
    for entry in &first_page {
        assert!(
            all_timestamps.insert(entry.timestamp),
            "Found duplicate timestamp: {}",
            entry.timestamp
        );
    }
    for entry in &second_page {
        assert!(
            all_timestamps.insert(entry.timestamp),
            "Found duplicate timestamp: {}",
            entry.timestamp
        );
    }

    // Verify we got all 200 unique entries
    assert_eq!(
        all_timestamps.len(),
        200,
        "Should have retrieved all 200 unique entries"
    );

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
        .with_limit(150)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_backward_same_timestamps() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 150 entries all at timestamp 1000
    let entries_file1: Vec<TestEntry> = (0..150)
        .map(|i| {
            TestEntry::new(Microseconds(1000))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
                .with_field("FILE", "1")
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: 150 entries all at timestamp 1000
    let entries_file2: Vec<TestEntry> = (0..150)
        .map(|i| {
            TestEntry::new(Microseconds(1000))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
                .with_field("FILE", "2")
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let file_field = FieldName::new("FILE").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[file_field.clone(), entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[file_field, entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // First page: limit=200, backward from tail
    let (first_page, state1) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
        .with_limit(200)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        200,
        "First page should contain exactly 200 entries"
    );

    // All timestamps should be 1000
    for entry in &first_page {
        assert_eq!(
            entry.timestamp, 1000,
            "All entries should have timestamp 1000"
        );
    }

    // State should track positions for both files
    assert_eq!(
        state1.file_positions.len(),
        2,
        "State should track positions for both files"
    );

    // Second page: use state to get remaining 100 entries
    let (second_page, state2) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
        .with_limit(200)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        100,
        "Second page should contain remaining 100 entries"
    );

    // All timestamps should still be 1000
    for entry in &second_page {
        assert_eq!(
            entry.timestamp, 1000,
            "All entries should have timestamp 1000"
        );
    }

    // Collect all ENTRY_ID values to verify uniqueness
    let mut all_entry_ids = HashSet::new();

    for entry in &first_page {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    for entry in &second_page {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Verify we got all 300 unique entries
    assert_eq!(
        all_entry_ids.len(),
        300,
        "Should have retrieved all 300 unique entries"
    );

    // Verify we have entries from both files
    let file1_entries: usize = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file1_"))
        .count();
    let file2_entries: usize = all_entry_ids
        .iter()
        .filter(|id| id.starts_with("file2_"))
        .count();

    assert_eq!(file1_entries, 150, "Should have 150 entries from file1");
    assert_eq!(file2_entries, 150, "Should have 150 entries from file2");

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
        .with_limit(200)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_backward_limit_one() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 10 entries at t=100..110
    let entries_file1: Vec<TestEntry> = (100..110)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: 10 entries at t=110..120
    let entries_file2: Vec<TestEntry> = (110..120)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // Paginate backward with limit=1 (extreme case)
    let mut all_entry_ids = HashSet::new();
    let mut all_timestamps = Vec::new();
    let mut state = None;
    let mut page_count = 0;

    loop {
        let (page, new_state) = LogQuery::new(&file_indexes, Anchor::Tail, Direction::Backward)
            .with_limit(1)
            .execute_page(state.as_ref())
            .unwrap();

        if page.is_empty() {
            break;
        }

        page_count += 1;

        // Each page should have exactly 1 entry
        assert_eq!(page.len(), 1, "Each page should have exactly 1 entry");

        // Collect ENTRY_ID and timestamp
        for entry in &page {
            all_timestamps.push(entry.timestamp);
            for field in &entry.fields {
                if field.field() == "ENTRY_ID" {
                    assert!(
                        all_entry_ids.insert(field.value().to_string()),
                        "Found duplicate ENTRY_ID: {}",
                        field.value()
                    );
                }
            }
        }

        state = Some(new_state);
    }

    // Should need 20 pages for 20 entries
    assert_eq!(page_count, 20, "Should need exactly 20 pages");

    // Verify we got all 20 unique entries
    assert_eq!(
        all_entry_ids.len(),
        20,
        "Should have retrieved all 20 unique entries"
    );

    // Verify timestamps are in descending order
    for i in 1..all_timestamps.len() {
        assert!(
            all_timestamps[i - 1] >= all_timestamps[i],
            "Timestamps should be in descending order"
        );
    }

    // Verify we got all timestamps from 119 down to 100
    let unique_timestamps: HashSet<_> = all_timestamps.into_iter().collect();
    assert_eq!(
        unique_timestamps.len(),
        20,
        "Should have 20 unique timestamps"
    );
    for ts in 100..120 {
        assert!(
            unique_timestamps.contains(&ts),
            "Missing timestamp: {}",
            ts
        );
    }
}

#[test]
fn test_multi_file_pagination_anchor_timestamp_forward() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 (100 entries)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=200..300 (100 entries)
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // Start from middle timestamp 150 (in file1), forward direction
    let anchor = Anchor::Timestamp(Microseconds(150));

    // First page: limit=80, should get 50 from file1 (150-199) + 30 from file2 (200-229)
    let (first_page, state1) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        80,
        "First page should contain exactly 80 entries"
    );

    // First entry should be at timestamp 150
    assert_eq!(
        first_page.first().unwrap().timestamp,
        150,
        "First entry should be at timestamp 150 (anchor)"
    );

    // Last entry should be at timestamp 229
    assert_eq!(
        first_page.last().unwrap().timestamp,
        229,
        "Last entry should be at timestamp 229"
    );

    // Verify ascending order
    for i in 1..first_page.len() {
        assert!(
            first_page[i - 1].timestamp <= first_page[i].timestamp,
            "Entries should be in ascending order"
        );
    }

    // Second page: get remaining entries
    let (second_page, state2) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        70,
        "Second page should contain remaining 70 entries (230-299)"
    );

    assert_eq!(
        second_page.first().unwrap().timestamp,
        230,
        "Second page should start at timestamp 230"
    );

    assert_eq!(
        second_page.last().unwrap().timestamp,
        299,
        "Second page should end at timestamp 299"
    );

    // Verify no duplicates
    let mut all_entry_ids = HashSet::new();
    for entry in first_page.iter().chain(second_page.iter()) {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Should have 150 entries total (from 150-299)
    assert_eq!(
        all_entry_ids.len(),
        150,
        "Should have retrieved 150 unique entries from timestamp 150 onwards"
    );

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_anchor_timestamp_backward() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: entries at t=100..200 (100 entries)
    let entries_file1: Vec<TestEntry> = (100..200)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: entries at t=200..300 (100 entries)
    let entries_file2: Vec<TestEntry> = (200..300)
        .map(|i| {
            TestEntry::new(Microseconds(i))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // Start from middle timestamp 250 (in file2), backward direction
    let anchor = Anchor::Timestamp(Microseconds(250));

    // First page: limit=80, should get 51 from file2 (250-200) + 29 from file1 (199-171)
    let (first_page, state1) = LogQuery::new(&file_indexes, anchor, Direction::Backward)
        .with_limit(80)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        80,
        "First page should contain exactly 80 entries"
    );

    // First entry should be at timestamp 250
    assert_eq!(
        first_page.first().unwrap().timestamp,
        250,
        "First entry should be at timestamp 250 (anchor)"
    );

    // Last entry should be at timestamp 171
    assert_eq!(
        first_page.last().unwrap().timestamp,
        171,
        "Last entry should be at timestamp 171"
    );

    // Verify descending order
    for i in 1..first_page.len() {
        assert!(
            first_page[i - 1].timestamp >= first_page[i].timestamp,
            "Entries should be in descending order"
        );
    }

    // Second page: get remaining entries
    let (second_page, state2) = LogQuery::new(&file_indexes, anchor, Direction::Backward)
        .with_limit(80)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        71,
        "Second page should contain remaining 71 entries (170-100)"
    );

    assert_eq!(
        second_page.first().unwrap().timestamp,
        170,
        "Second page should start at timestamp 170"
    );

    assert_eq!(
        second_page.last().unwrap().timestamp,
        100,
        "Second page should end at timestamp 100"
    );

    // Verify no duplicates
    let mut all_entry_ids = HashSet::new();
    for entry in first_page.iter().chain(second_page.iter()) {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Should have 151 entries total (from 250 down to 100)
    assert_eq!(
        all_entry_ids.len(),
        151,
        "Should have retrieved 151 unique entries from timestamp 250 backwards to 100"
    );

    // Third page should be empty
    let (third_page, _state3) = LogQuery::new(&file_indexes, anchor, Direction::Backward)
        .with_limit(80)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        0,
        "Third page should be empty (no more entries)"
    );
}

#[test]
fn test_multi_file_pagination_anchor_timestamp_same_timestamps() {
    // Create temporary directory
    let temp_dir = TempDir::new().unwrap();

    // File 1: 100 entries all at timestamp 150
    let entries_file1: Vec<TestEntry> = (0..100)
        .map(|i| {
            TestEntry::new(Microseconds(150))
                .with_field("MESSAGE", format!("File1 Entry {}", i))
                .with_field("ENTRY_ID", format!("file1_{}", i))
        })
        .collect();

    let file1 = create_test_journal(&temp_dir, "file1.journal", entries_file1).unwrap();

    // File 2: 100 entries all at timestamp 150
    let entries_file2: Vec<TestEntry> = (0..100)
        .map(|i| {
            TestEntry::new(Microseconds(150))
                .with_field("MESSAGE", format!("File2 Entry {}", i))
                .with_field("ENTRY_ID", format!("file2_{}", i))
        })
        .collect();

    let file2 = create_test_journal(&temp_dir, "file2.journal", entries_file2).unwrap();

    // Index both files
    let mut indexer = FileIndexer::default();
    let source_timestamp_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP").unwrap();
    let entry_id_field = FieldName::new("ENTRY_ID").unwrap();

    let index1 = indexer
        .index(
            &file1,
            Some(&source_timestamp_field),
            &[entry_id_field.clone()],
            Seconds(3600),
        )
        .unwrap();

    let index2 = indexer
        .index(
            &file2,
            Some(&source_timestamp_field),
            &[entry_id_field],
            Seconds(3600),
        )
        .unwrap();

    let file_indexes = vec![index1, index2];

    // Anchor at timestamp 150 (all entries have this timestamp), forward direction
    let anchor = Anchor::Timestamp(Microseconds(150));

    // First page: limit=80
    let (first_page, state1) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(None)
        .unwrap();

    assert_eq!(
        first_page.len(),
        80,
        "First page should contain exactly 80 entries"
    );

    // All timestamps should be 150
    for entry in &first_page {
        assert_eq!(
            entry.timestamp, 150,
            "All entries should have timestamp 150"
        );
    }

    // Second page: get remaining entries
    let (second_page, state2) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(Some(&state1))
        .unwrap();

    assert_eq!(
        second_page.len(),
        80,
        "Second page should contain 80 entries"
    );

    for entry in &second_page {
        assert_eq!(
            entry.timestamp, 150,
            "All entries should have timestamp 150"
        );
    }

    // Third page: remaining entries
    let (third_page, state3) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(Some(&state2))
        .unwrap();

    assert_eq!(
        third_page.len(),
        40,
        "Third page should contain remaining 40 entries"
    );

    for entry in &third_page {
        assert_eq!(
            entry.timestamp, 150,
            "All entries should have timestamp 150"
        );
    }

    // Verify no duplicates
    let mut all_entry_ids = HashSet::new();
    for entry in first_page
        .iter()
        .chain(second_page.iter())
        .chain(third_page.iter())
    {
        for field in &entry.fields {
            if field.field() == "ENTRY_ID" {
                assert!(
                    all_entry_ids.insert(field.value().to_string()),
                    "Found duplicate ENTRY_ID: {}",
                    field.value()
                );
            }
        }
    }

    // Should have all 200 entries
    assert_eq!(
        all_entry_ids.len(),
        200,
        "Should have retrieved all 200 unique entries"
    );

    // Fourth page should be empty
    let (fourth_page, _state4) = LogQuery::new(&file_indexes, anchor, Direction::Forward)
        .with_limit(80)
        .execute_page(Some(&state3))
        .unwrap();

    assert_eq!(
        fourth_page.len(),
        0,
        "Fourth page should be empty (no more entries)"
    );
}
