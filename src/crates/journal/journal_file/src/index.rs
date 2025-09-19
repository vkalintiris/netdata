use crate::JournalFile;
use crate::Mmap;
use error::Result;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroU64;

/// A minute-aligned bucket in the histogram index.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct FileBucket {
    /// Minute-aligned seconds since EPOCH.
    minute: u64,
    /// Index into the global entry offsets array.
    last_offset_index: usize,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
///
/// This structure stores minute boundaries and their corresponding offset indices,
/// enabling O(log n) lookups for time ranges and histogram generation with configurable
/// bucket sizes (1-minute, 10-minute, etc.).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
                    // First entry - don't create bucket yet, just track the minute
                    debug_assert_eq!(offset_index, 0);
                    current_minute = Some(minute);
                }
                Some(prev_minute) if minute > prev_minute => {
                    // New minute boundary - save the LAST index of the previous minute
                    buckets.push(FileBucket {
                        minute: prev_minute,
                        last_offset_index: offset_index - 1, // Changed from offset_index to last_index
                    });
                    current_minute = Some(minute);
                }
                _ => {} // Same minute, continue
            }
        }

        // Don't forget the last bucket!
        // This is crucial - we need to save the final minute's entries
        if let Some(last_minute) = current_minute {
            buckets.push(FileBucket {
                minute: last_minute,
                last_offset_index: entry_offsets.len() - 1,
            });
        }

        Ok(FileHistogram { buckets })
    }

    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    // New helper method - now we can easily get total entry count!
    pub fn total_entries(&self) -> usize {
        self.buckets
            .last()
            .map(|b| b.last_offset_index + 1)
            .unwrap_or(0)
    }

    // Helper to get entry range for a specific bucket
    pub fn get_entry_range(&self, bucket_index: usize) -> Option<(u32, u32)> {
        let bucket = self.buckets.get(bucket_index)?;
        let start = if bucket_index == 0 {
            0
        } else {
            self.buckets[bucket_index - 1].last_offset_index + 1
        };
        Some((start as u32, bucket.last_offset_index as u32))
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileIndex {
    pub file_histogram: FileHistogram,
    pub entry_indices: HashMap<String, RoaringBitmap>,
}

impl FileIndex {
    #[instrument(skip(journal_file), fields(field_count = field_names.len()))]
    pub fn from(journal_file: &JournalFile<Mmap>, field_names: &[&[u8]]) -> Result<FileIndex> {
        let mut index = FileIndex::default();

        let entry_offsets = journal_file.entry_offsets()?;
        index.file_histogram = FileHistogram::from(journal_file, &entry_offsets)?;

        let mut data_offsets = Vec::new();
        let mut data_indices = Vec::new();

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

                index.entry_indices.insert(data_payload.clone(), rb);
            }
        }

        Ok(index)
    }
}

// #[derive(Debug, Clone)]
// pub struct MinuteIndex {
//     pub minutes_info: HashMap<u64, Vec<MinuteInfo>>,
// }

// impl MinuteIndex {
//     pub fn from_file_index(filename: &str, file_index: &FileIndex) -> Self {
//         let mut minutes_info: HashMap<u64, Vec<MinuteInfo>> = HashMap::new();

//         let buckets = &file_index.file_histogram.buckets;

//         let first_minute = buckets.first().unwrap().minute;

//         // Process each bucket (minute)
//         let mut cells = Vec::new();
//         for (i, _) in buckets.iter().enumerate() {
//             let range = file_index.file_histogram.get_entry_range(i).unwrap();
//             cells.push(format!(
//                 "@{} - [{}, {}]",
//                 buckets[i].minute - first_minute,
//                 range.0,
//                 range.1,
//             ));
//         }

//         use term_grid::{Direction, Filling, Grid, GridOptions};

//         let grid = Grid::new(
//             cells,
//             GridOptions {
//                 filling: Filling::Spaces(1),
//                 direction: Direction::LeftToRight,
//                 width: 240,
//             },
//         );

