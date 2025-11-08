use super::{
    Bitmap, Histogram,
    field_types::{FieldName, FieldValuePair},
};
use crate::collections::{HashMap, HashSet};
use crate::error::{JournalError, Result};
use crate::file::{DataObject, HashableObject, JournalFile, Mmap, offset_array::InlinedCursor};
#[cfg(feature = "allocative")]
use allocative::Allocative;
use std::num::{NonZeroU32, NonZeroU64};
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

fn collect_file_fields(journal_file: &JournalFile<Mmap>) -> HashSet<FieldName> {
    let mut fields = HashSet::default();

    for value_guard in journal_file.fields() {
        let Ok(field) = value_guard else {
            error!("Failed to get collect file field");
            continue;
        };

        let payload = String::from_utf8_lossy(field.get_payload()).into_owned();
        if let Some(field_name) = FieldName::new(payload) {
            fields.insert(field_name);
        }
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
        source_timestamp_field: Option<&FieldName>,
        field_names: &[FieldName],
        bucket_duration: u32,
    ) -> Result<FileIndex> {
        // Get the File from the JournalFile
        let file = journal_file.file();

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

        // Capture indexing timestamp
        let indexed_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // Capture whether the file was online when indexed
        let was_online = journal_file.journal_header_ref().state == 1;

        // Collect all fields of the journal file
        let file_fields = collect_file_fields(journal_file);

        // Build the file histogram
        let histogram =
            self.build_histogram(journal_file, source_timestamp_field, bucket_duration)?;

        // Use the (timestamp, entry-offset) pairs to construct a vector that
        // will contain entry offsets sorted by time
        let entry_offsets = self
            .source_timestamp_entry_offset_pairs
            .iter()
            .map(|(_, entry_offset)| entry_offset.get() as u32)
            .collect();

        // Create the bitmaps for field=value pairs
        let entries = self.build_entries_index(journal_file, field_names)?;

        // Convert field_names to HashSet<FieldName> for indexed_fields
        let indexed_fields: HashSet<FieldName> = field_names.iter().cloned().collect();

        // let n_entries = journal_file.journal_header_ref().n_entries as _;
        self.source_timestamp_cursor_pairs = Vec::new();
        self.source_timestamp_entry_offset_pairs = Vec::new();
        self.realtime_entry_offset_pairs = Vec::new();
        self.entry_indices = Vec::new();
        self.entry_offsets = Vec::new();
        self.entry_offset_index = HashMap::default();

        Ok(FileIndex::new(
            file.clone(),
            indexed_at,
            was_online,
            histogram,
            entry_offsets,
            file_fields,
            indexed_fields,
            entries,
        ))
    }

    fn build_entries_index(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        field_names: &[FieldName],
    ) -> Result<HashMap<FieldValuePair, Bitmap>> {
        let mut entries_index = HashMap::default();

        for field_name in field_names {
            // Get the data object iterator for this field
            let field_data_iterator = match journal_file.field_data_objects(field_name.as_bytes()) {
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

                // Parse the payload into a FieldValuePair (format is "FIELD=value")
                let Some(pair) = FieldValuePair::parse(&data_payload) else {
                    warn!("Invalid field=value format: {}", data_payload);
                    continue;
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
                        debug_assert!(false, "missing entry offset from index");
                        continue;
                    };
                    self.entry_indices.push(*entry_index as u32);
                }
                self.entry_indices.sort_unstable();

                // Create the bitmap for the entry indices
                let mut bitmap =
                    Bitmap::from_sorted_iter(self.entry_indices.iter().copied()).unwrap();
                bitmap.optimize();

                entries_index.insert(pair, bitmap);
            }
        }

        Ok(entries_index)
    }

    fn collect_source_field_info(
        &mut self,
        journal_file: &JournalFile<Mmap>,
        source_field_name: &[u8],
    ) -> Result<()> {
        // Create an iterator over all the different values the field can take
        let field_data_iterator = journal_file.field_data_objects(source_field_name)?;

        // Collect all the inlined cursors of the source timestamp field
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

        // Collect all the [source_timestamp, entry-offset] pairs
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
        // Sort the [source_timestamp, entry-offset] pairs
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
        source_timestamp_field_name: Option<&FieldName>,
        bucket_duration: u32,
    ) -> Result<Histogram> {
        // Collect information from the source timestamp field
        if let Some(source_field_name) = source_timestamp_field_name {
            self.collect_source_field_info(journal_file, source_field_name.as_bytes())?;
        }

        // At this point:
        //
        // - `self.source_timestamp_entry_offset_pairs`: contains a vector of
        //   (timestamp, entry-offset) pairs sorted by time,
        // - `self.entry_offset_index`: maps an entry offset to a number
        //   with the following invariant:
        //      if (e1.offset < e2.offset) then e1.number < e2.number.

        // Load the global entry offset array from the file
        self.entry_offsets.clear();
        journal_file.entry_offsets(&mut self.entry_offsets)?;

        // Iterate the global entry offset array of the journal file and
        // find entries for which we could not collect a timestamp. In this
        // case, fall-back to using the journal file's realtime timestamp.
        self.realtime_entry_offset_pairs.clear();
        for entry_offset in self.entry_offsets.iter() {
            if self.entry_offset_index.contains_key(entry_offset) {
                // We have the timestamp of this entry offset
                continue;
            }

            // We don't know the timestamp of this entry offset, use
            // the journal's file realtime timestamp.

            let timestamp = {
                let entry = journal_file.entry_ref(*entry_offset)?;
                entry.header.realtime
            };

            // Add the new (timestamp, entry-offset) pair
            self.realtime_entry_offset_pairs
                .push((timestamp, *entry_offset));
        }

        // At this point:
        //
        // - `self.realtime_entry_offset_pairs`: contains (timestamp, entry-offset)
        // pairs of all the entries for which we had to use the journal file's
        // realtime timestamp.

        // Reconstruct our indexes if we have entries whose time does not
        // come from the source timestamp
        if !self.realtime_entry_offset_pairs.is_empty() {
            // Extend the vector holding pairs collected from the source timestamp
            // with the pairs collected from the realtime timestamp and
            // sorte it by time again.
            self.source_timestamp_entry_offset_pairs
                .append(&mut self.realtime_entry_offset_pairs);
            self.source_timestamp_entry_offset_pairs.sort_unstable();

            // We need to rebuild the `self.entry_offset_index` because
            // we found entry offsets from the global entry offset array
            // whose timestamp is assume to be equal to the realtime timestamp
            // of the journal file
            self.entry_offset_index.clear();
            for (idx, (_, entry_offset)) in
                self.source_timestamp_entry_offset_pairs.iter().enumerate()
            {
                self.entry_offset_index.insert(*entry_offset, idx as _);
            }
        }

        // At this point, we have information about the order and the time
        // of all entries in the journal file:
        //
        // - `self.source_timestamp_entry_offset_pairs`: contains a vector of
        //   (timestamp, entry-offset) pairs sorted by time,
        // - `self.entry_offset_index`: maps an entry offset to a number
        //   with the following invariant:
        //      if (e1.offset < e2.offset) then e1.number < e2.number.
        //
        // We can proceed with building the histogram

        let Some(bucket_duration) = NonZeroU32::new(bucket_duration) else {
            return Err(JournalError::InvalidMagicNumber);
        };

        // Now we can build the file histogram
        Ok(Histogram::from_timestamp_offset_pairs(
            bucket_duration,
            self.source_timestamp_entry_offset_pairs.as_slice(),
        ))
    }
}
