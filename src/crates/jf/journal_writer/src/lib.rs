#![allow(unused_imports, dead_code)]

use error::{JournalError, Result};
use journal_file::{
    DataObject, DataObjectHeader, FieldObject, FieldObjectHeader, HashItem, HashTableObject,
    HeaderIncompatibleFlags, JournalFile, JournalHeader, JournalState, ObjectHeader, ObjectType,
};
use memmap2::MmapMut;
use rand::Rng;
use std::path::Path;
use window_manager::MemoryMapMut;
use zerocopy::{FromBytes, IntoBytes};

const HEADER_SIZE: u64 = std::mem::size_of::<JournalHeader>() as u64;
const OBJECT_HEADER_SIZE: u64 = std::mem::size_of::<ObjectHeader>() as u64;
const DATA_OBJECT_HEADER_SIZE: u64 = std::mem::size_of::<DataObjectHeader>() as u64;
const FIELD_OBJECT_HEADER_SIZE: u64 = std::mem::size_of::<FieldObjectHeader>() as u64;
const HASH_ITEM_SIZE: u64 = std::mem::size_of::<HashItem>() as u64;

// Default hash table sizes
const DEFAULT_DATA_HASH_TABLE_SIZE: u64 = 2048;
const DEFAULT_FIELD_HASH_TABLE_SIZE: u64 = 512;

// Alignment for objects
const OBJECT_ALIGNMENT: u64 = 8;

pub struct JournalWriter<'a> {
    current_offset: u64,
}

impl<'a> JournalWriter<'a> {
    pub fn create(
        path: impl AsRef<Path>,
        window_size: u64,
        machine_id: [u8; 16],
    ) -> Result<(JournalFile<MmapMut>, Self)> {
        // Create the file
        std::fs::File::create(&path)?;
        
        // Open as journal file
        let mut journal_file = JournalFile::<MmapMut>::open(path, window_size)?;
        
        // Initialize the file
        let mut writer = JournalWriter {
            current_offset: 0,
        };
        
        writer.initialize_journal(machine_id)?;
        
        Ok((journal_file, writer))
    }

