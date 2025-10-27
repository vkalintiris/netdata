use super::Bitmap;
use crate::collections::{HashMap, HashSet};
use crate::error::{JournalError, Result};
use crate::file::{DataObject, HashableObject, JournalFile, Mmap, offset_array::InlinedCursor};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use bincode;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;
use std::sync::Arc;
use tracing::{error, warn};

fn parse_timestamp(field_name: &[u8], data_object: &DataObject<&[u8]>) -> Result<u64> {
    let payload = data_object.payload_bytes();

    if !payload.starts_with(field_name) || payload.len() < field_name.len() + 1 {
        return Err(JournalError::InvalidField);
    }

    // get the timestamp string after "field="
    let timestamp_str = std::str::from_utf8(&payload[field_name.len() + 1..])
        .map_err(|_| JournalError::InvalidField)?;

    let timestamp = timestamp_str
        .parse::<u64>()
        .map_err(|_| JournalError::InvalidField)?;

    Ok(timestamp)
}

fn collect_file_fields(journal_file: &JournalFile<Mmap>) -> HashSet<String> {
    let mut fields = HashSet::default();

    for value_guard in journal_file.fields() {
        let Ok(field) = value_guard else {
            error!("Failed to get collect file field");
            continue;
        };

        let payload = String::from_utf8_lossy(field.get_payload()).into_owned();
        fields.insert(payload);
    }

    fields
}

/// A time-aligned bucket in the file histogram.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Bucket {
    /// Start time of this bucket
    pub start_time: u32,
    /// Count of items in this bucket
    pub count: u32,
}

/// An index structure for efficiently generating time-based histograms from journal entries.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct Histogram {
    /// The duration of each bucket
    pub bucket_duration: u64,
    /// Sparse vector containing only bucket boundaries where changes occur.
    pub buckets: Vec<Bucket>,
}

impl Histogram {
    /// Calculate the total memory consumption in bytes
    pub fn memory_size_bytes(&self) -> usize {
        // Size of the struct itself on the stack
        std::mem::size_of::<Self>()
        // Size of the heap-allocated Vec buffer
        + self.buckets.capacity() * std::mem::size_of::<Bucket>()
    }

    pub fn from_timestamp_offset_pairs(
        bucket_duration: u64,
        timestamp_offset_pairs: &[(u64, NonZeroU64)],
    ) -> Histogram {
        debug_assert!(timestamp_offset_pairs.is_sorted());
        debug_assert_ne!(bucket_duration, 0);

        // let mut buckets = Vec::with_capacity(timestamp_offset_pairs.len());
        let mut buckets = Vec::new();
        let mut current_bucket = None;

        // Convert seconds to microseconds for bucket size
        let bucket_size_micros = bucket_duration * 1_000_000;

        for (offset_index, &(timestamp_micros, _offset)) in
            timestamp_offset_pairs.iter().enumerate()
        {
            // Calculate which bucket this timestamp falls into
            let bucket = (timestamp_micros / bucket_size_micros) * bucket_duration;

            match current_bucket {
                None => {
                    // First entry - don't create bucket yet, just track the bucket
                    debug_assert_eq!(offset_index, 0);
                    current_bucket = Some(bucket);
                }
                Some(prev_bucket) if bucket > prev_bucket => {
                    // New bucket boundary - save the LAST index of the previous bucket
                    buckets.push(Bucket {
                        start_time: prev_bucket as u32,
                        count: offset_index as u32 - 1,
                    });
                    current_bucket = Some(bucket);
                }
                _ => {} // Same bucket, continue
            }
        }

        // Don't forget the last bucket!
        if let Some(last_bucket) = current_bucket {
            buckets.push(Bucket {
                start_time: last_bucket as u32,
                count: timestamp_offset_pairs.len() as u32 - 1,
            });
        }

        Histogram {
            bucket_duration,
            buckets,
        }
    }

    // Construct the buckets of a bitmap that contains entry indexes.
    pub fn bitmap_buckets(&self, bitmap: &Bitmap) -> Vec<Bucket> {
        let mut buckets = Vec::new();
        let mut iter = bitmap.iter().peekable();

        for bucket in &self.buckets {
            let mut count = 0;

            while let Some(&value) = iter.peek() {
                if value <= bucket.count as u32 {
                    count += 1;
                    iter.next();
                } else {
                    break;
                }
            }

            if count > 0 {
                buckets.push(Bucket {
                    start_time: bucket.start_time,
                    count,
                });
            }

            if iter.peek().is_none() {
                break;
            }
        }

        buckets
    }

