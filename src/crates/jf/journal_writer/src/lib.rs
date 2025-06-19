#![allow(unused_imports, dead_code)]

use error::{JournalError, Result};
use journal_file::{
    journal_hash_data, CompactEntryItem, DataHashTable, DataObject, DataObjectHeader,
    DataPayloadType, EntryObject, EntryObjectHeader, FieldHashTable, FieldObject,
    FieldObjectHeader, HashItem, HashTable, HashTableMut, HashableObject, HashableObjectMut,
    HeaderIncompatibleFlags, JournalFile, JournalHeader, JournalState, ObjectHeader, ObjectType,
    RegularEntryItem,
};
use memmap2::MmapMut;
use rand::{seq::IndexedRandom, Rng};
use std::num::{NonZeroU64, NonZeroUsize};
use std::path::Path;
use window_manager::MemoryMapMut;
use zerocopy::{FromBytes, IntoBytes};

const OBJECT_ALIGNMENT: u64 = 8;

#[derive(Debug, Clone, Copy)]
struct EntryItem {
    offset: NonZeroU64,
    hash: u64,
}

pub struct JournalWriter {
    tail_offset: NonZeroU64,
    next_seqnum: u64,
    num_written_objects: u64,
    entry_items: Vec<EntryItem>,
}

impl JournalWriter {
    pub fn new(journal_file: &mut JournalFile<MmapMut>) -> Result<Self> {
        let (tail_offset, next_seqnum) = {
            let header = journal_file.journal_header_ref();

            let Some(tail_object_offset) = header.tail_object_offset else {
                return Err(JournalError::InvalidMagicNumber);
            };

            let tail_object = journal_file.object_header_ref(tail_object_offset)?;

            (
                tail_object_offset.saturating_add(tail_object.size),
                header.tail_entry_seqnum + 1,
            )
        };

        Ok(Self {
            tail_offset,
            next_seqnum,
            num_written_objects: 0,
            entry_items: Vec::with_capacity(128),
        })
    }

    fn add_data(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        payload: &[u8],
    ) -> Result<NonZeroU64> {
        let hash = journal_file.hash(payload);

        match journal_file.find_data_offset(hash, payload) {
            Ok(Some(data_offset)) => Ok(data_offset),
            Ok(None) => {
                // We will have to write the new data object at the current
                // tail offset
                let data_offset = self.tail_offset;
                let data_size = {
                    let mut data_guard =
                        journal_file.data_mut(data_offset, Some(payload.len() as u64))?;

                    data_guard.header.hash = hash;
                    data_guard.set_payload(payload);
                    data_guard.header.object_header.aligned_size()
                };

                // Update our tail offset
                self.num_written_objects += 1;
                self.tail_offset = self.tail_offset.saturating_add(data_size);

                // Update hash table
                journal_file.data_hash_table_set_tail_offset(hash, data_offset)?;

                // Add the field object, if we have any
                if let Some(equals_pos) = payload.iter().position(|&b| b == b'=') {
                    let field_offset = self.add_field(journal_file, &payload[..equals_pos])?;

                    // Link data object to the linked-list
                    {
                        let head_data_offset = {
                            let field_guard = journal_file.field_ref(field_offset)?;
                            field_guard.header.head_data_offset
                        };

                        let mut data_guard = journal_file.data_mut(data_offset, None)?;
                        data_guard.header.next_field_offset = head_data_offset;
                    }

                    // Link field to the head of the linked list
                    {
                        let mut field_guard = journal_file.field_mut(field_offset, None)?;
                        field_guard.header.head_data_offset = Some(data_offset);
                    };
                }

                Ok(data_offset)
            }
            Err(e) => Err(e),
        }
    }

    fn add_field(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        payload: &[u8],
    ) -> Result<NonZeroU64> {
        let hash = journal_file.hash(payload);

        match journal_file.find_field_offset(hash, payload) {
            Ok(Some(field_offset)) => Ok(field_offset),
            Ok(None) => {
                // We will have to write the new field object at the current
                // tail offset
                let field_offset = self.tail_offset;
                let field_size = {
                    let mut field_guard =
                        journal_file.field_mut(field_offset, Some(payload.len() as u64))?;

                    field_guard.header.hash = hash;
                    field_guard.set_payload(payload);
                    field_guard.header.object_header.aligned_size()
                };

                // Update our tail offset
                self.num_written_objects += 1;
                self.tail_offset = self.tail_offset.saturating_add(field_size);

                // Update hash table
                journal_file.field_hash_table_set_tail_offset(hash, field_offset)?;

                // Return the offset where we wrote the newly added data object
                Ok(field_offset)
            }
            Err(e) => Err(e),
        }
    }

