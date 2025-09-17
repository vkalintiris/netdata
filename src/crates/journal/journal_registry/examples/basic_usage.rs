use allocative::{Allocative, FlameGraphBuilder};
use journal_file::JournalFile;
use journal_file::Mmap;
use journal_file::histogram::HistogramIndex;
use journal_registry::JournalRegistry;
use roaring::RoaringBitmap;
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::time::Instant;
use tracing::{info, instrument, warn};

use bitvec::prelude::*;

pub struct GorillaEncoder {
    buffer: BitVec<u8, bitvec::order::Msb0>,
    prev_value: Option<u32>,
    prev_leading_zeros: u32,
    prev_trailing_zeros: u32,
}

impl GorillaEncoder {
    pub fn new() -> Self {
        Self {
            buffer: BitVec::new(),
            prev_value: None,
            prev_leading_zeros: 0,
            prev_trailing_zeros: 0,
        }
    }

    pub fn encode(&mut self, values: &[u32]) -> Result<(), Box<dyn std::error::Error>> {
        for &value in values {
            self.encode_value(value)?;
        }
        Ok(())
    }

    pub fn encode_value(&mut self, value: u32) -> Result<(), Box<dyn std::error::Error>> {
        match self.prev_value {
            None => {
                self.encode_first_value(value);
            }
            Some(prev) => {
                self.encode_delta_value(prev, value);
            }
        }
        self.prev_value = Some(value);
        Ok(())
    }

    pub fn encode_iter<I>(&mut self, values: I) -> Result<(), Box<dyn std::error::Error>>
    where
        I: IntoIterator<Item = u32>,
    {
        for value in values {
            self.encode_value(value)?;
        }
        Ok(())
    }

    fn encode_first_value(&mut self, value: u32) {
        for i in (0..32).rev() {
            self.buffer.push((value >> i) & 1 != 0);
        }
    }

    fn encode_delta_value(&mut self, prev_value: u32, current_value: u32) {
        let xor = prev_value ^ current_value;

        if xor == 0 {
            self.buffer.push(false);
            return;
        }

        self.buffer.push(true);

        let leading_zeros = xor.leading_zeros();
        let trailing_zeros = xor.trailing_zeros();

        if leading_zeros >= self.prev_leading_zeros && trailing_zeros >= self.prev_trailing_zeros {
            self.buffer.push(false);

            let meaningful_start = self.prev_leading_zeros;
            let meaningful_end = 32 - self.prev_trailing_zeros;
            let meaningful_bits = meaningful_end - meaningful_start;

            let shifted_xor = xor >> self.prev_trailing_zeros;
            for i in (0..meaningful_bits).rev() {
                self.buffer.push((shifted_xor >> i) & 1 != 0);
            }
        } else {
            self.buffer.push(true);

            for i in (0..5).rev() {
                self.buffer.push((leading_zeros >> i) & 1 != 0);
            }

            for i in (0..5).rev() {
                self.buffer.push((trailing_zeros >> i) & 1 != 0);
            }

            self.prev_leading_zeros = leading_zeros;
            self.prev_trailing_zeros = trailing_zeros;

            let meaningful_bits = 32 - leading_zeros - trailing_zeros;
            let meaningful_value = xor >> trailing_zeros;
            for i in (0..meaningful_bits).rev() {
                self.buffer.push((meaningful_value >> i) & 1 != 0);
            }
        }
    }

    pub fn finish(self) -> Vec<u8> {
        let mut result = vec![0u8; (self.buffer.len() + 7) / 8];
        for (i, bit) in self.buffer.iter().enumerate() {
            if *bit {
                result[i / 8] |= 1 << (7 - (i % 8));
            }
        }
        result
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

impl Default for GorillaEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Allocative, Debug, Clone)]
struct FileIndex {
    histogram_index: HistogramIndex,
    data_indexes: HashMap<String, Vec<u32>>,
    roaring_indexes: HashMap<String, Vec<u8>>,
    gorilla_indexes: HashMap<String, Vec<u8>>,
    lz4_data_indexes: HashMap<String, Vec<u8>>,
    lz4_roaring_indexes: HashMap<String, Vec<u8>>,
    lz4_gorilla_indexes: HashMap<String, Vec<u8>>,
}

