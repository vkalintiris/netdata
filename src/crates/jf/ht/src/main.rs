#![allow(unused_imports)]

use error::Result;
use memmap2::{Mmap, MmapMut};
use std::num::NonZeroU64;
use std::ops::{Deref, DerefMut};
use window_manager::{MemoryMap, MemoryMapMut, WindowManager};
use zerocopy::{
    ByteSlice, ByteSliceMut, FromBytes, Immutable, IntoBytes, KnownLayout, Ref, SplitByteSlice,
    SplitByteSliceMut,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ObjectType {
    Unused = 0,
    Data = 1,
    Field = 2,
    Entry = 3,
    DataHashTable = 4,
    FieldHashTable = 5,
    EntryArray = 6,
    Tag = 7,
}

#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct ObjectHeader {
    pub type_: u8,
    pub flags: u8,
    pub reserved: [u8; 6],
    pub size: u64,
}

/// Trait to standardize creation of journal objects from byte slices
pub trait JournalObject<B: SplitByteSlice>: Sized {
    /// Create a new journal object from a byte slice
    fn from_data(data: B, is_compact: bool) -> Option<Self>;
}

pub trait JournalObjectMut<B: SplitByteSliceMut>: JournalObject<B> {
    /// Create a new journal object from a byte slice
    fn from_data_mut(data: B, is_compact: bool) -> Option<Self>;
}

impl<B: SplitByteSlice> JournalObject<B> for HashTableObject<B> {
    fn from_data(data: B, _is_compact: bool) -> Option<Self> {
        let (header_data, items_data) = data.split_at(std::mem::size_of::<ObjectHeader>()).ok()?;

        let header = zerocopy::Ref::from_bytes(header_data).ok()?;
        let items = zerocopy::Ref::from_bytes(items_data).ok()?;

        Some(HashTableObject { header, items })
    }
}

impl<B: SplitByteSliceMut> JournalObjectMut<B> for HashTableObject<B> {
    fn from_data_mut(data: B, _is_compact: bool) -> Option<Self> {
        let (header_data, items_data) = data.split_at(std::mem::size_of::<ObjectHeader>()).ok()?;

        let header = zerocopy::Ref::from_bytes(header_data).ok()?;
        let items = zerocopy::Ref::from_bytes(items_data).ok()?;

        Some(HashTableObject { header, items })
    }
}

pub trait HashableObject {
    /// Get the hash value of this object
    fn hash(&self) -> u64;

    /// Get the payload data for matching
    fn get_payload(&self) -> &[u8];

    /// Get the offset to the next object in the hash chain
    fn next_hash_offset(&self) -> Option<NonZeroU64>;

    /// Get the object type
    fn object_type() -> ObjectType;
}

pub trait HashableObjectMut: HashableObject {
    /// Set the offset to the next object in the hash chain
    fn set_next_hash_offset(&mut self, offset: NonZeroU64);
}

#[derive(Debug, Copy, Clone, FromBytes, IntoBytes, KnownLayout, Immutable)]
#[repr(C)]
pub struct HashItem {
    pub head_hash_offset: Option<NonZeroU64>,
    pub tail_hash_offset: Option<NonZeroU64>,
}

pub struct HashTableObject<B: ByteSlice> {
    pub header: Ref<B, ObjectHeader>,
    pub items: Ref<B, [HashItem]>,
}

impl<B: ByteSliceMut> HashTableObject<B> {
    pub fn insert<T: HashableObjectMut>(&mut self) -> Option<Self> {
        todo!()
    }
}

use std::fs::{File, OpenOptions};

fn map_memory<M: MemoryMap>(file: &File, offset: NonZeroU64, size: NonZeroU64) -> Result<M> {
    M::create(file, offset.get(), size.get())
}

fn gvd() {
    let file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open("/tmp/foo.bin")
        .unwrap();

    let offset = NonZeroU64::new(4096).unwrap();
    let size = NonZeroU64::new(64 * 1024).unwrap();

    let mut map = map_memory::<MmapMut>(&file, offset, size).unwrap();
    let _ht = HashTableObject::from_data(map.deref_mut(), false).unwrap();

    println!("map: {:#?}", map);
}

fn main() {
    gvd()
}