    pub fn add_entry(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        items: &[&[u8]],
        realtime: u64,
        monotonic: u64,
        boot_id: [u8; 16],
    ) -> Result<()> {
        let header = journal_file.journal_header_ref();
        assert!(header.has_incompatible_flag(HeaderIncompatibleFlags::KeyedHash));

        // Write the data/field objects while computing the entry's xor-hash
        // and storing each data object's offset/hash
        let mut xor_hash = 0;
        {
            self.entry_items.clear();
            for payload in items {
                let offset = self.add_data(journal_file, payload)?;
                let hash = {
                    let data_guard = journal_file.data_ref(offset)?;
                    data_guard.hash()
                };

                let entry_item = EntryItem { offset, hash };
                self.entry_items.push(entry_item);

                xor_hash ^= journal_hash_data(payload, true, None);
            }

            self.entry_items
                .sort_unstable_by(|a, b| a.offset.cmp(&b.offset));
            self.entry_items.dedup_by(|a, b| a.offset == b.offset);
        }

        // write the entry itself
        let entry_offset = self.tail_offset;
        let entry_size = {
            let size = Some(self.entry_items.len() as u64 * 16);
            let mut entry_guard = journal_file.entry_mut(entry_offset, size)?;

            entry_guard.header.seqnum = self.next_seqnum;
            entry_guard.header.xor_hash = xor_hash;
            entry_guard.header.boot_id = boot_id;
            entry_guard.header.monotonic = monotonic;
            entry_guard.header.realtime = realtime;

            // set each entry item
            for (index, entry_item) in self.entry_items.iter().enumerate() {
                entry_guard
                    .items
                    .set(index, entry_item.offset, Some(entry_item.hash));
            }

            entry_guard.header.object_header.aligned_size()
        };

        // Update state
        self.next_seqnum += 1;
        self.num_written_objects += 1;
        self.tail_offset = NonZeroU64::new(self.tail_offset.get() + entry_size)
            .ok_or(JournalError::InvalidOffset)?;

        self.append_to_entry_array(journal_file, entry_offset)?;
        for entry_item_index in 0..self.entry_items.len() {
            self.link_data_to_entry(journal_file, entry_offset, entry_item_index)?;
        }

        // Update header
        {
            let header = journal_file.journal_header_mut();

            // Update counts
            header.n_entries += 1;
            header.n_objects += self.num_written_objects;

            header.tail_object_offset = Some(entry_offset);

            // Update sequence numbers
            header.tail_entry_seqnum = self.next_seqnum - 1;
            if header.head_entry_seqnum == 0 {
                header.head_entry_seqnum = self.next_seqnum - 1;
            }

            // Update timestamps
            header.tail_entry_realtime = realtime;
            if header.head_entry_realtime == 0 {
                header.head_entry_realtime = realtime;
            }

            header.tail_entry_monotonic = monotonic;
            header.tail_entry_boot_id = boot_id;

            // Update arena size
            header.arena_size = self.tail_offset.get() - header.header_size;
        }

        Ok(())
    }

    fn allocate_new_array(
        &mut self,
        journal_file: &JournalFile<MmapMut>,
        capacity: NonZeroU64,
    ) -> Result<NonZeroU64> {
        // let new_capacity = previous_capacity.saturating_mul(NonZeroU64::new(2).unwrap());

        let array_offset = self.tail_offset;
        let array_size = {
            let array_guard = journal_file.offset_array_mut(array_offset, Some(capacity))?;

            array_guard.header.object_header.aligned_size()
        };

        // Update tail offset
        self.num_written_objects += 1;
        self.tail_offset = self.tail_offset.saturating_add(array_size);

        Ok(array_offset)
    }

    fn append_to_entry_array(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        entry_offset: NonZeroU64,
    ) -> Result<()> {
        let header = journal_file.journal_header_ref();

        if header.entry_array_offset == 0 {
            let array_offset =
                self.allocate_new_array(journal_file, NonZeroU64::new(4096).unwrap())?;
            let mut array_guard = journal_file.offset_array_mut(array_offset, None)?;
            array_guard.set(0, entry_offset)?;
        } else {
            let tail_node = {
                let entry_list = journal_file
                    .entry_list()
                    .ok_or(JournalError::EmptyOffsetArrayList)?;
                entry_list.tail(journal_file)?
            };

            if tail_node.len() < tail_node.capacity() {
                let mut array_guard = journal_file.offset_array_mut(tail_node.offset(), None)?;
                array_guard.set(tail_node.len().get(), entry_offset)?;
            } else {
                let new_array_offset = {
                    let new_capacity = tail_node.capacity().get().saturating_mul(2) as u64;
                    let new_array_offset = self
                        .allocate_new_array(journal_file, NonZeroU64::new(new_capacity).unwrap())?;
                    let mut array_guard = journal_file.offset_array_mut(new_array_offset, None)?;
                    array_guard.set(0, entry_offset)?;

                    new_array_offset
                };

                // Link the old tail to the new array
                {
                    let mut array_guard =
                        journal_file.offset_array_mut(tail_node.offset(), None)?;
                    array_guard.header.next_offset_array = Some(new_array_offset);
                }
            }
        }

        journal_file.journal_header_mut().n_entries += 1;

        Ok(())
    }

