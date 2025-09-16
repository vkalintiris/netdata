use journal_file::JournalFile;
use journal_file::Mmap;
use journal_registry::JournalRegistry;
use std::time::Instant;
use tracing::{info, instrument, warn};

/// A single entry in the time histogram index representing a minute boundary.
#[derive(Debug, Clone, Copy)]
struct HistogramBucket {
    /// Index into the offsets vector where this minute's entries begin.
    offset_index: usize,
    /// The absolute minute value (microseconds / 60_000_000) since epoch.
    minute: u64,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
///
/// This structure stores minute boundaries and their corresponding offset indices,
/// enabling O(log n) lookups for time ranges and histogram generation with configurable
/// bucket sizes (1-minute, 10-minute, etc.).
#[derive(Clone)]
struct HistogramIndex {
    /// The first minute in the dataset, used as reference for relative calculations.
    base_minute: u64,
    /// Sparse vector containing only minute boundaries where changes occur.
    buckets: Vec<HistogramBucket>,
}

impl HistogramIndex {
    fn new(base_minute: u64) -> Self {
        Self {
            buckets: Vec::new(),
            base_minute,
        }
    }

    fn push(&mut self, minute: u64, offset_index: usize) {
        self.buckets.push(HistogramBucket {
            minute,
            offset_index,
        });
    }

    fn len(&self) -> usize {
        self.buckets.len()
    }
}

impl std::fmt::Debug for HistogramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(
            f,
            "{:<10} {:<15} {:<15} {:<30}",
            "Entry#", "Minute", "Offset Index", "Timestamp"
        )?;
        writeln!(f, "{}", "-".repeat(70))?;

        for (i, entry) in self.buckets.iter().take(10).enumerate() {
            writeln!(
                f,
                "{:<10} {:<15} {:<15} minute {}",
                i,
                entry.minute - self.base_minute,
                entry.offset_index,
                entry.minute
            )?;
        }

        if self.buckets.len() > 10 {
            writeln!(f, "... and {} more entries", self.buckets.len() - 10)?;
        }
        Ok(())
    }
}

impl std::fmt::Display for HistogramIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.buckets.is_empty() {
            return write!(f, "Empty index");
        }

        writeln!(f, "Time ranges and entry counts:")?;
        writeln!(f, "{:<30} {:<10}", "Range", "Count")?;
        writeln!(f, "{}", "-".repeat(40))?;

        for window in self.buckets.windows(2) {
            let start_minute = window[0].minute;
            let end_minute = window[1].minute;
            let count = window[1].offset_index - window[0].offset_index;

            writeln!(
                f,
                "{:02}:{:02} - {:02}:{:02} ({}m)          {}",
                (start_minute % (24 * 60)) / 60, // hours
                start_minute % 60,               // minutes
                (end_minute % (24 * 60)) / 60,
                end_minute % 60,
                end_minute - start_minute,
                count
            )?;
        }
        Ok(())
    }
}

#[instrument(skip(files))]
fn sequential(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut count = 0;
    let mut offsets = Vec::new();
    let mut midx_count = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        offsets.clear();

        let Some(entry_list) = journal_file.entry_list() else {
            continue;
        };

        entry_list
            .collect_offsets(&journal_file, &mut offsets)
            .unwrap();

        let first_timestamp = offsets
            .first()
            .and_then(|eo| journal_file.entry_ref(*eo).ok())
            .map(|entry| entry.header.realtime);

        let Some(first_timestamp) = first_timestamp else {
            continue;
        };

        let first_minute = first_timestamp / (60 * 1_000_000);

        // Use the new type
        let mut minute_index = HistogramIndex::new(first_minute);
        let mut current_minute = first_minute;
        minute_index.push(current_minute, 0);

        for (idx, &offset) in offsets.iter().enumerate().skip(1) {
            let entry = journal_file.entry_ref(offset).unwrap();
            let entry_minute = entry.header.realtime / (60 * 1_000_000);

            if entry_minute > current_minute {
                minute_index.push(entry_minute, idx);
                current_minute = entry_minute;
            }
        }

        midx_count += minute_index.len();

        count += offsets.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} entry offsets in {:#?} msec (midx: {:#?})",
        count,
        elapsed.as_millis(),
        midx_count,
    );
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    for dir in ["/var/log/journal", "/run/log/journal"] {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    let mut files = registry.query().execute();
    files.sort_by_key(|x| x.path.clone());
    files.sort_by_key(|x| x.size);
    files.reverse();
    files.truncate(5);

    sequential(&files);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}
