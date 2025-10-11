use journal_log::{JournalLog, JournalLogConfig, RotationPolicy};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory for the journal files
    let temp_dir = tempfile::tempdir()?;
    let journal_path = temp_dir.path();

    println!("Creating journal in: {}\n", journal_path.display());

    // Configure the journal with aggressive rotation policy
    let config = JournalLogConfig::new(journal_path)
        .with_rotation_policy(
            RotationPolicy::default()
                .with_number_of_entries(3), // Rotate after 3 entries
        );

    // Create the journal log
    let mut journal = JournalLog::new(config)?;

    println!("Writing 10 entries with rotation after every 3 entries:\n");

    // Write entries one by one and show when rotation happens
    for i in 1..=10 {
        journal.write_entry(&[format!("MESSAGE=Entry {}", i).as_bytes()])?;
        println!("✓ Wrote entry {}", i);

        if i % 3 == 0 && i < 10 {
            println!("  → Rotation triggered\n");
        }
    }

    println!("\n✅ All entries written!\n");
    println!("Journal files (sorted by head_seqnum):");
    println!("=======================================\n");

    // Collect and sort files by seqnum
    use journal_log::JournalFileInfo;
    let mut all_files = Vec::new();

    for entry in std::fs::read_dir(journal_path)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            for file_entry in std::fs::read_dir(&path)? {
                let file_entry = file_entry?;
                let file_path = file_entry.path();

                if file_path.extension().and_then(|s| s.to_str()) == Some("journal") {
                    if let Ok(info) = JournalFileInfo::from_path(&file_path) {
                        all_files.push(info);
                    }
                }
            }
        }
    }

    // Sort by head_seqnum
    all_files.sort_by_key(|f| f.head_seqnum);

    for (idx, file) in all_files.iter().enumerate() {
        println!(
            "File {}: seqnum={}, realtime={:016x}",
            idx + 1,
            file.head_seqnum,
            file.head_realtime
        );
        println!("  {}", file.path.display());
        println!();
    }

    println!("Expected sequence:");
    println!("  File 1: seqnum=1  (entries 1, 2, 3)");
    println!("  File 2: seqnum=4  (entries 4, 5, 6)");
    println!("  File 3: seqnum=7  (entries 7, 8, 9)");
    println!("  File 4: seqnum=10 (entry 10)");

    Ok(())
}
