use super::{
    Bitmap, Histogram,
    field_types::{FieldName, FieldValuePair},
};
use crate::collections::{HashMap, HashSet};
use crate::error::{JournalError, Result};
use crate::file::{JournalFile, Mmap};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use bincode;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU64;

/// Direction for iterating through entries
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// Iterate forward in time (from older to newer entries)
    Forward,
    /// Iterate backward in time (from newer to older entries)
    Backward,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "allocative", derive(Allocative))]
pub struct FileIndex {
    // The journal file's histogram
    pub histogram: Histogram,

    // Entry offsets sorted by time
    pub entry_offsets: Vec<u32>,

    // Set of fields in the file
    pub file_fields: HashSet<FieldName>,

    // Set of fields that were requested to be indexed
    pub indexed_fields: HashSet<FieldName>,

    // Bitmap for each indexed field=value pair
    pub bitmaps: HashMap<FieldValuePair, Bitmap>,
}

impl FileIndex {
    pub fn new(
        histogram: Histogram,
        entry_offsets: Vec<u32>,
        fields: HashSet<FieldName>,
        indexed_fields: HashSet<FieldName>,
        bitmaps: HashMap<FieldValuePair, Bitmap>,
    ) -> Self {
        Self {
            histogram,
            entry_offsets,
            file_fields: fields,
            indexed_fields,
            bitmaps,
        }
    }

    pub fn bucket_duration(&self) -> u32 {
        self.histogram.bucket_duration.get()
    }

    pub fn histogram(&self) -> &Histogram {
        &self.histogram
    }

    /// Get all field names present in this file.
    pub fn fields(&self) -> &HashSet<FieldName> {
        &self.file_fields
    }

    /// Get all indexed field=value pairs with their bitmaps.
    pub fn bitmaps(&self) -> &HashMap<FieldValuePair, Bitmap> {
        &self.bitmaps
    }

    /// Check if a field is indexed.
    pub fn is_indexed(&self, field: &FieldName) -> bool {
        self.indexed_fields.contains(field)
    }

    pub fn count_bitmap_entries_in_range(
        &self,
        bitmap: &Bitmap,
        start_time: u32,
        end_time: u32,
    ) -> Option<usize> {
        self.histogram()
            .count_bitmap_entries_in_range(bitmap, start_time, end_time)
    }

    /// Compresses the bincode serialized representation of the entries_index field using lz4.
    /// Returns the compressed bytes on success.
    pub fn compress_entries_index(&self) -> Vec<u8> {
        // Serialize the entries_index to bincode format
        let serialized = bincode::serialize(&self.bitmaps).unwrap();

        // Compress the serialized data using lz4
        lz4::block::compress(&serialized, None, false).unwrap()
    }

    pub fn memory_size(&self) -> usize {
        bincode::serialized_size(self).unwrap() as usize
    }
}

/// Get the timestamp for an entry at the given offset.
///
/// Attempts to read the source_timestamp_field from the entry's data objects.
/// Falls back to the entry's realtime timestamp if the field is not found.
fn get_entry_timestamp(
    journal_file: &JournalFile<Mmap>,
    source_timestamp_field: Option<&super::FieldName>,
    entry_offset: NonZeroU64,
) -> Result<u64> {
    // Try to read the source timestamp field if specified
    if let Some(field_name) = source_timestamp_field {
        if let Ok(timestamp) = get_timestamp_field(journal_file, field_name, entry_offset) {
            return Ok(timestamp);
        }
    }

    // Fall back to realtime timestamp
    let entry = journal_file.entry_ref(entry_offset)?;
    Ok(entry.header.realtime)
}

/// Read a timestamp field from an entry's data objects.
fn get_timestamp_field(
    journal_file: &JournalFile<Mmap>,
    field_name: &super::FieldName,
    entry_offset: NonZeroU64,
) -> Result<u64> {
    let field_bytes = field_name.as_bytes();
    let data_iter = journal_file.entry_data_objects(entry_offset)?;

    for data_result in data_iter {
        let data_object = data_result?;
        let payload = data_object.payload_bytes();

        // Check if this data object is our timestamp field
        if payload.starts_with(field_bytes) && payload.len() > field_bytes.len() + 1 {
            // Parse the timestamp value after "FIELD="
            let timestamp_str = std::str::from_utf8(&payload[field_bytes.len() + 1..])
                .map_err(|_| JournalError::InvalidField)?;

            return timestamp_str
                .parse::<u64>()
                .map_err(|_| JournalError::InvalidField);
        }
    }

    Err(JournalError::InvalidField)
}

// Find the partition point of entries based on a predicate function
fn partition_point_entries<F>(
    entry_offsets: &[NonZeroU64],
    left: usize,
    right: usize,
    predicate: F,
) -> Result<usize>
where
    F: Fn(NonZeroU64) -> Result<bool>,
{
    let mut left = left;
    let mut right = right;

    debug_assert!(left <= right);
    debug_assert!(right <= entry_offsets.len());

    while left != right {
        let mid = left.midpoint(right);

        if predicate(entry_offsets[mid])? {
            left = mid + 1;
        } else {
            right = mid;
        }
    }

    Ok(left)
}

