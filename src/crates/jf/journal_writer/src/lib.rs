#![allow(unused_imports, dead_code)]

use error::{JournalError, Result};
use journal_file::{
    journal_hash_data, CompactEntryItem, DataObject, DataObjectHeader, DataPayloadType,
    EntryObject, EntryObjectHeader, FieldObject, FieldObjectHeader, HashItem, HashTableObject,
    HeaderIncompatibleFlags, JournalFile, JournalHeader, JournalState, ObjectHeader, ObjectType,
    RegularEntryItem,
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

            println!("header: {:#?}", header);

            let Some(position) = header.tail_object_offset else {
                panic!("WTF1");
                return Err(JournalError::InvalidMagicNumber);
            };

            let Some(tail_offset) = NonZeroU64::new(journal_file.object_header_ref(position)?.size)
            else {
                panic!("WTF2");
                return Err(JournalError::InvalidMagicNumber);
            };

            tail_offset
        };

        Ok(Self {
            tail_offset,
            offsets_buffer: Vec::with_capacity(128),
            hash_buffer: Vec::with_capacity(128),
        })
    }

    // pub fn add_data(&mut self, journal_file: &mut JournalFile<MmapMut>, data: &[u8]) -> Result<()> {
    //     let hash = journal_file.hash(data);
    //     println!("Hash: {:?}", hash);

    //     match journal_file.find_data_offset(hash, data) {
    //         Ok(Some(data_offset)) => {
    //             println!(
    //                 "Found data {:?} at offset: {:?}",
    //                 String::from_utf8_lossy(data),
    //                 data_offset
    //             );

    //             self.offsets_buffer.push(data_offset);
    //             Ok(())
    //         }
    //         Ok(None) => {
    //             let size = data.len() as u64;

    //             let mut data_object = journal_file.data_mut(self.tail_offset, Some(size))?;

    //             data_object.header.hash = hash;
    //             data_object.set_payload(data);
    //             let advance_tail_offset = data_object.header.object_header.aligned_size();
    //             drop(data_object);

    //             let fetch_object = |offset| journal_file.data_mut(offset, None).map(|x| x);
    //             journal_file.data_hash_table_mut().unwrap().insert(
    //                 hash,
    //                 self.tail_offset,
    //                 fetch_object,
    //             );

    //             self.tail_offset = unsafe {
    //                 NonZeroU64::new_unchecked(self.tail_offset.get() + advance_tail_offset)
    //             };

    //             println!(
    //                 "Added data {:?} at offset: {:?}",
    //                 String::from_utf8_lossy(data),
    //                 self.tail_offset
    //             );

    //             Ok(())
    //         }
    //         Err(e) => Err(e),
    //     }
    // }

    pub fn add_data(&mut self, journal_file: &mut JournalFile<MmapMut>, data: &[u8]) -> Result<()> {
        let hash = journal_file.hash(data);
        println!("Hash: {:?}", hash);

        match journal_file.find_data_offset(hash, data) {
            Ok(Some(data_offset)) => {
                println!(
                    "Found data {:?} at offset: {:?}",
                    String::from_utf8_lossy(data),
                    data_offset
                );

                self.offsets_buffer.push(data_offset);
                Ok(())
            }
            Ok(None) => {
                let size = data.len() as u64;

                // First, create and populate the data object
                let advance_tail_offset = {
                    let mut data_object = journal_file.data_mut(self.tail_offset, Some(size))?;
                    data_object.header.hash = hash;
                    data_object.set_payload(data);
                    data_object.header.object_header.aligned_size()
                };

                // Get the current tail offset from the hash table (if any)
                let tail_offset = {
                    let hash_table = journal_file
                        .data_hash_table_ref()
                        .ok_or(JournalError::MissingHashTable)?;
                    let bucket_index = (hash % hash_table.items.len() as u64) as usize;
                    hash_table.items[bucket_index].tail_hash_offset
                };

                // Update the tail object's next_hash_offset if there is one
                if let Some(tail_offset) = tail_offset {
                    let mut tail_object = journal_file.data_mut(tail_offset, None)?;
                    tail_object.header.next_hash_offset = Some(self.tail_offset);
                }

                // Update the hash table bucket
                {
                    let mut hash_table = journal_file
                        .data_hash_table_mut()
                        .ok_or(JournalError::MissingHashTable)?;
                    let bucket_index = (hash % hash_table.items.len() as u64) as usize;
                    let bucket = &mut hash_table.items[bucket_index];

                    if bucket.head_hash_offset.is_none() {
                        bucket.head_hash_offset = Some(self.tail_offset);
                    }
                    bucket.tail_hash_offset = Some(self.tail_offset);
                }

                self.tail_offset = unsafe {
                    NonZeroU64::new_unchecked(self.tail_offset.get() + advance_tail_offset)
                };

                println!(
                    "Added data {:?} at offset: {:?}",
                    String::from_utf8_lossy(data),
                    self.tail_offset
                );

                Ok(())
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

        for payload in items.iter() {
            self.add_data(journal_file, payload)?;
        }

        Ok(0)
    }
}
