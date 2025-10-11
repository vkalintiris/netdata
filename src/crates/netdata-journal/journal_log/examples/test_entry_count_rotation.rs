use journal_log::{JournalLog, JournalLogConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory for the journal files
    let temp_dir = tempfile::tempdir()?;
    let journal_path = temp_dir.path();

    println!("Creating journal in: {}", journal_path.display());

    // Configure the journal with entry count rotation policy
    let config = JournalLogConfig::new(journal_path)
        .with_rotation_policy(
            journal_log::RotationPolicy::default()
                .with_number_of_entries(5), // Rotate after 5 entries
        )
        .with_retention_policy(
            journal_log::RetentionPolicy::default()
                .with_number_of_journal_files(10), // Keep max 10 files
        );

    // Create the journal log
    let mut journal = JournalLog::new(config)?;

    println!("\nWriting log entries to test entry count rotation...\n");

    // Write 15 entries - should create 3 files (5 entries each)
    for i in 1..=15 {
        let message = format!("MESSAGE=Entry number {}", i);
        journal.write_entry(&[message.as_bytes()])?;
        println!("✓ Wrote entry {}", i);
    }

    println!("\n✅ Successfully wrote 15 log entries to journal!");
    println!("\nJournal files created in: {}", journal_path.display());

    // List the created journal files
    println!("\nCreated journal files:");
    let mut file_count = 0;
    for entry in std::fs::read_dir(journal_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("journal") {
            file_count += 1;
            let metadata = std::fs::metadata(&path)?;
            println!(
                "  - {} ({} bytes)",
                path.file_name().unwrap().to_string_lossy(),
                metadata.len()
            );
        }
    }

    println!("\nTotal journal files created: {}", file_count);

    if file_count >= 3 {
        println!("✅ Entry count rotation is working correctly!");
        println!("   Expected at least 3 files for 15 entries with 5 entries per file.");
    } else {
        println!("⚠️  Warning: Expected at least 3 files, but got {}", file_count);
    }

    println!("\nTo inspect the journal files, use:");
    println!("  journalctl --directory={}", journal_path.display());
    println!("\nPress Enter to clean up and exit...");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}