//         println!("{grid}");

//         MinuteIndex { minutes_info }
//     }
// }

/// NOTE %%%%%%%%%%%%%%%% WORKS %%%%%%%%%%%%%%%%%%%%%%%
/// A reverse index that maps minutes to the field values that appear in those minutes,
/// along with the entry indices for each field value.
// #[derive(Debug, Clone, Default, Serialize, Deserialize)]
// pub struct MinuteIndex {
//     /// Maps minute timestamps to another map of (field_value -> entry indices in that minute)
//     pub minute_to_field_entries: HashMap<u64, HashMap<String, RoaringBitmap>>,
// }

// impl MinuteIndex {
//     /// Build a MinuteIndex from a FileIndex
//     pub fn from_file_index(file_index: &FileIndex) -> Self {
//         let mut minute_to_field_entries: HashMap<u64, HashMap<String, RoaringBitmap>> =
//             HashMap::new();

//         // First, build a map of minute -> all entries in that minute
//         let mut minute_to_all_entries: HashMap<u64, RoaringBitmap> = HashMap::new();

//         for (bucket_index, bucket) in file_index.file_histogram.buckets.iter().enumerate() {
//             if let Some((start, end)) = file_index.file_histogram.get_entry_range(bucket_index) {
//                 let mut minute_bitmap = RoaringBitmap::new();
//                 minute_bitmap.insert_range(start..=end);
//                 println!("U: {:#?}", minute_bitmap.statistics());
//                 minute_bitmap.optimize();
//                 println!("O: {:#?}", minute_bitmap.statistics());
//                 // for entry_idx in start..=end {
//                 //     minute_bitmap.insert(entry_idx as u32);
//                 // }
//                 minute_to_all_entries.insert(bucket.minute, minute_bitmap);
//             }
//         }

//         // Now, for each field value in the FileIndex, intersect with each minute's entries
//         for (field_value, field_entries) in &file_index.entry_indices {
//             for (&minute, minute_entries) in &minute_to_all_entries {
//                 // Find entries that have this field value AND are in this minute
//                 let intersection = field_entries & minute_entries;

//                 if !intersection.is_empty() {
//                     minute_to_field_entries
//                         .entry(minute)
//                         .or_insert_with(HashMap::new)
//                         .insert(field_value.clone(), intersection);
//                 }
//             }
//         }

//         MinuteIndex {
//             minute_to_field_entries,
//         }
//     }

//     /// Get all field values and their entries for a specific minute
//     pub fn get_minute_data(&self, minute: u64) -> Option<&HashMap<String, RoaringBitmap>> {
//         self.minute_to_field_entries.get(&minute)
//     }

//     /// Get all entries for a specific field value within a specific minute
//     pub fn get_field_entries_for_minute(
//         &self,
//         minute: u64,
//         field_value: &str,
//     ) -> Option<&RoaringBitmap> {
//         self.minute_to_field_entries
//             .get(&minute)
//             .and_then(|fields| fields.get(field_value))
//     }

//     /// Get all unique field values that appear in a specific minute
//     pub fn get_field_values_for_minute(&self, minute: u64) -> Vec<String> {
//         self.minute_to_field_entries
//             .get(&minute)
//             .map(|fields| fields.keys().cloned().collect())
//             .unwrap_or_default()
//     }

//     /// Get all entries for a field value across a time range
//     pub fn get_field_entries_in_range(
//         &self,
//         field_value: &str,
//         start_minute: u64,
//         end_minute: u64,
//     ) -> RoaringBitmap {
//         let mut result = RoaringBitmap::new();

//         for (&minute, fields) in &self.minute_to_field_entries {
//             if minute >= start_minute && minute <= end_minute {
//                 if let Some(entries) = fields.get(field_value) {
//                     result |= entries;
//                 }
//             }
//         }

//         result.optimize();
//         result
//     }

