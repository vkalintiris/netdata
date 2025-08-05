use arbitrary::{Arbitrary, Unstructured};
use journal_file::index::FileIndexer;
use journal_file::{JournalFile, JournalFileOptions, JournalWriter, Mmap};
use rand::prelude::*;
use rustc_hash::FxHashMap;
use std::path::Path;

/// Configuration for fuzzy test generation
#[derive(Debug, Clone)]
struct FuzzConfig {
    /// Number of unique timestamps to generate
    num_unique_timestamps: usize,
    /// Number of unique priority values
    num_unique_priorities: usize,
    /// Number of unique test field values
    num_unique_test_fields: usize,
    /// Number of unique messages
    num_unique_messages: usize,
    /// Total number of entries to generate
    num_entries: usize,
    /// Whether to allow duplicate entries
    allow_duplicates: bool,
    /// Bucket size for histogram in seconds
    bucket_size_seconds: u64,
}

impl<'a> Arbitrary<'a> for FuzzConfig {
    fn arbitrary(u: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        Ok(FuzzConfig {
            num_unique_timestamps: u.int_in_range(1..=20)?,
            num_unique_priorities: u.int_in_range(1..=7)?,
            num_unique_test_fields: u.int_in_range(1..=10)?,
            num_unique_messages: u.int_in_range(1..=15)?,
            num_entries: u.int_in_range(10..=100)?,
            allow_duplicates: u.arbitrary()?,
            bucket_size_seconds: *u.choose(&[1, 5, 10, 30, 60, 120])?,
        })
    }
}

/// Represents a journal entry for testing
#[derive(Debug, Clone)]
struct TestEntry {
    source_timestamp: u64,
    priority: u32,
    test_field: String,
    message: String,
}

/// Test data generator that creates valid journal entries
struct TestDataGenerator {
    config: FuzzConfig,
    rng: StdRng,
}

impl TestDataGenerator {
    fn new(config: FuzzConfig, seed: u64) -> Self {
        Self {
            config,
            rng: StdRng::seed_from_u64(seed),
        }
    }

    fn generate_entries(&mut self) -> Vec<TestEntry> {
        // Generate pools of values to choose from
        let timestamps = self.generate_timestamps();
        let priorities = self.generate_priorities();
        let test_fields = self.generate_test_fields();
        let messages = self.generate_messages();

        // Generate entries by randomly selecting from pools
        let mut entries = Vec::with_capacity(self.config.num_entries);

        for _ in 0..self.config.num_entries {
            let entry = TestEntry {
                source_timestamp: *timestamps.choose(&mut self.rng).unwrap(),
                priority: *priorities.choose(&mut self.rng).unwrap(),
                test_field: test_fields.choose(&mut self.rng).unwrap().clone(),
                message: messages.choose(&mut self.rng).unwrap().clone(),
            };
            entries.push(entry);
        }

        // Optionally shuffle to simulate real-world unordered data
        if self.rng.r#gen_bool(0.5) {
            entries.shuffle(&mut self.rng);
        }

        entries
    }

    fn generate_timestamps(&mut self) -> Vec<u64> {
        let mut timestamps = Vec::with_capacity(self.config.num_unique_timestamps);

        // Generate timestamps spread across different time ranges
        let base_time = 1_700_000_000_000_000u64; // Some base timestamp in microseconds
        let time_spread = 3_600_000_000u64; // 1 hour in microseconds

        for i in 0..self.config.num_unique_timestamps {
            let offset = (i as u64) * (time_spread / self.config.num_unique_timestamps as u64);
            let jitter = self.rng.r#gen_range(0..1_000_000); // Add some jitter
            timestamps.push(base_time + offset + jitter);
        }

        timestamps
    }

    fn generate_priorities(&mut self) -> Vec<u32> {
        (0..self.config.num_unique_priorities)
            .map(|i| i as u32)
            .collect()
    }

    fn generate_test_fields(&mut self) -> Vec<String> {
        (0..self.config.num_unique_test_fields)
            .map(|i| format!("test_value_{}", i))
            .collect()
    }

    fn generate_messages(&mut self) -> Vec<String> {
        (0..self.config.num_unique_messages)
            .map(|i| format!("Message content {}", i))
            .collect()
    }
}

