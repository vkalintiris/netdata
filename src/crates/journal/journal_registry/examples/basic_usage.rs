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

#[derive(Allocative, Debug, Clone)]
struct FileIndex {
    histogram_index: HistogramIndex,
    lz4_roaring_indexes: HashMap<String, Vec<u8>>,
}

fn get_matching_indices(
    entry_offsets: &Vec<NonZeroU64>,
    data_offsets: &Vec<NonZeroU64>,
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

fn compute_delta(indices: &[u32]) -> Vec<u32> {
    if indices.is_empty() {
        return Vec::new();
    }

    std::iter::once(indices[0])
        .chain(indices.windows(2).map(|pair| pair[1] - pair[0]))
        .collect()
}

fn parallel(files: &[journal_registry::RegistryFile]) -> Vec<FileIndex> {
    use rayon::prelude::*;
    let start_time = Instant::now();

    // Use par_iter() instead of iter() and collect results in parallel
    let file_indexes: Vec<FileIndex> = files
        .par_iter()
        .rev()
        .filter_map(|file| {
            println!("File: {:#?}", file.path);

            let window_size = 8 * 1024 * 1024;
            let journal_file = JournalFile::<Mmap>::open(&file.path, window_size).ok()?;

            let histogram_index = HistogramIndex::from(&journal_file).ok()??;

            let mut lz4_roaring_indexes = HashMap::new();

            let entry_offsets = {
                let mut entry_offsets = Vec::new();
                let entry_list = journal_file.entry_list().unwrap();
                entry_list
                    .collect_offsets(&journal_file, &mut entry_offsets)
                    .ok()?;
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
                let field_data_iterator = journal_file.field_data_objects(f.as_bytes()).ok()?;

                for item in field_data_iterator {
                    data_offsets.clear();
                    data_indices.clear();

                    let name = {
                        let item = item.ok()?;

                        let ic = item.inlined_cursor()?;
                        let name = String::from_utf8_lossy(item.payload_bytes()).into_owned();
                        drop(item);

                        ic.collect_offsets(&journal_file, &mut data_offsets).ok()?;
                        name
                    };

                    get_matching_indices(&entry_offsets, &data_offsets, &mut data_indices);

                    let mut roffsets =
                        RoaringBitmap::from_sorted_iter(data_indices.iter().copied()).ok()?;
                    roffsets.optimize();
                    let mut serialized = Vec::new();
                    roffsets.serialize_into(&mut serialized).ok()?;

                    let compressed_roaring =
                        lz4::block::compress(&serialized[..], None, false).ok()?;
                    lz4_roaring_indexes.insert(name.clone(), compressed_roaring);
                }
            }

            Some(FileIndex {
                histogram_index,
                lz4_roaring_indexes,
            })
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
