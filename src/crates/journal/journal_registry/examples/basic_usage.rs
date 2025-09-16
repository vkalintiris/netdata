use journal_file::JournalFile;
use journal_file::Mmap;
use journal_file::histogram::HistogramIndex;
use journal_registry::JournalRegistry;
use std::time::Instant;
use tracing::{info, instrument, warn};

#[instrument(skip(files))]
fn sequential(files: &[journal_registry::RegistryFile]) {
    let start_time = Instant::now();

    let mut midx_count = 0;

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        let Some(histogram_index) = HistogramIndex::from(&journal_file).unwrap() else {
            continue;
        };

        midx_count += histogram_index.len();
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} histogram index buckets in {:#?} msec",
        midx_count,
        elapsed.as_millis(),
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
    // files.truncate(5);

    sequential(&files);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}
