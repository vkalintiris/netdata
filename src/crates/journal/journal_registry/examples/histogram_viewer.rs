use chrono::{DateTime, Local, Utc};
use journal_file::index::{FileIndex, FileIndexer};
use journal_file::{JournalFile, Mmap};
use std::env;
use std::num::NonZeroU64;
use tracing::info;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Get filename from command line arguments
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <journal_file_path>", args[0]);
        std::process::exit(1);
    }

    let file_path = &args[1];
    info!("Processing journal file: {}", file_path);

    // Use the same systemd_keys as the basic_usage example
    let systemd_keys: Vec<&[u8]> = vec![
        // --- USER JOURNAL FIELDS ---
        b"MESSAGE_ID",
        b"PRIORITY",
        b"CODE_FILE",
        b"CODE_FUNC",
        b"ERRNO",
        b"SYSLOG_FACILITY",
        b"SYSLOG_IDENTIFIER",
        b"UNIT",
        b"USER_UNIT",
        b"UNIT_RESULT",
        // --- TRUSTED JOURNAL FIELDS ---
        b"_UID",
        b"_GID",
        b"_COMM",
        b"_EXE",
        b"_CAP_EFFECTIVE",
        b"_AUDIT_LOGINUID",
        b"_SYSTEMD_CGROUP",
        b"_SYSTEMD_SLICE",
        b"_SYSTEMD_UNIT",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        b"_SYSTEMD_SESSION",
        b"_SYSTEMD_OWNER_UID",
        b"_SELINUX_CONTEXT",
        b"_BOOT_ID",
        b"_MACHINE_ID",
        b"_HOSTNAME",
        b"_TRANSPORT",
        b"_STREAM_ID",
        b"_NAMESPACE",
        b"_RUNTIME_SCOPE",
        // --- KERNEL JOURNAL FIELDS ---
        b"_KERNEL_SUBSYSTEM",
        b"_UDEV_DEVNODE",
        // --- LOGGING ON BEHALF ---
        b"OBJECT_UID",
        b"OBJECT_GID",
        b"OBJECT_COMM",
        b"OBJECT_EXE",
        b"OBJECT_AUDIT_LOGINUID",
        b"OBJECT_SYSTEMD_CGROUP",
        b"OBJECT_SYSTEMD_SESSION",
        b"OBJECT_SYSTEMD_OWNER_UID",
        b"OBJECT_SYSTEMD_UNIT",
        b"OBJECT_SYSTEMD_USER_UNIT",
        // --- CORE DUMPS ---
        b"COREDUMP_COMM",
        b"COREDUMP_UNIT",
        b"COREDUMP_USER_UNIT",
        b"COREDUMP_SIGNAL_NAME",
        b"COREDUMP_CGROUP",
        // --- DOCKER ---
        b"CONTAINER_ID",
        b"CONTAINER_NAME",
        b"CONTAINER_TAG",
        b"IMAGE_NAME",
        // --- NETDATA ---
        b"ND_NIDL_NODE",
        b"ND_NIDL_CONTEXT",
        b"ND_LOG_SOURCE",
        b"ND_ALERT_NAME",
        b"ND_ALERT_CLASS",
        b"ND_ALERT_COMPONENT",
        b"ND_ALERT_TYPE",
        b"ND_ALERT_STATUS",
    ];

    // Open the journal file
    let window_size = 8 * 1024 * 1024;
    let journal_file = JournalFile::<Mmap>::open(file_path, window_size)?;
    info!("Journal file opened successfully");

    // Create the FileIndex
    let mut file_indexer = FileIndexer::default();
    let file_index = file_indexer.index(
        &journal_file,
        b"_SOURCE_REALTIME_TIMESTAMP",
        systemd_keys.as_slice(),
        60,
    )?;
    info!("FileIndex created successfully");

    // Pretty print the FileHistogram
    print_histogram(&file_index, &journal_file)?;

    Ok(())
}