/// Helper to generate deterministic UUIDs for journal file
fn generate_uuid(seed: u8) -> [u8; 16] {
    [seed; 16]
}

/// Creates a journal file with the given entries
fn create_journal_file(
    path: &Path,
    entries: &[TestEntry],
) -> Result<(), Box<dyn std::error::Error>> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }

    let options = JournalFileOptions::new(
        generate_uuid(1),
        generate_uuid(2),
        generate_uuid(3),
        generate_uuid(4),
    );

    let mut journal_file = JournalFile::create(path, options)?;
    let boot_id = [1; 16];
    let mut writer = JournalWriter::new(&mut journal_file)?;

    for entry in entries {
        let entry_data = vec![
            format!("_SOURCE_REALTIME_TIMESTAMP={}", entry.source_timestamp).into_bytes(),
            format!("PRIORITY={}", entry.priority).into_bytes(),
            format!("TEST_FIELD={}", entry.test_field).into_bytes(),
            format!("MESSAGE={}", entry.message).into_bytes(),
        ];

        let entry_refs: Vec<&[u8]> = entry_data.iter().map(|v| v.as_slice()).collect();

        writer.add_entry(
            &mut journal_file,
            &entry_refs,
            entry.source_timestamp,
            entry.source_timestamp,
            boot_id,
        )?;
    }

    Ok(())
}

/// Verification logic for histogram correctness
struct HistogramVerifier {
    entries: Vec<TestEntry>,
}

impl HistogramVerifier {
    fn new(entries: Vec<TestEntry>) -> Self {
        Self { entries }
    }

    /// Manually calculate what the histogram should be for a given field value
    fn calculate_expected_histogram(
        &self,
        field_name: &str,
        field_value: &str,
        bucket_size_seconds: u64,
    ) -> Vec<(u64, u32)> {
        // Filter entries that match the field value
        let matching_entries: Vec<&TestEntry> = self
            .entries
            .iter()
            .filter(|entry| match field_name {
                "TEST_FIELD" => entry.test_field == field_value,
                "PRIORITY" => entry.priority.to_string() == field_value,
                "MESSAGE" => entry.message == field_value,
                _ => false,
            })
            .collect();

        if matching_entries.is_empty() {
            return Vec::new();
        }

        // Group by time buckets
        let mut bucket_counts: FxHashMap<u64, u32> = FxHashMap::default();
        let bucket_size_micros = bucket_size_seconds * 1_000_000;

        for entry in matching_entries {
            let bucket = (entry.source_timestamp / bucket_size_micros) * bucket_size_seconds;
            *bucket_counts.entry(bucket).or_insert(0) += 1;
        }

        // Convert to sorted vec
        let mut histogram: Vec<(u64, u32)> = bucket_counts.into_iter().collect();
        histogram.sort_by_key(|&(bucket, _)| bucket);
        histogram
    }

    /// Verify that the indexed histogram matches our expected calculation
    fn verify_histogram(
        &self,
        file_index: &journal_file::index::FileIndex,
        field_name: &str,
        field_value: &str,
        bucket_size_seconds: u64,
    ) -> Result<(), String> {
        let key = format!("{}={}", field_name, field_value);

        // Get the histogram from the index
        let bitmap = file_index
            .entries_index
            .get(&key)
            .ok_or_else(|| format!("Key '{}' not found in index", key))?;

        let actual_histogram = file_index.file_histogram.from_bitmap(bitmap);

        // Calculate expected histogram
        let expected_histogram =
            self.calculate_expected_histogram(field_name, field_value, bucket_size_seconds);

        // Compare histograms
        if actual_histogram.len() != expected_histogram.len() {
            return Err(format!(
                "Histogram length mismatch for '{}': expected {}, got {}",
                key,
                expected_histogram.len(),
                actual_histogram.len()
            ));
        }

        for (i, (expected, actual)) in expected_histogram
            .iter()
            .zip(actual_histogram.iter())
            .enumerate()
        {
            if expected.0 != actual.0 || expected.1 != actual.1 {
                return Err(format!(
                    "Histogram mismatch at index {} for '{}': expected {:?}, got {:?}",
                    i, key, expected, actual
                ));
            }
        }

        Ok(())
    }