impl FileIndex {
    /// Retrieve sorted (timestamp, entry offset) pairs with filtering.
    ///
    /// This method efficiently retrieves journal entries based on a timestamp anchor,
    /// direction, and optional filter. It uses binary search (partition point) to find
    /// the starting position, then iterates in the specified direction.
    ///
    /// # Arguments
    ///
    /// * `journal_file` - The journal file to read timestamps and entries from
    /// * `source_timestamp_field` - The timestamp field name used when building this index.
    ///   If provided, timestamps are read from this field in entry data objects, falling
    ///   back to the entry's realtime timestamp if the field is not present.
    /// * `filter` - Optional filter expression to match entries against (e.g., PRIORITY=3)
    /// * `anchor_timestamp` - Starting point timestamp in microseconds
    /// * `direction` - Whether to iterate forward or backward in time:
    ///   - `Direction::Forward`: Returns entries with timestamp >= anchor_timestamp
    ///   - `Direction::Backward`: Returns entries with timestamp <= anchor_timestamp
    /// * `limit` - Maximum number of entries to return
    ///
    /// # Returns
    ///
    /// A vector of (timestamp, entry_offset) pairs sorted by time according to direction:
    /// - Forward: Returns entries in ascending time order (oldest to newest after anchor)
    /// - Backward: Returns entries in descending time order (newest to oldest before/at anchor)
    ///
    /// The vector length will not exceed `limit`. Returns an empty vector if no entries
    /// match the criteria or if limit is 0.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use journal_file::index::{FileIndex, Filter, Direction, FieldName, FieldValuePair};
    ///
    /// // Get up to 100 error entries after timestamp 1000000000
    /// let source_field = FieldName::new("_SOURCE_REALTIME_TIMESTAMP".to_string()).unwrap();
    /// let filter = Filter::match_field_value_pair(
    ///     FieldValuePair::parse("PRIORITY=3").unwrap()
    /// );
    /// let results = file_index.retrieve_sorted_entries(
    ///     &journal_file,
    ///     Some(&source_field),
    ///     Some(&filter),
    ///     1000000000,
    ///     Direction::Forward,
    ///     100
    /// )?;
    ///
    /// for (timestamp, offset) in results {
    ///     println!("Entry at {} with offset {}", timestamp, offset);
    /// }
    /// ```
    pub fn retrieve_sorted_entries(
        &self,
        journal_file: &JournalFile<Mmap>,
        source_timestamp_field: Option<&super::FieldName>,
        filter: Option<&super::Filter>,
        anchor_timestamp: u64,
        direction: Direction,
        limit: usize,
    ) -> Result<Vec<(u64, u32)>> {
        // FIXME/TODO: using just the (anchor_timestamp, limit) is not good enough,
        // we need a `skip` argument as well. This would handle the case where
        // we have more than `limit` entries with the same timestamp. Highly
        // unlikely, but we need UI to send this as well.

        if limit == 0 {
            return Ok(Vec::new());
        }

        // Use filter's bitmap or one that fully covers all entries
        let bitmap = filter
            .map(|f| f.resolve(self).evaluate())
            .unwrap_or_else(|| Bitmap::insert_range(0..self.entry_offsets.len() as u32));

        if bitmap.is_empty() {
            // Nothing matches
            return Ok(Vec::new());
        }

        // Collect the entry offsets in the bitmap
        // TODO: How should we handle zero offsets?
        let entry_offsets: Vec<_> = bitmap
            .iter()
            .map(|idx| self.entry_offsets[idx as usize])
            .filter(|offset| *offset != 0)
            .map(|x| NonZeroU64::new(x as u64).unwrap())
            .collect();

        let mut results = Vec::with_capacity(limit.min(entry_offsets.len()));

        match direction {
            Direction::Forward => {
                // Find the partition point: first index where timestamp >= anchor_timestamp
                // Predicate returns true while timestamp < anchor_timestamp
                // Result is the index of the first entry with timestamp >= anchor_timestamp
                let start_idx = partition_point_entries(
                    &entry_offsets,
                    0,
                    entry_offsets.len(),
                    |entry_offset| {
                        let entry_timestamp = get_entry_timestamp(
                            journal_file,
                            source_timestamp_field,
                            entry_offset,
                        )?;
                        Ok(entry_timestamp < anchor_timestamp)
                    },
                )?;

                // Edge cases for forward iteration:
                // - start_idx == 0: anchor is <= all entries, start from first entry
                // - start_idx == len: anchor is > all entries, no results
                // - Otherwise: start from entry at start_idx (first entry >= anchor)

                for &entry_offset in entry_offsets.iter().skip(start_idx).take(limit) {
                    let timestamp =
                        get_entry_timestamp(journal_file, source_timestamp_field, entry_offset)?;
                    results.push((timestamp, entry_offset.get() as u32));
                }
            }
            Direction::Backward => {
                // Find the partition point: first index where timestamp > anchor_timestamp
                // We want the LAST entry with timestamp <= anchor_timestamp
                // which is at index (partition_point - 1)
                let partition_idx = partition_point_entries(
                    &entry_offsets,
                    0,
                    entry_offsets.len(),
                    |entry_offset| {
                        let entry_timestamp = get_entry_timestamp(
                            journal_file,
                            source_timestamp_field,
                            entry_offset,
                        )?;
                        Ok(entry_timestamp <= anchor_timestamp)
                    },
                )?;

                // Edge cases for backward iteration:
                // - partition_idx == 0: all entries are > anchor, no results
                // - partition_idx == len: anchor is >= all entries, start from last entry
                // - Otherwise: start from entry at (partition_idx - 1), last entry <= anchor

                if partition_idx == 0 {
                    // All entries have timestamp > anchor, no results
                    return Ok(results);
                }

                // Start from the last entry <= anchor (at partition_idx - 1)
                // and iterate backwards, taking up to `limit` entries
                let start_idx = partition_idx - 1;

                // Iterate backwards: from start_idx down to 0
                for i in (0..=start_idx).rev().take(limit) {
                    let entry_offset = entry_offsets[i];
                    let timestamp =
                        get_entry_timestamp(journal_file, source_timestamp_field, entry_offset)?;
                    results.push((timestamp, entry_offset.get() as u32));
                }
            }
        }

        Ok(results)
    }
}
