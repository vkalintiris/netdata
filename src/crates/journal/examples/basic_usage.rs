#![allow(dead_code, unused_imports)]

use journal::file::JournalFile;
use journal::file::Mmap;
use journal::index::{FileIndex, FileIndexer};
use journal::repository::File;
use rand::Rng;
use std::collections::VecDeque;
use std::time::Duration;
use std::time::Instant;
use tracing::{info, warn};

fn sequential(
    files: &[journal::repository::File],
    field_names: &[&[u8]],
) -> Vec<(journal::repository::File, FileIndex)> {
    let start_time = Instant::now();

    let mut total_index_size = 0;

    let mut file_indexes = Vec::new();

    let mut file_indexer = FileIndexer::default();
    const SOURCE_TIMESTAMP_FIELD: &[u8] = b"_SOURCE_REALTIME_TIMESTAMP";

    for file in files {
        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(file.path(), window_size).unwrap();

        let Ok(jfi) = file_indexer.index(&journal_file, None, field_names, 10) else {
            continue;
        };

        let mut index_size = 0;
        for (data_payload, entry_indices) in jfi.bitmaps().iter() {
            index_size += data_payload.len() + entry_indices.serialized_size();
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
    field_names: &[&[u8]],
) -> Vec<(journal::repository::File, FileIndex)> {
    let start_time = Instant::now();

    const SOURCE_TIMESTAMP_FIELD: Option<&[u8]> = Some(b"_SOURCE_REALTIME_TIMESTAMP");

    // Process files in parallel
    let file_indexes: Vec<_> = files
        .par_iter() // Create parallel iterator
        .filter_map(|file| {
            // Each thread gets its own FileIndexer
            let mut file_indexer = FileIndexer::default();
            let window_size = 64 * 1024 * 1024;

            let journal_file = JournalFile::<Mmap>::open(file.path(), window_size).ok()?;

            let jfi = file_indexer
                .index(&journal_file, SOURCE_TIMESTAMP_FIELD, field_names, 3600)
                .ok()?;

            let mut index_size = 0;
            for (data_payload, entry_indices) in jfi.bitmaps().iter() {
                index_size += data_payload.len() + entry_indices.serialized_size();
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

    let facets: Vec<&[u8]> = vec![
        b"_HOSTNAME",
        b"PRIORITY",
        b"SYSLOG_FACILITY",
        b"ERRNO",
        b"SYSLOG_IDENTIFIER",
        b"UNIT",
        b"USER_UNIT",
        b"MESSAGE_ID",
        b"_BOOT_ID",
        b"_SYSTEMD_OWNER_UID",
        b"_UID",
        b"OBJECT_SYSTEMD_OWNER_UID",
        b"OBJECT_UID",
        b"_GID",
        b"OBJECT_GID",
        b"_CAP_EFFECTIVE",
        b"_AUDIT_LOGINUID",
        b"OBJECT_AUDIT_LOGINUID",
        b"CODE_FUNC",
        b"ND_LOG_SOURCE",
        b"CODE_FILE",
        b"ND_ALERT_NAME",
        b"ND_ALERT_CLASS",
        b"_SELINUX_CONTEXT",
        b"_MACHINE_ID",
        b"ND_ALERT_TYPE",
        b"_SYSTEMD_SLICE",
        b"_EXE",
        b"_SYSTEMD_UNIT",
        b"_NAMESPACE",
        b"_TRANSPORT",
        b"_RUNTIME_SCOPE",
        b"_STREAM_ID",
        b"ND_NIDL_CONTEXT",
        b"ND_ALERT_STATUS",
        b"_SYSTEMD_CGROUP",
        b"ND_NIDL_NODE",
        b"ND_ALERT_COMPONENT",
        b"_COMM",
        b"_SYSTEMD_USER_UNIT",
        b"_SYSTEMD_USER_SLICE",
        b"_SYSTEMD_SESSION",
        b"__logs_sources",
    ];
    // let facets: Vec<&[u8]> = vec![b"log.severity_number"];

    // Initialize tracing
    // tracing_subscriber::fmt()
    //     .with_max_level(tracing::Level::INFO)
    //     .init();

    let mut registry = journal::registry::Registry::new()?;
    info!("Journal registry initialized");

    for dir in ["/home/vk/repos/tmp/agent-events-journal"] {
        match registry.watch_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    let mut files = VecDeque::new();
    registry.find_files_in_range(0, u64::MAX, &mut files);
    let mut files: Vec<File> = files.into();
    files.sort_by_key(|f| String::from(f.path()));
    files.reverse();

    // for file in files.iter() {
    //     println!("file: {:#?}", file.path);
    // }
    // files.truncate(5);

    // let _ = sequential(&files, facets.as_slice());
    let start = std::time::Instant::now();
    let v = parallel(&files, facets.as_slice());
    let elapsed = start.elapsed();

    let mut total_size = 0;
    let mut total_entries_index_size = 0;
    let mut total_entries = 0;
    let mut priority_count = 0;

    let mut priority_keys = Vec::new();

    for i in 0..10 {
        let key = format!("PRIORITY={}", i);
        priority_keys.push(key);
    }

    for (f, fi) in v {
        println!("Histogram for {}", f.path());
        println!("{}", fi.histogram());

        for k in &priority_keys {
            if let Some(b) = fi.bitmaps().get(k) {
                priority_count += b.len();
            }
        }

        total_entries += fi.histogram().count();
        total_size += fi.memory_size();
        total_entries_index_size += fi.compress_entries_index().len();
    }

    println!("Total files: {}", files.len());
    println!("Total size: {:#?} MiB", total_size / (1024 * 1024));
    println!(
        "Compressed size: {:#?} MiB",
        total_entries_index_size / (1024 * 1024)
    );
    println!("Total entries: {:#?}", total_entries);
    println!("Priority count: {:#?}", priority_count);

    println!("Building took: {:#?} msec", elapsed.as_millis());
    println!("GiB/sec: {:#?}", 9400.0 / elapsed.as_millis() as f64);

    std::thread::sleep(std::time::Duration::from_secs(3600));

    Ok(())
}