    fn append_to_data_entry_array(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        mut array_offset: NonZeroU64,
        entry_offset: NonZeroU64,
        current_count: u64,
    ) -> Result<()> {
        // Navigate to the tail of the array chain
        let mut current_index = 0u64;
        let mut tail_offset = array_offset;

        loop {
            let array_guard = journal_file.offset_array_ref(array_offset)?;
            let capacity = array_guard.capacity() as u64;

            if current_index + capacity >= current_count {
                // This is the tail array
                tail_offset = array_offset;
                break;
            }

            current_index += capacity;

            if let Some(next_offset) = array_guard.header.next_offset_array {
                array_offset = next_offset;
            } else {
                // This shouldn't happen if counts are correct
                return Err(JournalError::InvalidOffsetArrayOffset);
            }
        }

        // Try to add to the tail array
        let tail_guard = journal_file.offset_array_ref(tail_offset)?;
        let tail_capacity = tail_guard.capacity() as u64;
        let entries_in_tail = current_count - current_index;

        if entries_in_tail < tail_capacity {
            // There's space in the tail array
            drop(tail_guard);
            let mut tail_guard = journal_file.offset_array_mut(tail_offset, None)?;
            tail_guard.set(entries_in_tail as usize, entry_offset)?;
        } else {
            // Need to create a new array
            let new_capacity = NonZeroU64::new(tail_capacity * 2).unwrap(); // Double the size
            let new_array_offset = self.allocate_new_array(journal_file, new_capacity)?;

            // Link the old tail to the new array
            drop(tail_guard);
            let mut tail_guard = journal_file.offset_array_mut(tail_offset, None)?;
            tail_guard.header.next_offset_array = Some(new_array_offset);
            drop(tail_guard);

            // Add entry to the new array
            let mut new_array_guard = journal_file.offset_array_mut(new_array_offset, None)?;
            new_array_guard.set(0, entry_offset)?;
        }

        Ok(())
    }

    fn link_data_to_entry(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        entry_offset: NonZeroU64,
        entry_item_index: usize,
    ) -> Result<()> {
        let data_offset = self.entry_items[entry_item_index].offset;
        let mut data_guard = journal_file.data_mut(data_offset, None)?;

        match data_guard.header.n_entries {
            None => {
                data_guard.header.entry_offset = Some(entry_offset);
                data_guard.header.n_entries = NonZeroU64::new(1);
            }
            Some(n_entries) => {
                match n_entries.get() {
                    0 => {
                        unreachable!();
                    }
                    1 => {
                        drop(data_guard);

                        // Create new entry array with initial capacity
                        let array_capacity = NonZeroU64::new(64).unwrap();
                        let array_offset = self.allocate_new_array(journal_file, array_capacity)?;

                        // Load new array and set its first entry offset
                        {
                            let mut array_guard =
                                journal_file.offset_array_mut(array_offset, None)?;
                            array_guard.set(0, entry_offset)?;
                        }

                        // Update data object to point to the array
                        let mut data_guard = journal_file.data_mut(data_offset, None)?;
                        data_guard.header.entry_array_offset = Some(array_offset);
                        data_guard.header.n_entries = NonZeroU64::new(2);
                    }
                    x => {
                        // There's already an entry array, append to it
                        let current_count = x - 1;
                        let array_offset = data_guard.header.entry_array_offset.unwrap();

                        // Drop the data guard to avoid borrow conflicts
                        drop(data_guard);

                        // Find the tail of the entry array chain and append
                        self.append_to_data_entry_array(
                            journal_file,
                            array_offset,
                            entry_offset,
                            current_count,
                        )?;

                        // Update the count
                        let mut data_guard = journal_file.data_mut(data_offset, None)?;
                        data_guard.header.n_entries = NonZeroU64::new(current_count + 1);
                    }
                }
            }
        }

        Ok(())
    }
}