    /// Get the start time of the histogram.
    pub fn start_time(&self) -> Option<u64> {
        self.buckets.first().map(|bucket| bucket.start_time as u64)
    }

    /// Get the end time of the histogram.
    pub fn end_time(&self) -> Option<u64> {
        self.buckets
            .last()
            .map(|bucket| bucket.start_time as u64 + self.bucket_duration)
    }

    /// Get the time range covered by the histogram.
    pub fn time_range(&self) -> Option<(u64, u64)> {
        Some((self.start_time()?, self.end_time()?))
    }

    /// Get the duration covered by this histogram.
    pub fn duration(&self) -> Option<u64> {
        let (start, end) = self.time_range()?;
        Some(end - start)
    }

    /// Returns the number of buckets in the histogram.
    pub fn len(&self) -> usize {
        self.buckets.len()
    }

    /// Check if the file histogram is empty.
    pub fn is_empty(&self) -> bool {
        self.buckets.is_empty()
    }

    /// Get the total number of entries in the histogram.
    pub fn count(&self) -> usize {
        self.buckets
            .last()
            .map(|b| b.count as usize + 1)
            .unwrap_or(0)
    }

    /// Count the number of bitmap entries that fall within a time range.
    ///
    /// This method efficiently counts entries by using the histogram's bucket structure
    /// rather than iterating through all bitmap entries. It only works when the time
    /// range is aligned to the histogram's bucket_duration.
    ///
    /// # Arguments
    ///
    /// * `bitmap` - The bitmap containing entry indices to count
    /// * `start_time` - Start of the time range (must be aligned to bucket_duration)
    /// * `end_time` - End of the time range (must be aligned to bucket_duration)
    ///
    /// # Returns
    ///
    /// * `Some(count)` - The number of bitmap entries in the time range
    /// * `None` - If the time range is not aligned to bucket_duration or if the range
    ///   is invalid (start >= end, or outside histogram bounds)
    ///
    /// # Example
    ///
    /// ```ignore
    /// // Histogram with 60-second buckets
    /// let histogram = Histogram::new(60, buckets);
    /// let bitmap = /* bitmap from filter evaluation */;
    ///
    /// // Count entries between 1000 and 1120 seconds (aligned to 60s buckets)
    /// if let Some(count) = histogram.count_bitmap_entries_in_range(&bitmap, 1000, 1120) {
    ///     println!("Found {} entries in time range", count);
    /// }
    /// ```
    pub fn count_bitmap_entries_in_range(
        &self,
        bitmap: &Bitmap,
        start_time: u64,
        end_time: u64,
    ) -> Option<usize> {
        // Validate inputs
        if start_time >= end_time {
            return None;
        }

        // Verify alignment to bucket_duration
        if !start_time.is_multiple_of(self.bucket_duration)
            || !end_time.is_multiple_of(self.bucket_duration)
        {
            return None;
        }

        // Handle empty histogram or bitmap
        if self.buckets.is_empty() || bitmap.is_empty() {
            return Some(0);
        }

        // Find the bucket indices for start and end times using binary search
        // partition_point returns the index of the first bucket with start_time >= start_time
        let start_bucket_idx = self
            .buckets
            .partition_point(|b| (b.start_time as u64) < start_time);

        // If start_bucket_idx is beyond all buckets, no matches possible
        if start_bucket_idx >= self.buckets.len() {
            return Some(0);
        }

        // Find the last bucket that starts before end_time
        // partition_point returns the index of the first bucket with start_time >= end_time,
        // so we need to subtract 1 to get the last bucket before end_time
        let end_bucket_idx = self
            .buckets
            .partition_point(|b| (b.start_time as u64) < end_time)
            .saturating_sub(1);

        // If start is after end, the range doesn't contain any buckets
        if start_bucket_idx > end_bucket_idx {
            return Some(0);
        }

        // Get the running count boundaries
        // For start: we want entries AFTER the previous bucket's running count
        let start_running_count = if start_bucket_idx == 0 {
            0
        } else {
            self.buckets[start_bucket_idx - 1].count + 1
        };

        // For end: we want entries UP TO AND INCLUDING this bucket's running count
        let end_running_count = self.buckets[end_bucket_idx].count;

        // Range is [start_running_count, end_running_count + 1) since range_cardinality is exclusive on the end
        let count = bitmap.range_cardinality(start_running_count..(end_running_count + 1));

        Some(count as usize)
    }
}

