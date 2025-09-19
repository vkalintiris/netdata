use crate::JournalFile;
use crate::Mmap;
use allocative::Allocative;
use error::Result;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroU64;

/// A minute-aligned bucket in the histogram index.
#[derive(Allocative, Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct FileBucket {
    /// Minute-aligned seconds since EPOCH.
    minute: u64,
    /// Index into the global entry offsets array.
    offset_index: usize,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
///
/// This structure stores minute boundaries and their corresponding offset indices,
/// enabling O(log n) lookups for time ranges and histogram generation with configurable
/// bucket sizes (1-minute, 10-minute, etc.).
#[derive(Allocative, Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileHistogram {
    /// Sparse vector containing only minute boundaries where changes occur.
    buckets: Vec<FileBucket>,
}

impl FileHistogram {
    pub fn from(
        journal_file: &JournalFile<Mmap>,
        entry_offsets: &[NonZeroU64],
    ) -> Result<FileHistogram> {
        let mut buckets = Vec::new();
        let mut current_minute = None;

        for (offset_index, &offset) in entry_offsets.iter().enumerate() {
            let entry = journal_file.entry_ref(offset)?;
            let minute = entry.header.realtime / (60 * 1_000_000);

            match current_minute {
                None => {
                    // First entry
                    debug_assert_eq!(offset_index, 0);

                    buckets.push(FileBucket {
                        minute,
                        offset_index: 0,
                    });
                    current_minute = Some(minute);
                }
                Some(prev_minute) if minute > prev_minute => {
                    // New minute boundary
                    buckets.push(FileBucket {
                        minute,
                        offset_index,
                    });
                    current_minute = Some(minute);
                }
                _ => {} // Same minute, skip
            }
        }

        Ok(FileHistogram { buckets })
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }
}

impl std::fmt::Display for FileHistogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.buckets.is_empty() {
            return write!(f, "Empty index");
        }

        writeln!(f, "Time ranges and entry counts:")?;
        writeln!(f, "{:<30} {:<10}", "Range", "Count")?;
        writeln!(f, "{}", "-".repeat(40))?;

        for window in self.buckets.windows(2) {
            let start_minute = window[0].minute;
            let end_minute = window[1].minute;
            let count = window[1].offset_index - window[0].offset_index;

            writeln!(
                f,
                "{:02}:{:02} - {:02}:{:02} ({}m)          {}",
                (start_minute % (24 * 60)) / 60, // hours
                start_minute % 60,               // minutes
                (end_minute % (24 * 60)) / 60,
                end_minute % 60,
                end_minute - start_minute,
                count
            )?;
        }
        Ok(())
    }
}

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

use tracing::{debug, instrument, trace, warn};

#[derive(Allocative, Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileIndex {
    pub histogram: FileHistogram,
    pub entry_indices: HashMap<String, Vec<u8>>,
}

impl FileIndex {
    #[instrument(skip(journal_file), fields(field_count = field_names.len()))]
    pub fn from(journal_file: &JournalFile<Mmap>, field_names: &[&[u8]]) -> Result<FileIndex> {
        let mut index = FileIndex::default();

        let entry_offsets = journal_file.entry_offsets()?;
        index.histogram = FileHistogram::from(journal_file, &entry_offsets)?;

        let mut data_offsets = Vec::new();
        let mut data_indices = Vec::new();
        let mut rb_serialized = Vec::new();

        for field_name in field_names {
            let field_data_iterator = journal_file.field_data_objects(field_name)?;

            for data_object in field_data_iterator {
                let (data_payload, ic) = {
                    let Ok(data_object) = data_object else {
                        continue;
                    };

                    let data_payload =
                        String::from_utf8_lossy(data_object.payload_bytes()).into_owned();
                    let Some(ic) = data_object.inlined_cursor() else {
                        continue;
                    };

                    (data_payload, ic)
                };

                data_offsets.clear();
                if ic.collect_offsets(journal_file, &mut data_offsets).is_err() {
                    continue;
                }

                data_indices.clear();
                get_matching_indices(&entry_offsets, &data_offsets, &mut data_indices);

                let mut rb = RoaringBitmap::from_sorted_iter(data_indices.iter().copied()).unwrap();
                rb.optimize();

                rb_serialized.clear();
                rb.serialize_into(&mut rb_serialized).unwrap();

                let compressed_roaring = lz4::block::compress(&rb_serialized[..], None, false)
                    .map_err(|e| format!("LZ4 compression failed: {}", e))
                    .unwrap();

                index
                    .entry_indices
                    .insert(data_payload.clone(), compressed_roaring);
            }
        }

        Ok(index)
    }
}
