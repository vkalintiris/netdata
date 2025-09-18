use allocative::{Allocative, FlameGraphBuilder};
use journal_file::JournalFile;
use journal_file::Mmap;
use journal_file::histogram::HistogramIndex;
use journal_registry::JournalRegistry;
use roaring::RoaringBitmap;
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::Duration;
use std::time::Instant;
use tracing::{info, instrument, warn};

fn get_matching_indices(
    entry_offsets: &[NonZeroU64],
    data_offsets: &[NonZeroU64],
    data_indices: &mut Vec<u32>,
) {
    let mut data_iter = data_offsets.iter();
    let mut current_data = data_iter.next();

    for (i, entry) in entry_offsets.iter().enumerate() {
        if let Some(data) = current_data {
            if entry == data {
                data_indices.push(i as u32);
                current_data = data_iter.next();
            }
        } else {
            break; // No more data_offsets to match
        }
    }
}

#[derive(Allocative, Debug, Clone)]
struct FileIndex {
    histogram_index: HistogramIndex,
    lz4_roaring_indexes: HashMap<String, Vec<u8>>,
}

impl FileIndex {
    fn from(
        journal_file: &JournalFile<Mmap>,
        field_names: &[&[u8]],
    ) -> Result<Option<FileIndex>, Box<dyn std::error::Error>> {
        let mut lz4_roaring_indexes = HashMap::new();

        let entry_offsets = {
            let mut entry_offsets = Vec::new();
            let Some(entry_list) = journal_file.entry_list() else {
                return Ok(None);
            };
            entry_list
                .collect_offsets(journal_file, &mut entry_offsets)
                .unwrap();
            entry_offsets
        };

        let Some(histogram_index) = HistogramIndex::from(journal_file)? else {
            return Ok(None);
        };

        let mut data_offsets = Vec::new();
        let mut data_indices = Vec::new();

        for field_name in field_names {
            let field_data_iterator = journal_file.field_data_objects(field_name)?;

            for data_object in field_data_iterator {
                data_offsets.clear();
                data_indices.clear();

                let name = {
                    let data_object = data_object.unwrap();

                    let Some(ic) = data_object.inlined_cursor() else {
                        continue;
                    };
                    let name = String::from_utf8_lossy(data_object.payload_bytes()).into_owned();
                    drop(data_object);

                    ic.collect_offsets(journal_file, &mut data_offsets).unwrap();
                    name
                };

                get_matching_indices(&entry_offsets, &data_offsets, &mut data_indices);

                let mut roffsets =
                    RoaringBitmap::from_sorted_iter(data_indices.iter().copied()).unwrap();
                roffsets.optimize();
                let mut serialized = Vec::new();
                roffsets.serialize_into(&mut serialized).unwrap();

                // Compress roaring bitmap data with LZ4
                let compressed_roaring = lz4::block::compress(&serialized[..], None, false)
                    .map_err(|e| format!("LZ4 compression failed: {}", e))
                    .unwrap();
                lz4_roaring_indexes.insert(name.clone(), compressed_roaring);
            }
        }

        Ok(Some(FileIndex {
            histogram_index,
            lz4_roaring_indexes,
        }))
    }
}

fn parallel(files: &[journal_registry::RegistryFile]) -> Vec<FileIndex> {
    use rayon::prelude::*;
    let start_time = Instant::now();

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

    // Use par_iter() instead of iter() and collect results in parallel
    let file_indexes: Vec<FileIndex> = files
        .par_iter()
        .rev()
        .filter_map(|file| {
            println!("File: {:#?}", file.path);

            let window_size = 8 * 1024 * 1024;
            let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).ok()?;

            FileIndex::from(&journal_file, systemd_keys.as_slice()).unwrap()
        })
        .collect();

    // Count midx_count after parallel processing
    let midx_count: usize = file_indexes.iter().map(|fi| fi.histogram_index.len()).sum();

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} histogram index buckets in {:#?} msec",
        midx_count,
        elapsed.as_millis(),
    );

    file_indexes
}

