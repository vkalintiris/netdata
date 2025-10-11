use journal_log::{JournalLog, JournalLogConfig};
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a temporary directory for the journal files
    let temp_dir = tempfile::tempdir()?;
    let journal_path = temp_dir.path();

    println!("Creating journal in: {}", journal_path.display());

    // Configure the journal with rotation and retention policies
    let config = JournalLogConfig::new(journal_path)
        .with_rotation_policy(
            journal_log::RotationPolicy::default()
                .with_size_of_journal_file(10 * 1024 * 1024) // 10MB max file size
                .with_duration_of_journal_file(Duration::from_secs(3600)), // 1 hour max span
        )
        .with_retention_policy(
            journal_log::RetentionPolicy::default()
                .with_number_of_journal_files(5) // Keep max 5 files
                .with_size_of_journal_files(50 * 1024 * 1024) // 50MB total
                .with_duration_of_journal_files(Duration::from_secs(24 * 3600)), // 24 hours
        );

    // Create the journal log
    let mut journal = JournalLog::new(config)?;

    println!("\nWriting log entries...\n");

    // Write a simple log entry with MESSAGE field
    journal.write_entry(&[b"MESSAGE=Hello, world! This is my first journal entry."])?;
    journal.write_entry(&[b"_GVD_SOURCE=what"])?;
    journal.write_entry(&[b"sjflkjw.wjef=what"])?;
    println!("✓ Wrote entry 1: Simple message");

    // Write a structured log entry with multiple fields
    journal.write_entry(&[
        b"MESSAGE=Application started successfully",
        b"PRIORITY=6", // Informational
        b"SYSLOG_IDENTIFIER=my_app",
        b"_PID=12345",
    ])?;
    println!("✓ Wrote entry 2: Application start event");

    // Write an error log entry
    journal.write_entry(&[
        b"MESSAGE=Failed to connect to database",
        b"PRIORITY=3", // Error
        b"SYSLOG_IDENTIFIER=my_app",
        b"_PID=12345",
        b"ERRNO=111", // Connection refused
        b"DATABASE_HOST=localhost",
        b"DATABASE_PORT=5432",
    ])?;
    println!("✓ Wrote entry 3: Database connection error");

    // Write a warning log entry
    journal.write_entry(&[
        b"MESSAGE=High memory usage detected",
        b"PRIORITY=4", // Warning
        b"SYSLOG_IDENTIFIER=my_app",
        b"_PID=12345",
        b"MEMORY_USAGE_MB=8192",
        b"MEMORY_LIMIT_MB=10240",
    ])?;
    println!("✓ Wrote entry 4: Memory warning");

    // Write a systemd-style log entry
    journal.write_entry(&[
        b"MESSAGE=Service operation completed",
        b"PRIORITY=6",
        b"SYSLOG_IDENTIFIER=my_service",
        b"_SYSTEMD_UNIT=my_service.service",
        b"_SYSTEMD_CGROUP=/system.slice/my_service.service",
        b"OPERATION=backup",
        b"DURATION_MS=1234",
        b"STATUS=success",
    ])?;
    println!("✓ Wrote entry 5: Systemd-style service log");

    // Write a Netdata-specific log entry
    journal.write_entry(&[
        b"MESSAGE=Alert triggered: high CPU usage",
        b"PRIORITY=4",
        b"ND_ALERT_NAME=cpu_usage",
        b"ND_ALERT_CLASS=System",
        b"ND_ALERT_COMPONENT=CPU",
        b"ND_ALERT_TYPE=warning",
        b"ND_ALERT_STATUS=raised",
        b"ND_NIDL_NODE=localhost",
        b"ND_NIDL_CONTEXT=system.cpu",
    ])?;
    println!("✓ Wrote entry 6: Netdata alert");

    println!("\n✅ Successfully wrote 6 log entries to journal!");
    println!("\nJournal files created in: {}", journal_path.display());

    // List the created journal files
    println!("\nCreated journal files:");
    for entry in std::fs::read_dir(journal_path)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("journal") {
            let metadata = std::fs::metadata(&path)?;
            println!(
                "  - {} ({} bytes)",
                path.file_name().unwrap().to_string_lossy(),
                metadata.len()
            );
        }
    }

    // Keep the temporary directory around so user can inspect it
    println!("\nTo inspect the journal files, use:");
    println!("  journalctl --directory={}", journal_path.display());
    println!("\nPress Enter to clean up and exit...");

    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;

    Ok(())
}
