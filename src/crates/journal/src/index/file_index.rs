use crate::error::JournalError;
use crate::error::Result;
use crate::file::DataObject;
use crate::file::HashableObject;
use crate::file::JournalFile;
use crate::file::Mmap;
use crate::file::offset_array::InlinedCursor;
use crate::index::bitmap::Bitmap;
use bincode;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroU64;
use tracing::{error, warn};

// TODO: pass the field name that should be used to extract the source timestamp
fn parse_source_timestamp(data_object: &DataObject<&[u8]>) -> Result<u64> {
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
    // Construct the file histogram of a field value from it's bitmap
    pub fn from_bitmap(&self, bitmap: &Bitmap) -> Vec<(u64, u32)> {
        if self.buckets.is_empty() || bitmap.is_empty() {
            return Vec::new();
        }

        let mut rb_histogram = Vec::new();
        let mut rb_iter = bitmap.iter().peekable();

        for bucket in &self.buckets {
            let mut count = 0;

            // Count values in this bucket
            while let Some(&value) = rb_iter.peek() {
                if value <= bucket.last_offset_index as u32 {
                    count += 1;
                    rb_iter.next();
                } else {
                    break;
                }
            }

            if count > 0 {
                rb_histogram.push((bucket.bucket_seconds, count));
            }

            // All values processed
            if rb_iter.peek().is_none() {
                break;
            }
        }

        rb_histogram
    }

    pub fn from_timestamp_offset_pairs(
        timestamp_offset_pairs: &[(u64, NonZeroU64)],
        bucket_size_seconds: u64,
    ) -> FileHistogram {
        debug_assert!(timestamp_offset_pairs.is_sorted());
        debug_assert_ne!(bucket_size_seconds, 0);

        let mut buckets = Vec::new();
        let mut current_bucket = None;

        // Convert seconds to microseconds for bucket size
        let bucket_size_micros = bucket_size_seconds * 1_000_000;

        for (offset_index, &(timestamp_micros, _offset)) in
            timestamp_offset_pairs.iter().enumerate()
        {
            // Calculate which bucket this timestamp falls into
            let bucket = (timestamp_micros / bucket_size_micros) * bucket_size_seconds;

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
                last_offset_index: timestamp_offset_pairs.len() - 1,
            });
        }

        FileHistogram {
            bucket_size_seconds,
            buckets,
        }
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileIndex {
    pub file_histogram: FileHistogram,

    // File fields
    pub file_fields: HashSet<String>,

    // Index that maps field values to their entries bitmap
    pub entries_index: HashMap<String, Bitmap>,
}

impl FileIndex {
    pub fn memory_size(&self) -> usize {
        bincode::serialized_size(self).unwrap() as usize
    }

    pub fn is_indexed(&self, field: &str) -> bool {
        // If the file does not contain the field, then it's not indexed
        if !self.file_fields.contains(field) {
            return false;
        }

        // If the entries index contains a key that starts with the field
        // name, then we've have indexed all the values the field takes
        for key in self.entries_index.keys() {
            if key.starts_with(field) {
                return true;
            }
        }

        false
    }
}

#[derive(Debug, Default)]
pub struct FileIndexer {
    // Associates a source timestamp value with its inlined cursor
    source_timestamp_cursor_pairs: Vec<(u64, InlinedCursor)>,

    // Scratch buffer to collect entry offsets from the inlined cursor of a
    // a source timestamp value, or the global entry offset array
    entry_offsets: Vec<NonZeroU64>,

    // Associates a source timestamp value with its entry offset
    source_timestamp_entry_offset_pairs: Vec<(u64, NonZeroU64)>,

    // Associates a journal file's entry realtime value with its offset
    realtime_entry_offset_pairs: Vec<(u64, NonZeroU64)>,

    // Scratch buffer to collect the indices of entries in which a data
    // object appears
    entry_indices: Vec<u32>,

    /// Maps entry offsets to an index of an implicitly defined time-ordered
    /// array of entries.
    entry_offset_index: HashMap<NonZeroU64, u64>,
}

fn collect_file_fields(journal_file: &JournalFile<Mmap>) -> HashSet<String> {
    let mut fields = HashSet::new();

    for item in journal_file.fields() {
        let field = item.unwrap();

        let payload = String::from_utf8_lossy(field.get_payload()).into_owned();
        fields.insert(payload);
    }

    fields
}

impl FileIndexer {
    pub fn index(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        source_timestamp_field: Option<&[u8]>,
        field_names: &[&[u8]],
        bucket_size_seconds: u64,
    ) -> Result<FileIndex> {
        let n_entries = journal_file.journal_header_ref().n_entries as _;
        self.source_timestamp_cursor_pairs.reserve(n_entries);
        self.source_timestamp_entry_offset_pairs.reserve(n_entries);
        self.realtime_entry_offset_pairs.reserve(n_entries);
        self.entry_indices.reserve(n_entries / 2);
        self.entry_offsets.reserve(8);
        self.entry_offset_index.reserve(n_entries);

        let file_fields = collect_file_fields(journal_file);

        let file_histogram =
            self.build_file_histogram(journal_file, source_timestamp_field, bucket_size_seconds)?;
        let entries_index = self.build_entries_index(journal_file, field_names)?;

        Ok(FileIndex {
            file_fields,
            file_histogram,
            entries_index,
        })
    }