//     /// Get histogram of a specific field value over time
//     pub fn get_field_histogram(
//         &self,
//         field_value: &str,
//         start_minute: u64,
//         end_minute: u64,
//         bucket_size_minutes: u64,
//     ) -> Vec<(u64, usize)> {
//         let mut histogram = Vec::new();
//         let mut current_bucket_start = (start_minute / bucket_size_minutes) * bucket_size_minutes;

//         while current_bucket_start <= end_minute {
//             let bucket_end = current_bucket_start + bucket_size_minutes - 1;
//             let entries = self.get_field_entries_in_range(
//                 field_value,
//                 current_bucket_start.max(start_minute),
//                 bucket_end.min(end_minute),
//             );

//             if entries.len() > 0 {
//                 histogram.push((current_bucket_start, entries.len() as usize));
//             }

//             current_bucket_start += bucket_size_minutes;
//         }

//         histogram
//     }

//     /// Get counts of all field values within a time range
//     pub fn get_field_value_counts_in_range(
//         &self,
//         start_minute: u64,
//         end_minute: u64,
//     ) -> HashMap<String, usize> {
//         let mut counts = HashMap::new();

//         for (&minute, fields) in &self.minute_to_field_entries {
//             if minute >= start_minute && minute <= end_minute {
//                 for (field_value, entries) in fields {
//                     *counts.entry(field_value.clone()).or_insert(0) += entries.len() as usize;
//                 }
//             }
//         }

//         counts
//     }

//     /// Get top N field values by entry count within a time range
//     pub fn get_top_field_values_in_range(
//         &self,
//         start_minute: u64,
//         end_minute: u64,
//         top_n: usize,
//     ) -> Vec<(String, usize)> {
//         let counts = self.get_field_value_counts_in_range(start_minute, end_minute);
//         let mut sorted: Vec<_> = counts.into_iter().collect();
//         sorted.sort_by(|a, b| b.1.cmp(&a.1));
//         sorted.truncate(top_n);
//         sorted
//     }

//     /// Get statistics about the index
//     pub fn stats(&self) -> MinuteIndexStats {
//         let total_minutes = self.minute_to_field_entries.len();
//         let mut total_field_values = 0;
//         let mut total_entries = 0;

//         for fields in self.minute_to_field_entries.values() {
//             total_field_values += fields.len();
//             for entries in fields.values() {
//                 total_entries += entries.len() as usize;
//             }
//         }

//         MinuteIndexStats {
//             total_minutes,
//             total_field_values,
//             total_entries,
//             avg_field_values_per_minute: if total_minutes > 0 {
//                 total_field_values as f64 / total_minutes as f64
//             } else {
//                 0.0
//             },
//         }
//     }
// }

// #[derive(Debug, Clone, Serialize, Deserialize)]
// pub struct MinuteIndexStats {
//     pub total_minutes: usize,
//     pub total_field_values: usize,
//     pub total_entries: usize,
//     pub avg_field_values_per_minute: f64,
// }