use chrono::{Local, TimeZone};

impl std::fmt::Display for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.buckets.is_empty() {
            return writeln!(f, "Empty histogram");
        }

        writeln!(f, "Histogram (bucket_duration: {}s)", self.bucket_duration)?;
        writeln!(f, "{:<18} {:<12} {:<12}", "Start Time", "Count", "Running")?;
        writeln!(f, "{:-<42}", "")?;

        let mut prev_running = 0;
        for (i, bucket) in self.buckets.iter().enumerate() {
            let count = if i == 0 {
                bucket.count + 1
            } else {
                bucket.count - prev_running
            };

            // Convert seconds to datetime with format: dd/mm HH:MM:SS
            let datetime = Local
                .timestamp_opt(bucket.start_time as i64, 0)
                .single()
                .map(|dt| dt.format("%d/%m %H:%M:%S").to_string())
                .unwrap_or_else(|| format!("{}", bucket.start_time));

            writeln!(f, "{:<18} {:<12} {:<12}", datetime, count, bucket.count)?;
            prev_running = bucket.count;
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndexInner {
    // The journal file's histogram
    pub histogram: Histogram,

    // Set of fields in the file
    pub fields: HashSet<String>,

    // Bitmap for each indexed field
    pub bitmaps: HashMap<String, Bitmap>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndex {
    pub inner: Arc<FileIndexInner>,
}

impl Serialize for FileIndex {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.inner.as_ref().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for FileIndex {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let inner = FileIndexInner::deserialize(deserializer)?;
        Ok(FileIndex {
            inner: Arc::new(inner),
        })
    }
}

impl FileIndex {
    pub fn new(
        histogram: Histogram,
        fields: HashSet<String>,
        bitmaps: HashMap<String, Bitmap>,
    ) -> Self {
        let inner = FileIndexInner {
            histogram,
            fields,
            bitmaps,
        };
        Self {
            inner: Arc::new(inner),
        }
    }

    pub fn bucket_duration(&self) -> u64 {
        self.inner.histogram.bucket_duration
    }

    pub fn histogram(&self) -> &Histogram {
        &self.inner.histogram
    }

    pub fn fields(&self) -> &HashSet<String> {
        &self.inner.fields
    }

    pub fn bitmaps(&self) -> &HashMap<String, Bitmap> {
        &self.inner.bitmaps
    }

    pub fn is_indexed(&self, field: &str) -> bool {
        // If the file does not contain the field, then it's not indexed
        if !self.inner.fields.contains(field) {
            return false;
        }

        // If the entries index contains a key that starts with the field
        // name, then we've have indexed all the values the field takes
        for key in self.inner.bitmaps.keys() {
            if key.starts_with(field) {
                return true;
            }
        }

        false
    }

    pub fn count_bitmap_entries_in_range(
        &self,
        bitmap: &Bitmap,
        start_time: u64,
        end_time: u64,
    ) -> Option<usize> {
        self.histogram()
            .count_bitmap_entries_in_range(bitmap, start_time, end_time)
    }

    /// Compresses the bincode serialized representation of the entries_index field using lz4.
    /// Returns the compressed bytes on success.
    pub fn compress_entries_index(&self) -> Vec<u8> {
        // Serialize the entries_index to bincode format
        let serialized = bincode::serialize(&self.inner.bitmaps).unwrap();

        // Compress the serialized data using lz4
        lz4::block::compress(&serialized, None, false).unwrap()
    }

    pub fn memory_size(&self) -> usize {
        bincode::serialized_size(&*self.inner).unwrap() as usize
    }
}

#[derive(Debug, Default)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
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

impl FileIndexer {
    pub fn capacity(&self) -> usize {
        let mut size = 0;

        size += self.source_timestamp_cursor_pairs.capacity()
            * std::mem::size_of::<(u64, InlinedCursor)>();

        size += self.entry_offsets.capacity() * std::mem::size_of::<NonZeroU64>();

        size += self.source_timestamp_entry_offset_pairs.capacity()
            * std::mem::size_of::<(u64, NonZeroU64)>();

        size +=
            self.realtime_entry_offset_pairs.capacity() * std::mem::size_of::<(u64, NonZeroU64)>();

        size += self.entry_indices.capacity() * std::mem::size_of::<u32>();

        size += self.entry_offset_index.capacity() * std::mem::size_of::<(NonZeroU64, u64)>();

        size
    }
    pub fn index(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        source_timestamp_field: Option<&[u8]>,
        field_names: &[&[u8]],
        bucket_duration: u64,
    ) -> Result<FileIndex> {
        if false {
            let n_entries = journal_file.journal_header_ref().n_entries as _;
            self.source_timestamp_cursor_pairs.reserve(n_entries);
            self.source_timestamp_entry_offset_pairs.reserve(n_entries);
            self.realtime_entry_offset_pairs.reserve(n_entries);
            self.entry_indices.reserve(n_entries / 2);
            self.entry_offsets.reserve(8);
            self.entry_offset_index.reserve(n_entries);
        } else {
            // let n_entries = journal_file.journal_header_ref().n_entries as _;
            self.source_timestamp_cursor_pairs = Vec::new();
            self.source_timestamp_entry_offset_pairs = Vec::new();
            self.realtime_entry_offset_pairs = Vec::new();
            self.entry_indices = Vec::new();
            self.entry_offsets = Vec::new();
            self.entry_offset_index = HashMap::default();
        }

        let fields = collect_file_fields(journal_file);

        let histogram =
            self.build_histogram(journal_file, source_timestamp_field, bucket_duration)?;
        let entries = self.build_entries_index(journal_file, field_names)?;

        let inner = FileIndexInner {
            fields,
            histogram,
            bitmaps: entries,
        };

        // let n_entries = journal_file.journal_header_ref().n_entries as _;
        self.source_timestamp_cursor_pairs = Vec::new();
        self.source_timestamp_entry_offset_pairs = Vec::new();
        self.realtime_entry_offset_pairs = Vec::new();
        self.entry_indices = Vec::new();
        self.entry_offsets = Vec::new();
        self.entry_offset_index = HashMap::default();

        Ok(FileIndex {
            inner: Arc::new(inner),
        })
    }

    fn build_entries_index(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        field_names: &[&[u8]],
    ) -> Result<HashMap<String, Bitmap>> {
        let mut entries_index = HashMap::default();

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

    fn collect_source_field_info(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        source_field_name: &[u8],
    ) -> Result<()> {
        let field_data_iterator = journal_file.field_data_objects(source_field_name)?;

        // Collect the inlined cursors of the source timestamp field
        self.source_timestamp_cursor_pairs.clear();
        for data_object_result in field_data_iterator {
            let Ok(data_object) = data_object_result else {
                warn!("loading data object failed");
                continue;
            };

            let Ok(source_timestamp) = parse_timestamp(source_field_name, &data_object) else {
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
        for (idx, (_, entry_offset)) in self.source_timestamp_entry_offset_pairs.iter().enumerate()
        {
            self.entry_offset_index.insert(*entry_offset, idx as _);
        }

        Ok(())
    }

    fn build_histogram(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        source_timestamp_field_name: Option<&[u8]>,
        bucket_duration: u64,
    ) -> Result<Histogram> {
        // Collect information from the source timestamp field
        if let Some(source_field_name) = source_timestamp_field_name {
            self.collect_source_field_info(journal_file, source_field_name)?;
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
        Ok(Histogram::from_timestamp_offset_pairs(
            bucket_duration,
            self.source_timestamp_entry_offset_pairs.as_slice(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to create a test histogram with known buckets
    ///
    /// Creates a histogram with:
    /// - bucket_duration: 60 seconds
    /// - Entries at indices 0-4 in bucket starting at time 0
    /// - Entries at indices 5-9 in bucket starting at time 60
    /// - Entries at indices 10-14 in bucket starting at time 120
    /// - Entries at indices 15-19 in bucket starting at time 180
    fn create_test_histogram() -> Histogram {
        let buckets = vec![
            Bucket {
                start_time: 0,
                count: 4, // entries 0-4 (5 entries)
            },
            Bucket {
                start_time: 60,
                count: 9, // entries 5-9 (5 entries)
            },
            Bucket {
                start_time: 120,
                count: 14, // entries 10-14 (5 entries)
            },
            Bucket {
                start_time: 180,
                count: 19, // entries 15-19 (5 entries)
            },
        ];

        Histogram {
            bucket_duration: 60,
            buckets,
        }
    }

    #[test]
    fn test_count_bitmap_entries_in_range_full_bucket() {
        let histogram = create_test_histogram();
        // Bitmap contains entries 5, 6, 7, 8, 9 (all in bucket starting at 60)
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7, 8, 9]).unwrap();

        // Query for the full bucket from 60 to 120
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 120);
        assert_eq!(count, Some(5));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_partial_match() {
        let histogram = create_test_histogram();
        // Bitmap contains some entries in bucket 60-120 and some in 120-180
        let bitmap = Bitmap::from_sorted_iter([7, 8, 9, 10, 11]).unwrap();

        // Query for bucket 60-120 should only count entries 7, 8, 9
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 120);
        assert_eq!(count, Some(3));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_multiple_buckets() {
        let histogram = create_test_histogram();
        // Bitmap spans multiple buckets
        let bitmap = Bitmap::from_sorted_iter([5, 6, 10, 11, 15, 16]).unwrap();

        // Query for buckets 60-180 (includes buckets at 60 and 120)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 180);
        assert_eq!(count, Some(4)); // 5, 6, 10, 11
    }

    #[test]
    fn test_count_bitmap_entries_in_range_no_matches() {
        let histogram = create_test_histogram();
        // Bitmap contains entries in bucket 0-60
        let bitmap = Bitmap::from_sorted_iter([0, 1, 2]).unwrap();

        // Query for bucket 120-180 should find no matches
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 120, 180);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_empty_bitmap() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::new();

        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 60);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_unaligned_start() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // Start time not aligned to bucket_duration (60)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 30, 120);
        assert_eq!(count, None);
    }

    #[test]
    fn test_count_bitmap_entries_in_range_unaligned_end() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // End time not aligned to bucket_duration (60)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 100);
        assert_eq!(count, None);
    }

    #[test]
    fn test_count_bitmap_entries_in_range_invalid_range() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // start >= end
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 120, 60);
        assert_eq!(count, None);

        // start == end
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 60, 60);
        assert_eq!(count, None);
    }

    #[test]
    fn test_count_bitmap_entries_in_range_outside_histogram() {
        let histogram = create_test_histogram();
        let bitmap = Bitmap::from_sorted_iter([5, 6, 7]).unwrap();

        // Range completely before histogram
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 60);
        // This will actually work since 0-60 is the first bucket
        assert!(count.is_some());

        // Range completely after histogram (histogram ends at 240)
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 240, 300);
        assert_eq!(count, Some(0));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_first_bucket() {
        let histogram = create_test_histogram();
        // Entries in first bucket (0-60)
        let bitmap = Bitmap::from_sorted_iter([0, 1, 2, 3, 4]).unwrap();

        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 60);
        assert_eq!(count, Some(5));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_last_bucket() {
        let histogram = create_test_histogram();
        // Entries in last bucket (180-240)
        let bitmap = Bitmap::from_sorted_iter([15, 16, 17, 18, 19]).unwrap();

        let count = histogram.count_bitmap_entries_in_range(&bitmap, 180, 240);
        assert_eq!(count, Some(5));
    }

    #[test]
    fn test_count_bitmap_entries_in_range_all_buckets() {
        let histogram = create_test_histogram();
        // Entries spanning all buckets
        let bitmap = Bitmap::from_sorted_iter([0, 5, 10, 15]).unwrap();

        // Query for entire histogram range
        let count = histogram.count_bitmap_entries_in_range(&bitmap, 0, 240);
        assert_eq!(count, Some(4));
    }

    #[test]
    fn test_histogram_properties() {
        let histogram = create_test_histogram();

        assert_eq!(histogram.start_time(), Some(0));
        assert_eq!(histogram.end_time(), Some(240)); // last bucket starts at 180, duration is 60
        assert_eq!(histogram.time_range(), Some((0, 240)));
        assert_eq!(histogram.duration(), Some(240));
        assert_eq!(histogram.len(), 4);
        assert!(!histogram.is_empty());
        assert_eq!(histogram.count(), 20); // 20 total entries (indices 0-19)
    }
}
