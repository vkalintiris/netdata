use clap::Parser;
use journal::file::Mmap;
use journal::index::FileIndexer;
use journal::repository::File;
use journal::{FieldName, JournalFile};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Stress test journal file indexing to reproduce race conditions"
)]
struct Args {
    /// Path to the journal file
    #[arg(long)]
    file: PathBuf,

    /// Number of times to index the file consecutively
    #[arg(long, default_value = "100")]
    repeat: usize,

    /// Bucket duration for histogram (in seconds)
    #[arg(long, default_value = "1")]
    bucket_duration: u32,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    println!("Stress testing journal file indexing");
    println!("File: {}", args.file.display());
    println!("Iterations: {}", args.repeat);
    println!("Bucket duration: {} seconds", args.bucket_duration);
    println!();

    // Source timestamp field
    let source_field = FieldName::new_unchecked("_SOURCE_REALTIME_TIMESTAMP");

    // Indexed fields (common ones)
    let indexed_fields = vec![
        FieldName::new_unchecked("PRIORITY"),
        // FieldName::new_unchecked("SYSLOG_IDENTIFIER"),
        // FieldName::new_unchecked("_SYSTEMD_UNIT"),
    ];

    // Convert PathBuf to File
    let file = File::from_path(&args.file)
        .ok_or_else(|| format!("Invalid journal file path: {}", args.file.display()))?;

    for iteration in 1..=args.repeat {
        println!("Iteration {}/{}", iteration, args.repeat);

        // Open the journal file
        let journal_file = JournalFile::<Mmap>::open(&file, 8 * 1024 * 1024)?;

        // Create indexer and index the file
        let mut indexer = FileIndexer::default();
        let _file_index = indexer.index(
            &journal_file,
            Some(&source_field),
            &indexed_fields,
            args.bucket_duration,
        )?;
    }

    println!();
    println!("✓ All {} iterations completed successfully!", args.repeat);

    Ok(())
}
