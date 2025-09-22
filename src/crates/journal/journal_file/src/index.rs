use crate::DataObject;
use crate::JournalFile;
use crate::Mmap;
use error::JournalError;
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

fn parse_source_realtime_timestamp(data_object: &DataObject<&[u8]>) -> Result<u64> {
    const PREFIX: &[u8] = b"_SOURCE_REALTIME_TIMESTAMP=";

    // Get the payload bytes
    let payload = data_object.payload_bytes();

    // Check if it starts with the expected prefix
    if !payload.starts_with(PREFIX) {
        return Err(JournalError::InvalidField);
    }

    // Get the timestamp portion after the '='
    let timestamp_bytes = &payload[PREFIX.len()..];

    // Convert to string and parse
    let timestamp_str =
        std::str::from_utf8(timestamp_bytes).map_err(|_| JournalError::InvalidField)?;

    let timestamp = timestamp_str
        .parse::<u64>()
        .map_err(|_| JournalError::InvalidField)?;

    Ok(timestamp)
}

impl FileIndex {
    /// Creates a sorted vector of entry offsets ordered by _SOURCE_REALTIME_TIMESTAMP field
    /// instead of the journal's default realtime ordering. This is useful when the source
    /// realtime differs from the journal realtime (e.g., when entries are received out of order).
    ///
    /// Returns a vector of entry offsets sorted by their _SOURCE_REALTIME_TIMESTAMP value.
    /// If the field is not found or entries don't have this field, returns an empty vector.
    pub fn entries_sorted_by_source_realtime(
        journal_file: &JournalFile<Mmap>,
        entries_hashmap: &mut HashMap<u64, u64>,
    ) -> Result<()> {
        let field_name = b"_SOURCE_REALTIME_TIMESTAMP";

        // Get the field data objects for _SOURCE_REALTIME_TIMESTAMP
        let field_data_iterator = journal_file.field_data_objects(field_name)?;

        // Each pair holds the timestamp and the inlined cursor so that we
        // can iterate/collect the entry offsets that have that timestamp.
        let mut tic = Vec::with_capacity(journal_file.journal_header_ref().n_entries as usize);

        for data_object_result in field_data_iterator {
            let Ok(data_object) = data_object_result else {
                panic!(">>>>>>>>>>>>>>>>>>>>>>>>>> 1");
                // continue;
            };

            let Ok(source_timestamp) = parse_source_realtime_timestamp(&data_object) else {
                panic!(">>>>>>>>>>>>>>>>>>>>>>>>>> 2");
                // continue;
            };

            let Some(ic) = data_object.inlined_cursor() else {
                // panic!(">>>>>>>>>>>>>>>>>>>>>>>>>> 3");
                println!("Data object does not have an inlined cursor");
                continue;
            };

            tic.push((source_timestamp, ic));
        }

        let mut timestamp_entries = Vec::new();

        let mut offsets: Vec<NonZeroU64> = Vec::with_capacity(8);
        for (ts, ic) in tic {
            offsets.clear();
            ic.collect_offsets(journal_file, &mut offsets).ok();

            for offset in &offsets {
                timestamp_entries.push((ts, offset.get()));
            }
        }

        timestamp_entries.sort();

        entries_hashmap.reserve(timestamp_entries.len());
        for (idx, (_, entry_offset)) in timestamp_entries.iter().enumerate() {
            entries_hashmap.insert(*entry_offset, idx as _);
        }

        Ok(())
    }

    #[instrument(skip(journal_file), fields(field_count = field_names.len()))]
    pub fn from(
        journal_file: &JournalFile<Mmap>,
        field_names: &[&[u8]],
        hm: &mut HashMap<u64, u64>,
    ) -> Result<FileIndex> {
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

        Self::entries_sorted_by_source_realtime(journal_file, hm).unwrap();
        // println!("foo length {:#?}", index.foo.len());

        Ok(index)
    }
}
