use journal_log::{JournalLog, JournalLogConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let test_dir = "/home/cm/repos/nd/otel-plugin/src/crates/jf/journal_log_dir/";
    let config = JournalLogConfig::new(test_dir)
        .with_rotation_max_file_size(128 * 1024)
        .with_rotation_max_duration(128 * 1024)
        .with_retention_max_size(1024 * 1024)
        .with_retention_max_duration(24 * 3600)
        .with_retention_max_files(10000);

    println!("Configuration: {:#?}", config);

    let mut journal = JournalLog::new(config)?;
    println!("\nWriting log entries to trigger rotation...");

    for i in 0..100000 {
        let message = format!("LOG_ENTRY_{:03}: This is a test log entry with some data to make it reasonably sized for testing rotation policies", i);
        let field_name = b"MESSAGE";
        let field_value = message.as_bytes();

        let items_refs: Vec<&[u8]> = vec![field_name, field_value];

        journal.write_entry(&items_refs)?;

        if i % 10000 == 0 {
            println!("  Written {} entries", i + 1);
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }

    Ok(())
}