    fn build_entries_index(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        field_names: &[&[u8]],
    ) -> Result<HashMap<String, Bitmap>> {
        let mut entries_index = HashMap::new();

        for field_name in field_names {
            // Get the data object iterator for this field
            let field_data_iterator = match journal_file.field_data_objects(field_name) {
                Ok(field_data_iterator) => field_data_iterator,
                Err(e) => {
                    warn!("failed to iterate field data objects {:#?}", e);
                    continue;
                }
            };

            for data_object in field_data_iterator {
                // Get the payload and the inlined cursor for this data object
                let (data_payload, inlined_cursor) = {
                    let Ok(data_object) = data_object else {
                        continue;
                    };

                    let data_payload =
                        String::from_utf8_lossy(data_object.payload_bytes()).into_owned();
                    let Some(inlined_cursor) = data_object.inlined_cursor() else {
                        continue;
                    };

                    (data_payload, inlined_cursor)
                };

                // Collect the offset of entries where this data object appears
                self.entry_offsets.clear();
                if inlined_cursor
                    .collect_offsets(journal_file, &mut self.entry_offsets)
                    .is_err()
                {
                    continue;
                }

                // Map entry offsets where this data object appears to
                // entry indices
                self.entry_indices.clear();
                for entry_offset in self.entry_offsets.iter() {
                    let Some(entry_index) = self.entry_offset_index.get(entry_offset) else {
                        continue;
                    };
                    self.entry_indices.push(*entry_index as u32);
                }
                self.entry_indices.sort_unstable();

                // Create the bitmap for the entry indices
                let mut bitmap =
                    Bitmap::from_sorted_iter(self.entry_indices.iter().copied()).unwrap();
                bitmap.optimize();

                entries_index.insert(data_payload.clone(), bitmap);
            }
        }

        Ok(entries_index)
    }

    fn build_file_histogram(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        source_timestamp_field: Option<&[u8]>,
        bucket_size_seconds: u64,
    ) -> Result<FileHistogram> {
        if let Some(source_timestamp_field) = source_timestamp_field {
            if let Ok(field_data_iterator) = journal_file.field_data_objects(source_timestamp_field)
            {
                // Collect the inlined cursors of the source timestamp field
                self.source_timestamp_cursor_pairs.clear();
                for data_object_result in field_data_iterator {
                    let Ok(data_object) = data_object_result else {
                        warn!("loading data object failed");
                        continue;
                    };

                    let Ok(source_timestamp) = parse_source_timestamp(&data_object) else {
                        warn!("parsing source timestamp failed");
                        continue;
                    };

                    let Some(ic) = data_object.inlined_cursor() else {
                        warn!(
                            "missing inlined cursor for source timestamp {:?}",
                            source_timestamp
                        );
                        continue;
                    };

                    self.source_timestamp_cursor_pairs
                        .push((source_timestamp, ic));
                }

                // Collect the source timestamp value and offset pairs
                self.source_timestamp_entry_offset_pairs.clear();
                for (ts, ic) in self.source_timestamp_cursor_pairs.iter() {
                    self.entry_offsets.clear();

                    match ic.collect_offsets(journal_file, &mut self.entry_offsets) {
                        Ok(_) => {}
                        Err(e) => {
                            error!("failed to collect offsets from source timestamp: {}", e);
                            continue;
                        }
                    }

                    for entry_offset in &self.entry_offsets {
                        self.source_timestamp_entry_offset_pairs
                            .push((*ts, *entry_offset));
                    }
                }
                self.source_timestamp_entry_offset_pairs.sort_unstable();

                // Map each entry offset to its position in the pair vector
                for (idx, (_, entry_offset)) in
                    self.source_timestamp_entry_offset_pairs.iter().enumerate()
                {
                    self.entry_offset_index.insert(*entry_offset, idx as _);
                }
            }
        }

        // Load the global entry offset array
        self.entry_offsets.clear();
        journal_file.entry_offsets(&mut self.entry_offsets)?;

        // Fill any missing entry offset keys
        self.realtime_entry_offset_pairs.clear();
        for entry_offset in self.entry_offsets.iter() {
            if self.entry_offset_index.contains_key(entry_offset) {
                continue;
            }

            let timestamp = {
                let entry = journal_file.entry_ref(*entry_offset)?;
                entry.header.realtime
            };

            self.realtime_entry_offset_pairs
                .push((timestamp, *entry_offset));
        }

        // Reconstruct our indexes if we have entries whose time does not
        // come from the source timestamp
        if !self.realtime_entry_offset_pairs.is_empty() {
            self.source_timestamp_entry_offset_pairs
                .append(&mut self.realtime_entry_offset_pairs);
            self.source_timestamp_entry_offset_pairs.sort_unstable();

            // Map again each entry offset to its position in the pair vector
            self.entry_offset_index.clear();
            for (idx, (_, entry_offset)) in
                self.source_timestamp_entry_offset_pairs.iter().enumerate()
            {
                self.entry_offset_index.insert(*entry_offset, idx as _);
            }
        }

        // Now we can build the file histogram
        Ok(FileHistogram::from_timestamp_offset_pairs(
            self.source_timestamp_entry_offset_pairs.as_slice(),
            bucket_size_seconds,
        ))
    }
}
