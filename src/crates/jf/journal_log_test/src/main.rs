use journal_log::{JournalLog, JournalLogConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = "/tmp/journal_log_test";
    let config = JournalLogConfig::new(test_dir)
        .with_max_file_size(512 * 1024)
        .with_max_entry_age_secs(365 * 24 * 3600)
        .with_max_files(1000)
        .with_max_total_size(1 * 1024 * 1024 * 1024);

    println!("Configuration: {:#?}", config);

    let mut journal = JournalLog::new(config)?;
    println!("\nWriting log entries to trigger rotation...");

    for i in 0..10000 {
        let message = format!("LOG_ENTRY_{:03}: This is a test log entry with some data to make it reasonably sized for testing rotation policies", i);
        let field_name = b"MESSAGE";
        let field_value = message.as_bytes();

        let items_refs: Vec<&[u8]> = vec![field_name, field_value];

        journal.write_entry(&items_refs)?;

        if i % 10 == 0 {
            println!("  Written {} entries", i + 1);
        }
    }

    Ok(())
}
