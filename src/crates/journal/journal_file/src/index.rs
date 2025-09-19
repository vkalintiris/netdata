use crate::JournalFile;
use crate::Mmap;
use error::Result;
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::num::NonZeroU64;

/// A time-aligned bucket in the histogram index.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
struct FileBucket {
    /// Bucket-aligned seconds since EPOCH.
    /// (e.g., for 30s buckets, this would be 0, 30, 60, 90...)
    bucket_seconds: u64,
    /// Index into the global entry offsets array.
    last_offset_index: usize,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FileHistogram {
    /// The bucket size in seconds (e.g., 30, 60, 120)
    bucket_size_seconds: u64,
    /// Sparse vector containing only bucket boundaries where changes occur.
    buckets: Vec<FileBucket>,
}

impl FileHistogram {
    pub fn from(
        journal_file: &JournalFile<Mmap>,
        entry_offsets: &[NonZeroU64],
        bucket_size_seconds: u64,
    ) -> Result<FileHistogram> {
        debug_assert_ne!(bucket_size_seconds, 0);

        let mut buckets = Vec::new();
        let mut current_bucket = None;

        // Convert microseconds to seconds for bucket size
        let bucket_size_micros = bucket_size_seconds * 1_000_000;

        for (offset_index, &offset) in entry_offsets.iter().enumerate() {
            let entry = journal_file.entry_ref(offset)?;
            // Calculate which bucket this timestamp falls into
            let bucket = (entry.header.realtime / bucket_size_micros) * bucket_size_seconds;

            match current_bucket {
                None => {
                    // First entry - don't create bucket yet, just track the bucket
                    debug_assert_eq!(offset_index, 0);
                    current_bucket = Some(bucket);
                }
                Some(prev_bucket) if bucket > prev_bucket => {
                    // New bucket boundary - save the LAST index of the previous bucket
                    buckets.push(FileBucket {
                        bucket_seconds: prev_bucket,
                        last_offset_index: offset_index - 1,
                    });
                    current_bucket = Some(bucket);
                }
                _ => {} // Same bucket, continue
            }
        }

        // Don't forget the last bucket!
        if let Some(last_bucket) = current_bucket {
            buckets.push(FileBucket {
                bucket_seconds: last_bucket,
                last_offset_index: entry_offsets.len() - 1,
            });
        }

        Ok(FileHistogram {
            bucket_size_seconds,
            buckets,
        })
    }

    /// Get the end time of the histogram in microseconds since epoch
    pub fn end_time_micros(&self) -> Option<u64> {
        self.buckets.last().map(|bucket| {
            // The last bucket starts at bucket_seconds, and spans bucket_size_seconds
            // So the end time is the start of the bucket plus the bucket size
            (bucket.bucket_seconds + self.bucket_size_seconds) * 1_000_000
        })
    }

    /// Get the start time of the histogram in microseconds since epoch
    pub fn start_time_micros(&self) -> Option<u64> {
        self.buckets
            .first()
            .map(|bucket| bucket.bucket_seconds * 1_000_000)
    }

    /// Get the time range covered by this histogram
    pub fn time_range(&self) -> Option<(u64, u64)> {
        match (self.buckets.first(), self.buckets.last()) {
            (Some(first), Some(last)) => {
                let start = first.bucket_seconds * 1_000_000;
                let end = (last.bucket_seconds + self.bucket_size_seconds) * 1_000_000;

                Some((start, end))
            }
            _ => None,
        }
    }

    /// Get the duration covered by this histogram in seconds
    pub fn duration_seconds(&self) -> Option<u64> {
        match (self.buckets.first(), self.buckets.last()) {
            (Some(first), Some(last)) => {
                Some(last.bucket_seconds - first.bucket_seconds + self.bucket_size_seconds)
            }
            _ => None,
        }
    }

    /// Helper method to convert a timestamp to its bucket boundary
    pub fn timestamp_to_bucket(&self, timestamp_micros: u64) -> u64 {
        let bucket_size_micros = self.bucket_size_seconds * 1_000_000;
        (timestamp_micros / bucket_size_micros) * self.bucket_size_seconds
    }

    /// Get the bucket size in seconds
    pub fn bucket_size(&self) -> u64 {
        self.bucket_size_seconds
    }

    // Rest of the methods remain the same...
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    pub fn total_entries(&self) -> usize {
        self.buckets
            .last()
            .map(|b| b.last_offset_index + 1)
            .unwrap_or(0)
    }

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
        index.file_histogram = FileHistogram::from(journal_file, &entry_offsets, 60)?;

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