fn get_matching_indices(
    entry_offsets: &Vec<NonZeroU64>,
    data_offsets: &Vec<NonZeroU64>,
) -> Vec<u32> {
    let mut indices = Vec::new();
    let mut data_iter = data_offsets.iter();
    let mut current_data = data_iter.next();

    for (i, entry) in entry_offsets.iter().enumerate() {
        if let Some(data) = current_data {
            if entry == data {
                indices.push(i as u32);
                current_data = data_iter.next();
            }
        } else {
            break; // No more data_offsets to match
        }
    }

    indices
}

fn compute_delta(indices: &[u32]) -> Vec<u32> {
    if indices.is_empty() {
        return Vec::new();
    }

    std::iter::once(indices[0])
        .chain(indices.windows(2).map(|pair| pair[1] - pair[0]))
        .collect()
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

        let mut data_indexes = HashMap::new();
        let mut roaring_indexes = HashMap::new();
        let mut gorilla_indexes = HashMap::new();
        let mut lz4_data_indexes = HashMap::new();
        let mut lz4_roaring_indexes = HashMap::new();
        let mut lz4_gorilla_indexes = HashMap::new();
        let mut data_offsets = Vec::new();

        let entry_offsets = {
            let mut entry_offsets = Vec::new();
            let entry_list = journal_file.entry_list().unwrap();
            entry_list
                .collect_offsets(&journal_file, &mut entry_offsets)
                .unwrap();
            entry_offsets
        };

        let v = vec!["PRIORITY"];
        for f in v {
            let field_data_iterator = journal_file.field_data_objects(f.as_bytes()).unwrap();

            for item in field_data_iterator {
                data_offsets.clear();

                let name = {
                    let item = item.unwrap();

                    let ic = item.inlined_cursor().unwrap();
                    let name = String::from_utf8_lossy(item.payload_bytes()).into_owned();
                    drop(item);

                    ic.collect_offsets(&journal_file, &mut data_offsets)
                        .unwrap();
                    name
                };

                let offsets = get_matching_indices(&entry_offsets, &data_offsets);
                let offsets = compute_delta(&offsets);

                let mut gb = GorillaEncoder::new();
                gb.encode_iter(offsets.clone()).unwrap();
                let gorilla_data = gb.finish();
                gorilla_indexes.insert(name.clone(), gorilla_data.clone());

                // Compress raw data indexes with LZ4
                let raw_data_bytes: Vec<u8> = offsets
                    .iter()
                    .flat_map(|offset| offset.to_le_bytes())
                    .collect();
                let compressed_raw = lz4::block::compress(&raw_data_bytes[..], None, false)
                    .map_err(|e| format!("LZ4 compression failed: {}", e))
                    .unwrap();
                lz4_data_indexes.insert(name.clone(), compressed_raw);

                data_indexes.insert(name.clone(), offsets.clone());
                // let mut roffsets = RoaringBitmap::from_sorted_iter(offsets.iter().map(|x| *x)).unwrap();
                let mut roffsets = RoaringBitmap::new();
                for df in offsets.iter() {
                    roffsets.insert(*df);
                }
                roffsets.optimize();
                let mut serialized = Vec::new();
                roffsets.serialize_into(&mut serialized).unwrap();

                // Compress roaring bitmap data with LZ4
                let compressed_roaring = lz4::block::compress(&serialized[..], None, false)
                    .map_err(|e| format!("LZ4 compression failed: {}", e))
                    .unwrap();
                lz4_roaring_indexes.insert(name.clone(), compressed_roaring);

                roaring_indexes.insert(name.clone(), serialized);

                // Compress gorilla encoded data with LZ4
                let compressed_gorilla = lz4::block::compress(&gorilla_data[..], None, false)
                    .map_err(|e| format!("LZ4 compression failed: {}", e))
                    .unwrap();
                lz4_gorilla_indexes.insert(name, compressed_gorilla);
            }
        }

        midx_count += histogram_index.len();

        file_indexes.push(FileIndex {
            histogram_index,
            data_indexes,
            roaring_indexes,
            gorilla_indexes,
            lz4_data_indexes,
            lz4_roaring_indexes,
            lz4_gorilla_indexes,
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

    let v = sequential(&files);

    let mut flamegraph = FlameGraphBuilder::default();
    flamegraph.visit_root(&v);
    let flamegraph_src = flamegraph.finish().flamegraph().write();
    std::fs::write("/tmp/flamegraph.txt", flamegraph_src).unwrap();

    // Calculate and report compression ratios
    println!("\n=== Compression Ratios ===");
    let mut total_raw_size = 0usize;
    let mut total_gorilla_size = 0usize;
    let mut total_roaring_size = 0usize;
    let mut total_lz4_raw_size = 0usize;
    let mut total_lz4_gorilla_size = 0usize;
    let mut total_lz4_roaring_size = 0usize;

    for file_index in &v {
        for (name, raw_data) in &file_index.data_indexes {
            let raw_size = raw_data.len() * std::mem::size_of::<NonZeroU64>();
            total_raw_size += raw_size;

            if let Some(lz4_compressed) = file_index.lz4_data_indexes.get(name) {
                total_lz4_raw_size += lz4_compressed.len();
            }
        }

        for (name, gorilla_data) in &file_index.gorilla_indexes {
            total_gorilla_size += gorilla_data.len();

            if let Some(lz4_compressed) = file_index.lz4_gorilla_indexes.get(name) {
                total_lz4_gorilla_size += lz4_compressed.len();
            }
        }

        for (name, roaring_data) in &file_index.roaring_indexes {
            total_roaring_size += roaring_data.len();

            if let Some(lz4_compressed) = file_index.lz4_roaring_indexes.get(name) {
                total_lz4_roaring_size += lz4_compressed.len();
            }
        }
    }

    println!("Raw data:");
    println!("  Original size: {} bytes", total_raw_size);
    println!("  LZ4 compressed: {} bytes", total_lz4_raw_size);
    println!(
        "  Compression ratio: {:.2}:1",
        total_raw_size as f64 / total_lz4_raw_size as f64
    );
    println!(
        "  Space saved: {:.1}%",
        (1.0 - total_lz4_raw_size as f64 / total_raw_size as f64) * 100.0
    );

    println!("\nGorilla encoded data:");
    println!("  Original size: {} bytes", total_gorilla_size);
    println!("  LZ4 compressed: {} bytes", total_lz4_gorilla_size);
    println!(
        "  Compression ratio: {:.2}:1",
        total_gorilla_size as f64 / total_lz4_gorilla_size as f64
    );
    println!(
        "  Space saved: {:.1}%",
        (1.0 - total_lz4_gorilla_size as f64 / total_gorilla_size as f64) * 100.0
    );

    println!("\nRoaring bitmap data:");
    println!("  Original size: {} bytes", total_roaring_size);
    println!("  LZ4 compressed: {} bytes", total_lz4_roaring_size);
    println!(
        "  Compression ratio: {:.2}:1",
        total_roaring_size as f64 / total_lz4_roaring_size as f64
    );
    println!(
        "  Space saved: {:.1}%",
        (1.0 - total_lz4_roaring_size as f64 / total_roaring_size as f64) * 100.0
    );

    println!("\n=== Journal Files Statistics ===");
    println!("Total files: {}", registry.query().count());
    println!(
        "Total size: {:.2} MB",
        registry.query().total_size() as f64 / (1024.0 * 1024.0)
    );

    Ok(())
}