    /// Verify all field values in the index
    fn verify_all_fields(
        &self,
        file_index: &journal_file::index::FileIndex,
        bucket_size_seconds: u64,
    ) -> Result<(), String> {
        // Collect all unique field values
        let mut test_fields: Vec<String> =
            self.entries.iter().map(|e| e.test_field.clone()).collect();
        test_fields.sort();
        test_fields.dedup();

        let mut priorities: Vec<String> = self
            .entries
            .iter()
            .map(|e| e.priority.to_string())
            .collect();
        priorities.sort();
        priorities.dedup();

        // Verify TEST_FIELD histograms
        for field_value in &test_fields {
            self.verify_histogram(file_index, "TEST_FIELD", field_value, bucket_size_seconds)?;
        }

        // Verify PRIORITY histograms
        for priority_value in &priorities {
            self.verify_histogram(file_index, "PRIORITY", priority_value, bucket_size_seconds)?;
        }

        println!(
            "✓ Successfully verified {} TEST_FIELD and {} PRIORITY histograms",
            test_fields.len(),
            priorities.len()
        );

        Ok(())
    }
}

/// Main fuzzing test runner
fn run_fuzz_test(config: FuzzConfig, seed: u64) -> Result<(), Box<dyn std::error::Error>> {
    println!("\n--- Running fuzz test with seed {} ---", seed);
    println!("Config: {:?}", config);

    // Generate test entries
    let mut generator = TestDataGenerator::new(config.clone(), seed);
    let entries = generator.generate_entries();
    println!("Generated {} entries:", entries.len());

    // Print each entry in JSON-like format on a single line
    // for entry in &entries {
    //     println!(
    //         r#"{{"_SOURCE_REALTIME_TIMESTAMP":{},"PRIORITY":{},"TEST_FIELD":"{}","MESSAGE":"{}"}}"#,
    //         entry.source_timestamp, entry.priority, entry.test_field, entry.message
    //     );
    // }

    // Create journal file in RAM filesystem to avoid SSD wear
    let journal_path = Path::new("/mnt/ramfs/fuzz_test.journal");
    create_journal_file(journal_path, &entries)?;

    // Index the journal file
    let journal_file = JournalFile::<Mmap>::open(journal_path, 8 * 1024 * 1024)?;
    let mut file_indexer = FileIndexer::default();

    let field_names = vec![
        b"MESSAGE".as_slice(),
        b"PRIORITY".as_slice(),
        b"TEST_FIELD".as_slice(),
    ];

    let file_index = file_indexer.index(
        &journal_file,
        b"_SOURCE_REALTIME_TIMESTAMP",
        &field_names,
        config.bucket_size_seconds,
    )?;

    // Verify histogram correctness
    let verifier = HistogramVerifier::new(entries);
    verifier.verify_all_fields(&file_index, config.bucket_size_seconds)?;

    // Additional sanity checks
    let total_entries = file_index.file_histogram.total_entries();
    if total_entries != config.num_entries {
        return Err(format!(
            "Total entries mismatch: expected {}, got {}",
            config.num_entries, total_entries
        )
        .into());
    }

    println!("✓ All verifications passed!");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Run multiple fuzz tests with different seeds
    let num_tests = 100_000_000;
    // let num_tests = 3;
    let mut rng = thread_rng();

    for i in 0..num_tests {
        let seed = rng.r#gen::<u64>();

        // Generate arbitrary config
        let config_bytes: Vec<u8> = (0..256).map(|_| rng.r#gen()).collect();
        let mut u = Unstructured::new(&config_bytes);
        let config = FuzzConfig::arbitrary(&mut u)?;

        println!("\nTest {}/{}", i + 1, num_tests);

        match run_fuzz_test(config, seed) {
            Ok(_) => println!("Test passed"),
            Err(e) => {
                eprintln!("Test failed with seed {}: {}", seed, e);
                return Err(e);
            }
        }
    }

    println!("\n All {} fuzz tests passed!", num_tests);
    Ok(())
}
