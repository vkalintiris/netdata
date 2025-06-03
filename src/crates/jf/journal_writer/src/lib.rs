#![allow(unused_imports, dead_code)]

use error::{JournalError, Result};
use journal_file::{
    journal_hash_data, CompactEntryItem, DataObject, DataObjectHeader, DataPayloadType,
    EntryObject, EntryObjectHeader, FieldObject, FieldObjectHeader, HashItem, HashTableObject,
    HashableObjectMut, HeaderIncompatibleFlags, JournalFile, JournalHeader, JournalState,
    ObjectHeader, ObjectType, RegularEntryItem,
};
use memmap2::MmapMut;
use rand::{seq::IndexedRandom, Rng};
use std::num::{NonZeroI128, NonZeroU64};
use std::path::Path;
use window_manager::MemoryMapMut;
use zerocopy::{FromBytes, IntoBytes};

const OBJECT_ALIGNMENT: u64 = 8;

pub struct JournalWriter {
    tail_offset: NonZeroU64,
    offsets_buffer: Vec<NonZeroU64>,
    hash_buffer: Vec<u64>,
}

impl JournalWriter {
    pub fn new(journal_file: &mut JournalFile<MmapMut>) -> Result<Self> {
        let tail_offset = {
            let header = journal_file.journal_header_ref();

            let Some(tail_object_offset) = header.tail_object_offset else {
                return Err(JournalError::InvalidMagicNumber);
            };

            let tail_object = journal_file.object_header_ref(tail_object_offset)?;
            tail_object_offset.saturating_add(tail_object.size)
        };

        Ok(Self {
            tail_offset,
            offsets_buffer: Vec::with_capacity(128),
            hash_buffer: Vec::with_capacity(128),
        })
    }

    pub fn add_data(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        data: &[u8],
    ) -> Result<NonZeroU64> {
        let hash = journal_file.hash(data);

        match journal_file.find_data_offset(hash, data) {
            Ok(Some(data_offset)) => Ok(data_offset),
            Ok(None) => {
                // Write new data object
                let advance_tail_offset = {
                    let mut data_object =
                        journal_file.data_mut(self.tail_offset, Some(data.len() as u64))?;

                    data_object.header.hash = hash;
                    data_object.set_payload(data);
                    data_object.header.object_header.aligned_size()
                };

                // Update tail object's next_hash_offset
                {
                    let dht = journal_file
                        .data_hash_table_ref()
                        .ok_or(JournalError::MissingHashTable)?;

                    let hash_item = dht.hash_item_ref(hash);
                    if let Some(tail_hash_offset) = hash_item.tail_hash_offset {
                        let mut tail_object = journal_file.data_mut(tail_hash_offset, None)?;
                        tail_object.set_next_hash_offset(self.tail_offset);
                    }
                };

                // Update the hash table bucket
                {
                    let mut dht = journal_file
                        .data_hash_table_mut()
                        .ok_or(JournalError::MissingHashTable)?;

                    let hash_item = dht.hash_item_mut(hash);
                    if hash_item.head_hash_offset.is_none() {
                        hash_item.head_hash_offset = Some(self.tail_offset);
                    }
                    hash_item.tail_hash_offset = Some(self.tail_offset);
                }

                self.tail_offset = self.tail_offset.saturating_add(advance_tail_offset);
                Ok(self.tail_offset)
            }
            Err(e) => Err(e),
        }
    }

    pub fn add_entry(
        &mut self,
        journal_file: &mut JournalFile<MmapMut>,
        items: &[&[u8]],
        // realtime: u64,
        // monotonic: u64,
        // boot_id: [u8; 16],
    ) -> Result<u64> {
        let header = journal_file.journal_header_ref();

        let is_keyed_hash = header.has_incompatible_flag(HeaderIncompatibleFlags::KeyedHash);

        let file_id = if is_keyed_hash {
            Some(&header.file_id)
        } else {
            None
        };

        self.hash_buffer.clear();
        self.hash_buffer.extend(
            items
                .iter()
                .map(|item| journal_hash_data(item, is_keyed_hash, file_id)),
        );

        self.offsets_buffer.clear();
        for payload in items.iter() {
            let data_offset = self.add_data(journal_file, payload)?;
            self.offsets_buffer.push(data_offset);
        }

        Ok(0)
    }
}
