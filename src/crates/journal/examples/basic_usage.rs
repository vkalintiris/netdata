#![allow(dead_code, unused_imports)]

use journal::file::JournalFile;
use journal::file::Mmap;
use journal::index::{FieldName, FieldValuePair, FileIndex, FileIndexer};
use journal::repository::File;
use rand::Rng;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;
use tracing::{info, warn};

fn sequential(
    files: &[journal::repository::File],
    field_names: &[FieldName],
) -> Vec<(journal::repository::File, FileIndex)> {
    let start_time = Instant::now();

    let mut total_index_size = 0;

    let mut file_indexes = Vec::new();

    let mut file_indexer = FileIndexer::default();

    for file in files {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(file, window_size).unwrap();

        let source_timestamp_field = Some(&FieldName::new_unchecked("_SOURCE_REALTIME_TIMESTAMP"));

        let Ok(jfi) = file_indexer.index(&journal_file, source_timestamp_field, field_names, 10)
        else {
            continue;
        };

        let mut index_size = 0;
        for (data_payload, entry_indices) in jfi.bitmaps().iter() {
            index_size += data_payload.as_str().len() + entry_indices.serialized_size();
        }

        let path = file.path();
        info!(path, index_size);

        total_index_size += index_size;

        file_indexes.push((file.clone(), jfi));
    }

    // Count midx_count after parallel processing
    let midx_count: usize = file_indexes.iter().map(|fi| fi.1.histogram().len()).sum();

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} histogram index buckets in {:#?} msec",
        midx_count,
        elapsed.as_millis(),
    );
    info!(
        "total index size: {:#?} MiB",
        total_index_size / (1024 * 1024)
    );

    file_indexes
}

use rayon::prelude::*;

fn parallel(
    files: &[journal::repository::File],
    field_names: &[FieldName],
) -> Vec<(journal::repository::File, FileIndex)> {
    let start_time = Instant::now();

    // Process files in parallel
    let file_indexes: Vec<_> = files
        .par_iter() // Create parallel iterator
        .filter_map(|file| {
            // Each thread gets its own FileIndexer
            let mut file_indexer = FileIndexer::default();
            let window_size = 64 * 1024 * 1024;

            let journal_file = JournalFile::<Mmap>::open(file, window_size).ok()?;

            let source_timestamp_field =
                Some(&FieldName::new_unchecked("_SOURCE_REALTIME_TIMESTAMP"));

            let jfi = file_indexer
                .index(&journal_file, source_timestamp_field, field_names, 60)
                .ok()?;

            let mut index_size = 0;
            for (data_payload, entry_indices) in jfi.bitmaps().iter() {
                index_size += data_payload.as_str().len() + entry_indices.serialized_size();
            }

            let path = file.path();
            info!(path, index_size);

            Some((file, jfi, index_size))
        })
        .collect();

    // Calculate totals after parallel processing
    let total_index_size: usize = file_indexes.iter().map(|(_, _, size)| size).sum();

    let midx_count: usize = file_indexes
        .iter()
        .map(|(_, fi, _)| fi.histogram().len())
        .sum();

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} histogram index buckets in {:#?} msec",
        midx_count,
        elapsed.as_millis(),
    );
    info!(
        "total index size: {:#?} MiB",
        total_index_size / (1024 * 1024)
    );

    // Return without the size component
    file_indexes
        .into_iter()
        .map(|(path, index, _)| (path.clone(), index))
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _facets: Vec<&[u8]> = vec![
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

    let facets: Vec<FieldName> = vec![
        "_HOSTNAME",
        "PRIORITY",
        "SYSLOG_FACILITY",
        "ERRNO",
        "SYSLOG_IDENTIFIER",
        "UNIT",
        "USER_UNIT",
        "MESSAGE_ID",
        "_BOOT_ID",
        "_SYSTEMD_OWNER_UID",
        "_UID",
        "OBJECT_SYSTEMD_OWNER_UID",
        "OBJECT_UID",
        "_GID",
        "OBJECT_GID",
        "_CAP_EFFECTIVE",
        "_AUDIT_LOGINUID",
        "OBJECT_AUDIT_LOGINUID",
        "CODE_FUNC",
        "ND_LOG_SOURCE",
        "CODE_FILE",
        "ND_ALERT_NAME",
        "ND_ALERT_CLASS",
        "_SELINUX_CONTEXT",
        "_MACHINE_ID",
        "ND_ALERT_TYPE",
        "_SYSTEMD_SLICE",
        "_EXE",
        "_SYSTEMD_UNIT",
        "_NAMESPACE",
        "_TRANSPORT",
        "_RUNTIME_SCOPE",
        "_STREAM_ID",
        "ND_NIDL_CONTEXT",
        "ND_ALERT_STATUS",
        "_SYSTEMD_CGROUP",
        "ND_NIDL_NODE",
        "ND_ALERT_COMPONENT",
        "_COMM",
        "_SYSTEMD_USER_UNIT",
        "_SYSTEMD_USER_SLICE",
        "_SYSTEMD_SESSION",
        "__logs_sources",
    ]
    .into_iter()
    .map(|s| FieldName::new_unchecked(s))
    .collect();

    let monitor = journal::monitor::Monitor::new().unwrap();
    let mut registry = journal::registry::Registry::new(monitor);
    info!("Journal registry initialized");

    let dirs = ["/home/vk/repos/tmp/agent-events-journal"];
    // let dirs = ["/var/log/journal"];

    for dir in dirs {
        match registry.watch_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    let files = registry.find_files_in_range(0, u32::MAX);
    let mut files: Vec<File> = files.iter().cloned().collect::<_>();
    files.sort_by_key(|f| String::from(f.path()));
    files.reverse();

    // let _ = sequential(&files, &facets);
    let start = std::time::Instant::now();
    let v = parallel(&files, &facets);
    let elapsed = start.elapsed();

    let mut total_size = 0;
    let mut total_entries_index_size = 0;
    let mut total_entries = 0;

    let priority_field = FieldName::new_unchecked("PRIORITY");
    let mut priority_keys = Vec::new();

    for i in 0..10 {
        let key = priority_field.with_value(&i.to_string());
        priority_keys.push(key);
    }

    let mut unique_total = 0;
    for (_f, fi) in v {
        unique_total += allocative::size_of_unique(&fi);
        total_entries += fi.histogram().count();
        total_size += fi.memory_size();
        total_entries_index_size += fi.compress_entries_index().len();
    }

    println!("Total files: {}", files.len());
    println!("Total memory size: {:#?} MiB", total_size / (1024 * 1024));
    println!(
        "Total allocative size: {:#?} MiB",
        unique_total / (1024 * 1024)
    );
    println!(
        "Compressed size: {:#?} MiB",
        total_entries_index_size / (1024 * 1024)
    );
    println!("Total entries: {:#?}", total_entries);

    println!("Building took: {:#?} msec", elapsed.as_millis());
    println!("GiB/sec: {:#?}", 45000.0 / elapsed.as_millis() as f64);

    std::thread::sleep(std::time::Duration::from_secs(3600));

    Ok(())
}