// // Extension to FileIndex
// impl FileIndex {
//     /// Build a MinuteIndex for this FileIndex
//     pub fn build_minute_index(&self) -> MinuteIndex {
//         MinuteIndex::from_file_index(self)
//     }
// }
use tracing::info;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileInfo {
    pub filename: String,
    pub entry_indices: HashMap<String, RoaringBitmap>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MinuteInfo {
    pub minute: u64,
    pub files_info: Vec<FileInfo>,
}

/// A reverse index that maps minutes to file information and field values within those minutes
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MinuteIndex {
    pub minute_info: HashMap<u64, MinuteInfo>,
}

impl MinuteIndex {
    pub fn merge_file_index(&mut self, filename: &str, file_index: FileIndex) {
        // First, build a map of minute -> all entries in that minute
        let mut minute_to_all_entries: HashMap<u64, RoaringBitmap> = HashMap::new();

        for (bucket_index, bucket) in file_index.file_histogram.buckets.iter().enumerate() {
            if let Some((start, end)) = file_index.file_histogram.get_entry_range(bucket_index) {
                let mut minute_bitmap = RoaringBitmap::new();
                minute_bitmap.insert_range(start..=end);
                minute_to_all_entries.insert(bucket.minute, minute_bitmap);
            }
        }

        // Now, for each minute, find which field values appear in it
        for (&minute, minute_entries) in &minute_to_all_entries {
            let mut entry_indices = HashMap::new();

            // For each field value in the FileIndex, check if it appears in this minute
            for (field_value, field_entries) in &file_index.entry_indices {
                // Find entries that have this field value AND are in this minute
                let intersection = field_entries & minute_entries;

                if !intersection.is_empty() {
                    entry_indices.insert(field_value.clone(), intersection);
                }
            }

            // Only create FileInfo if there are entries for this minute
            if !entry_indices.is_empty() {
                let file_info = FileInfo {
                    filename: filename.to_string(),
                    entry_indices,
                };

                if let Some(minute_info) = self.minute_info.get_mut(&minute) {
                    minute_info.files_info.push(file_info);
                } else {
                    self.minute_info.insert(
                        minute,
                        MinuteInfo {
                            minute,
                            files_info: vec![file_info],
                        },
                    );
                }
            }
        }
    }

    // pub fn from_file_index(filename: &str, file_index: &FileIndex) -> Self {
    //     let mut minute_to_file_info: HashMap<u64, FileInfo> = HashMap::new();

    //     // First, build a map of minute -> all entries in that minute
    //     let mut minute_to_all_entries: HashMap<u64, RoaringBitmap> = HashMap::new();

    //     for (bucket_index, bucket) in file_index.file_histogram.buckets.iter().enumerate() {
    //         if let Some((start, end)) = file_index.file_histogram.get_entry_range(bucket_index) {
    //             let mut minute_bitmap = RoaringBitmap::new();
    //             minute_bitmap.insert_range(start..=end);
    //             minute_to_all_entries.insert(bucket.minute, minute_bitmap);
    //         }
    //     }

    //     // Now, for each minute, find which field values appear in it
    //     for (&minute, minute_entries) in &minute_to_all_entries {
    //         let mut entry_indices = HashMap::new();

    //         // For each field value in the FileIndex, check if it appears in this minute
    //         for (field_value, field_entries) in &file_index.entry_indices {
    //             // Find entries that have this field value AND are in this minute
    //             let intersection = field_entries & minute_entries;

    //             if !intersection.is_empty() {
    //                 entry_indices.insert(field_value.clone(), intersection);
    //             }
    //         }

    //         // Only create FileInfo if there are entries for this minute
    //         if !entry_indices.is_empty() {
    //             minute_to_file_info.insert(
    //                 minute,
    //                 FileInfo {
    //                     filename: filename.to_string(),
    //                     entry_indices,
    //                 },
    //             );
    //         }
    //     }

    //     MinuteIndex {
    //         minute_to_file_info,
    //     }
    // }

    // /// Get file info for a specific minute
    // pub fn get_minute_info(&self, minute: u64) -> Option<&FileInfo> {
    //     self.minute_info.get(&minute)
    // }

    // /// Get all entries for a specific field value within a specific minute
    // pub fn get_field_entries_for_minute(
    //     &self,
    //     minute: u64,
    //     field_value: &str,
    // ) -> Option<&RoaringBitmap> {
    //     self.minute_info
    //         .get(&minute)
    //         .and_then(|info| info.entry_indices.get(field_value))
    // }

    // /// Get all unique field values that appear in a specific minute
    // pub fn get_field_values_for_minute(&self, minute: u64) -> Vec<String> {
    //     self.minute_info
    //         .get(&minute)
    //         .map(|info| info.entry_indices.keys().cloned().collect())
    //         .unwrap_or_default()
    // }

    // /// Get all entries for a field value across a time range
    // pub fn get_field_entries_in_range(
    //     &self,
    //     field_value: &str,
    //     start_minute: u64,
    //     end_minute: u64,
    // ) -> RoaringBitmap {
    //     let mut result = RoaringBitmap::new();

    //     for (&minute, file_info) in &self.minute_info {
    //         if minute >= start_minute && minute <= end_minute {
    //             if let Some(entries) = file_info.entry_indices.get(field_value) {
    //                 result |= entries;
    //             }
    //         }
    //     }

    //     result.optimize();
    //     result
    // }

    // /// Get histogram of a specific field value over time
    // pub fn get_field_histogram(
    //     &self,
    //     field_value: &str,
    //     start_minute: u64,
    //     end_minute: u64,
    //     bucket_size_minutes: u64,
    // ) -> Vec<(u64, usize)> {
    //     let mut histogram = Vec::new();
    //     let mut current_bucket_start = (start_minute / bucket_size_minutes) * bucket_size_minutes;

    //     while current_bucket_start <= end_minute {
    //         let bucket_end = current_bucket_start + bucket_size_minutes - 1;
    //         let entries = self.get_field_entries_in_range(
    //             field_value,
    //             current_bucket_start.max(start_minute),
    //             bucket_end.min(end_minute),
    //         );

    //         if !entries.is_empty() {
    //             histogram.push((current_bucket_start, entries.len() as usize));
    //         }

    //         current_bucket_start += bucket_size_minutes;
    //     }

    //     histogram
    // }

    // /// Get counts of all field values within a time range
    // pub fn get_field_value_counts_in_range(
    //     &self,
    //     start_minute: u64,
    //     end_minute: u64,
    // ) -> HashMap<String, usize> {
    //     let mut counts = HashMap::new();

    //     for (&minute, file_info) in &self.minute_info {
    //         if minute >= start_minute && minute <= end_minute {
    //             for (field_value, entries) in &file_info.entry_indices {
    //                 *counts.entry(field_value.clone()).or_insert(0) += entries.len() as usize;
    //             }
    //         }
    //     }

    //     counts
    // }

    /// Get all minutes that contain data
    pub fn get_minutes(&self) -> Vec<u64> {
        let mut minutes: Vec<_> = self.minute_info.keys().copied().collect();
        minutes.sort_unstable();
        minutes
    }

    /// Get time bounds (earliest and latest minutes)
    pub fn time_bounds(&self) -> Option<(u64, u64)> {
        let minutes = self.get_minutes();
        if minutes.is_empty() {
            None
        } else {
            Some((minutes[0], minutes[minutes.len() - 1]))
        }
    }
}

// Extension to FileIndex

impl FileIndex {
    /// Calculate the approximate memory size in bytes consumed by this FileIndex
    pub fn memory_size(&self) -> usize {
        let mut total = 0;

        // FileHistogram: struct + vector of buckets
        total += std::mem::size_of::<FileHistogram>();
        total += self.file_histogram.buckets.capacity() * std::mem::size_of::<FileBucket>();

        // HashMap overhead
        total += self.entry_indices.capacity() * std::mem::size_of::<(*const u8, usize)>();

        // HashMap contents: keys + bitmaps
        for (key, bitmap) in &self.entry_indices {
            total += key.capacity();
            total += bitmap.serialized_size();
        }

        total
    }
}

impl MinuteIndex {
    /// Calculate the approximate memory size in bytes consumed by this MinuteIndex
    pub fn memory_size(&self) -> usize {
        0
    }

    pub fn ser(&self) -> Vec<u8> {
        bincode::serde::encode_to_vec(self, bincode::config::standard()).unwrap()
        // let length = bincode::serde::encode_to_vec(self, &mut slice, bincode::config::standard());
    }

    pub fn per_minute_compressed_len(&self) -> usize {
        let mut n = 0;
        for index in self.minute_info.values() {
            let serialized =
                bincode::serde::encode_to_vec(index, bincode::config::standard()).unwrap();
            // let serialized = minute_index.ser();
            let compressed = lz4::block::compress(&serialized[..], None, false)
                .map_err(|e| format!("LZ4 compression failed: {}", e))
                .unwrap();

            n += compressed.len();
        }
        n
    }
}
