use journal_log::{JournalLog, JournalLogConfig, RotationPolicy};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory for the journal files
    let temp_dir = tempfile::tempdir()?;
    let journal_path = temp_dir.path();

    println!("Creating journal in: {}", journal_path.display());
    println!();

    // Configure the journal with aggressive rotation policy to create multiple files
    let config = JournalLogConfig::new(journal_path)
        .with_rotation_policy(
            RotationPolicy::default()
                .with_number_of_entries(3), // Rotate after just 3 entries
        );

    // Create the journal log
    let mut journal = JournalLog::new(config)?;

    println!("Writing entries to trigger rotation...\n");

    // Write entries - should create multiple files due to rotation policy
    for i in 1..=10 {
        journal.write_entry(&[format!("MESSAGE=Log entry {}", i).as_bytes()])?;
        println!("✓ Wrote entry {}", i);
    }

    println!("\n✅ Successfully wrote 10 log entries to journal!");
    println!("\nJournal directory structure:");
    println!("============================\n");

    // Walk the directory tree to show the systemd-style structure
    for entry in std::fs::read_dir(journal_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // This should be the machine_id directory
            let dir_name = path.file_name().unwrap().to_string_lossy();
            println!("📁 {}/", dir_name);

            // List journal files in this directory
            for file_entry in std::fs::read_dir(&path)? {
                let file_entry = file_entry?;
                let file_path = file_entry.path();

                if file_path.extension().and_then(|s| s.to_str()) == Some("journal") {
                    let filename = file_path.file_name().unwrap().to_string_lossy();
                    let metadata = std::fs::metadata(&file_path)?;
                    println!("  📄 {} ({} bytes)", filename, metadata.len());
                }
            }
        }
    }

    println!("\n✨ Paths follow systemd journal format:");
    println!("   {{journal_dir}}/{{machine_id}}/system@{{seqnum_id}}-{{head_seqnum}}-{{head_realtime}}.journal");

    println!("\nPress Enter to clean up and exit...");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}