fn print_histogram(
    file_index: &FileIndex,
    journal_file: &JournalFile<Mmap>,
) -> Result<(), Box<dyn std::error::Error>> {
    let histogram = &file_index.file_histogram;

    println!("\n=== FILE HISTOGRAM ===");
    println!("Total buckets: {}", histogram.len());
    println!("Bucket size: {} seconds", histogram.bucket_size());
    println!("Total entries: {}", histogram.total_entries());

    if let Some((start_time, end_time)) = histogram.time_range() {
        let start_dt = DateTime::from_timestamp_micros(start_time as i64)
            .unwrap_or_else(|| Utc::now())
            .with_timezone(&Local);
        let end_dt = DateTime::from_timestamp_micros(end_time as i64)
            .unwrap_or_else(|| Utc::now())
            .with_timezone(&Local);

        println!(
            "Time range: {} to {}",
            start_dt.format("%Y-%m-%d %H:%M:%S"),
            end_dt.format("%Y-%m-%d %H:%M:%S")
        );
    }

    if let Some(duration) = histogram.duration_seconds() {
        println!(
            "Duration: {} seconds ({:.1} minutes)",
            duration,
            duration as f64 / 60.0
        );
    }

    println!("\n=== BUCKET DETAILS ===");

    let bucket_count = histogram.len();
    let buckets_to_show = if bucket_count <= 5 {
        // Show all buckets if we have 5 or fewer
        (0..bucket_count).collect::<Vec<_>>()
    } else {
        // Show first, last, and 2-3 in between
        let mut buckets = vec![0, bucket_count - 1]; // First and last

        // Add 2-3 buckets in between
        let step = bucket_count / 4;
        if step > 0 {
            buckets.push(step);
            buckets.push(step * 2);
            if bucket_count > 8 {
                buckets.push(step * 3);
            }
        }

        buckets.sort();
        buckets.dedup();
        buckets
    };

    let mut entry_offsets = Vec::new();
    journal_file.entry_offsets(&mut entry_offsets)?;

    for bucket_idx in buckets_to_show {
        print_bucket_info(histogram, bucket_idx, &entry_offsets, journal_file)?;
    }

    Ok(())
}

fn print_bucket_info(
    histogram: &journal_file::index::FileHistogram,
    bucket_idx: usize,
    entry_offsets: &[NonZeroU64],
    journal_file: &JournalFile<Mmap>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some((start_idx, end_idx)) = histogram.get_entry_range(bucket_idx) {
        let entry_count = end_idx - start_idx + 1;

        // Get the bucket time range
        let bucket_start = if bucket_idx == 0 {
            if let Some(offset) = entry_offsets.get(start_idx as usize) {
                let entry = journal_file.entry_ref(*offset)?;
                entry.header.realtime
            } else {
                0
            }
        } else {
            // Calculate bucket time from previous bucket
            if let Some((_, prev_end_idx)) = histogram.get_entry_range(bucket_idx - 1) {
                if let Some(offset) = entry_offsets.get((prev_end_idx + 1) as usize) {
                    let entry = journal_file.entry_ref(*offset)?;
                    entry.header.realtime
                } else {
                    0
                }
            } else {
                0
            }
        };

        let bucket_end = if let Some(offset) = entry_offsets.get(end_idx as usize) {
            let entry = journal_file.entry_ref(*offset)?;
            entry.header.realtime
        } else {
            bucket_start
        };

        // Convert microseconds to datetime
        let start_dt = DateTime::from_timestamp_micros(bucket_start as i64)
            .unwrap_or_else(|| Utc::now())
            .with_timezone(&Local);
        let end_dt = DateTime::from_timestamp_micros(bucket_end as i64)
            .unwrap_or_else(|| Utc::now())
            .with_timezone(&Local);

        println!(
            "Bucket #{}: {} entries ({} - {})",
            bucket_idx,
            entry_count,
            start_dt.format("%Y-%m-%d %H:%M:%S"),
            end_dt.format("%Y-%m-%d %H:%M:%S")
        );

        // Show a couple of sample log entries from this bucket
        let sample_count = 3.min(entry_count as usize);
        println!("  Sample entries:");

        for i in 0..sample_count {
            let entry_idx = start_idx + i as u32;
            if let Some(offset) = entry_offsets.get(entry_idx as usize) {
                if let Ok(entry_obj) = journal_file.entry_ref(*offset) {
                    let timestamp =
                        DateTime::from_timestamp_micros(entry_obj.header.realtime as i64)
                            .unwrap_or_else(|| Utc::now())
                            .with_timezone(&Local);

                    // For now, just show entry timestamp and basic info
                    // TODO: Add MESSAGE field extraction when memory management issues are resolved
                    let message = format!("Entry #{}", entry_idx);

                    println!("    [{}] {}", timestamp.format("%H:%M:%S"), message);
                }
            }
        }

        if entry_count > sample_count as u32 {
            println!(
                "    ... and {} more entries",
                entry_count - sample_count as u32
            );
        }
        println!();
    }

    Ok(())
}
