use super::{Bitmap, Histogram};
use crate::collections::{HashMap, HashSet};
use crate::error::{JournalError, Result};
use crate::file::{DataObject, HashableObject, JournalFile, Mmap, offset_array::InlinedCursor};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use std::num::NonZeroU64;
use tracing::{error, warn};

use super::file_index::FileIndex;

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

        let file_fields = collect_file_fields(journal_file);

        let histogram =
            self.build_histogram(journal_file, source_timestamp_field, bucket_duration)?;
        let entries = self.build_entries_index(journal_file, field_names)?;

        // Convert field_names to HashSet<String> for indexed_fields
        let indexed_fields: HashSet<String> = field_names
            .iter()
            .filter_map(|field_name| std::str::from_utf8(field_name).ok())
            .map(|s| s.to_string())
            .collect();

        // let n_entries = journal_file.journal_header_ref().n_entries as _;
        self.source_timestamp_cursor_pairs = Vec::new();
        self.source_timestamp_entry_offset_pairs = Vec::new();
        self.realtime_entry_offset_pairs = Vec::new();
        self.entry_indices = Vec::new();
        self.entry_offsets = Vec::new();
        self.entry_offset_index = HashMap::default();

        Ok(FileIndex {
            histogram,
            file_fields,
            indexed_fields,
            bitmaps: entries,
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

        let Some(bucket_duration) = NonZeroU64::new(bucket_duration) else {
            return Err(JournalError::InvalidMagicNumber);
        };

        // Now we can build the file histogram
        Ok(Histogram::from_timestamp_offset_pairs(
            bucket_duration,
            self.source_timestamp_entry_offset_pairs.as_slice(),
        ))
    }
}