#[instrument(skip(files))]
fn sequential(files: &[journal_registry::RegistryFile]) -> Vec<FileIndex> {
    let start_time = Instant::now();

    let mut midx_count = 0;

    let mut file_indexes = Vec::with_capacity(files.len());

    #[allow(clippy::never_loop)]
    for file in files.iter().rev() {
        println!("File: {:#?}", file.path);

        let window_size = 8 * 1024 * 1024;
        let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).unwrap();

        let Some(histogram_index) = HistogramIndex::from(&journal_file).unwrap() else {
            continue;
        };

        let mut lz4_roaring_indexes = HashMap::new();

        let entry_offsets = {
            let mut entry_offsets = Vec::new();
            let entry_list = journal_file.entry_list().unwrap();
            entry_list
                .collect_offsets(&journal_file, &mut entry_offsets)
                .unwrap();
            entry_offsets
        };

        let systemd_keys: Vec<&str> = vec![
            // --- USER JOURNAL FIELDS ---
            "MESSAGE_ID",
            "PRIORITY",
            "CODE_FILE",
            "CODE_FUNC",
            "ERRNO",
            "SYSLOG_FACILITY",
            "SYSLOG_IDENTIFIER",
            "UNIT",
            "USER_UNIT",
            "UNIT_RESULT",
            // --- TRUSTED JOURNAL FIELDS ---
            "_UID",
            "_GID",
            "_COMM",
            "_EXE",
            "_CAP_EFFECTIVE",
            "_AUDIT_LOGINUID",
            "_SYSTEMD_CGROUP",
            "_SYSTEMD_SLICE",
            "_SYSTEMD_UNIT",
            "_SYSTEMD_USER_UNIT",
            "_SYSTEMD_USER_SLICE",
            "_SYSTEMD_SESSION",
            "_SYSTEMD_OWNER_UID",
            "_SELINUX_CONTEXT",
            "_BOOT_ID",
            "_MACHINE_ID",
            "_HOSTNAME",
            "_TRANSPORT",
            "_STREAM_ID",
            "_NAMESPACE",
            "_RUNTIME_SCOPE",
            // --- KERNEL JOURNAL FIELDS ---
            "_KERNEL_SUBSYSTEM",
            "_UDEV_DEVNODE",
            // --- LOGGING ON BEHALF ---
            "OBJECT_UID",
            "OBJECT_GID",
            "OBJECT_COMM",
            "OBJECT_EXE",
            "OBJECT_AUDIT_LOGINUID",
            "OBJECT_SYSTEMD_CGROUP",
            "OBJECT_SYSTEMD_SESSION",
            "OBJECT_SYSTEMD_OWNER_UID",
            "OBJECT_SYSTEMD_UNIT",
            "OBJECT_SYSTEMD_USER_UNIT",
            // --- CORE DUMPS ---
            "COREDUMP_COMM",
            "COREDUMP_UNIT",
            "COREDUMP_USER_UNIT",
            "COREDUMP_SIGNAL_NAME",
            "COREDUMP_CGROUP",
            // --- DOCKER ---
            "CONTAINER_ID",
            "CONTAINER_NAME",
            "CONTAINER_TAG",
            "IMAGE_NAME",
            // --- NETDATA ---
            "ND_NIDL_NODE",
            "ND_NIDL_CONTEXT",
            "ND_LOG_SOURCE",
            "ND_ALERT_NAME",
            "ND_ALERT_CLASS",
            "ND_ALERT_COMPONENT",
            "ND_ALERT_TYPE",
            "ND_ALERT_STATUS",
        ];

        let mut data_offsets = Vec::new();
        let mut data_indices = Vec::new();

        for f in systemd_keys {
            let field_data_iterator = journal_file.field_data_objects(f.as_bytes()).unwrap();

            for item in field_data_iterator {
                data_offsets.clear();
                data_indices.clear();

                let name = {
                    let item = item.unwrap();

                    let Some(ic) = item.inlined_cursor() else {
                        continue;
                    };
                    let name = String::from_utf8_lossy(item.payload_bytes()).into_owned();
                    drop(item);

                    ic.collect_offsets(&journal_file, &mut data_offsets)
                        .unwrap();
                    name
                };

                get_matching_indices(&entry_offsets, &data_offsets, &mut data_indices);

                let mut roffsets =
                    RoaringBitmap::from_sorted_iter(data_indices.iter().copied()).unwrap();
                roffsets.optimize();
                let mut serialized = Vec::new();
                roffsets.serialize_into(&mut serialized).unwrap();

                // Compress roaring bitmap data with LZ4
                let compressed_roaring = lz4::block::compress(&serialized[..], None, false)
                    .map_err(|e| format!("LZ4 compression failed: {}", e))
                    .unwrap();
                lz4_roaring_indexes.insert(name.clone(), compressed_roaring);
            }
        }

        midx_count += histogram_index.len();

        file_indexes.push(FileIndex {
            histogram_index,
            lz4_roaring_indexes,
        });
    }

    let elapsed = start_time.elapsed();
    info!(
        "{:#?} histogram index buckets in {:#?} msec",
        midx_count,
        elapsed.as_millis(),
    );

    file_indexes
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let registry = JournalRegistry::new()?;
    info!("Journal registry initialized");

    for dir in ["/var/log/journal", "/run/log/journal"] {
        match registry.add_directory(dir) {
            Ok(_) => info!("Added directory: {}", dir),
            Err(e) => warn!("Failed to add directory {}: {}", dir, e),
        }
    }

    let mut files = registry.query().execute();
    files.sort_by_key(|x| x.path.clone());
    files.sort_by_key(|x| x.size);
    files.reverse();
    // files.truncate(5);

    let v = parallel(&files);

    let mut flamegraph = FlameGraphBuilder::default();
    flamegraph.visit_root(&v);
    let flamegraph_src = flamegraph.finish().flamegraph().write();
    std::fs::write("/tmp/flamegraph.txt", flamegraph_src).unwrap();

    // Calculate and report compression ratios
    println!("\n=== Compression Ratios ===");
    let mut total_lz4_roaring_size = 0usize;

    for file_index in &v {
        for roaring_data in file_index.lz4_roaring_indexes.values() {
            total_lz4_roaring_size += roaring_data.len();
        }
    }

    println!("\nRoaring bitmap data:");
    println!("  LZ4 compressed: {} bytes", total_lz4_roaring_size);

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    tokio::time::sleep(Duration::from_secs(100)).await;

    Ok(())
}
