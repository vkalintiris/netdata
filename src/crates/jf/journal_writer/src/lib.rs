// // #![allow(unused_imports, dead_code)]

// use error::{JournalError, Result};
// use journal_file::{
//     journal_hash_data, CompactEntryItem, DataObject, DataObjectHeader, DataPayloadType,
//     EntryObject, EntryObjectHeader, FieldObject, FieldObjectHeader, HashItem, HashTableObject,
//     HeaderIncompatibleFlags, JournalFile, JournalHeader, JournalState, ObjectHeader, ObjectType,
//     RegularEntryItem,
// };
// use memmap2::MmapMut;
// use rand::{seq::IndexedRandom, Rng};
// use std::num::NonZeroU64;
// use std::path::Path;
// use window_manager::MemoryMapMut;
// use zerocopy::{FromBytes, IntoBytes};

// const OBJECT_ALIGNMENT: u64 = 8;

// #[derive(Default)]
// pub struct JournalWriter {
//     tail_offset: u64,
//     offsets_buffer: Vec<u64>,
//     hash_buffer: Vec<u64>,
// }

// impl JournalWriter {
//     pub fn new(journal_file: &mut JournalFile<MmapMut>) -> Result<Self> {
//         let tail_offset = {
//             let header = journal_file.journal_header_ref();
//             let position = header.tail_object_offset;
//             journal_file.object_header_ref(position)?.size
//         };

//         Ok(Self {
//             tail_offset,
//             offsets_buffer: Vec::with_capacity(128),
//             hash_buffer: Vec::with_capacity(128),
//         })
//     }

//     pub fn add_data(&mut self, journal_file: &mut JournalFile<MmapMut>, data: &[u8]) -> Result<()> {
//         let hash = journal_file.hash(data);

//         match journal_file.find_data_offset_by_payload(data, hash) {
//             Ok(data_offset) => {
//                 self.offsets_buffer.push(data_offset);
//                 Ok(())
//             }
//             Err(JournalError::MissingObjectFromHashTable) => {
//                 let size = data.len() as u64;

//                 let mut data_object = journal_file.data_mut(self.tail_offset, Some(size))?;

//                 data_object.header.hash = hash;
//                 data_object.set_payload(data);

//                 self.tail_offset += data_object.header.object_header.aligned_size();
//                 Ok(())
//             }
//             Err(e) => Err(e),
//         }
//     }

//     pub fn add_entry(
//         &mut self,
//         journal_file: &mut JournalFile<MmapMut>,
//         items: &[&[u8]],
//         realtime: u64,
//         monotonic: u64,
//         boot_id: [u8; 16],
//     ) -> Result<u64> {
//         let header = journal_file.journal_header_ref();

//         let is_keyed_hash = header.has_incompatible_flag(HeaderIncompatibleFlags::KeyedHash);

//         let file_id = if is_keyed_hash {
//             Some(&header.file_id)
//         } else {
//             None
//         };
//         self.hash_buffer.clear();
//         self.hash_buffer.extend(
//             items
//                 .iter()
//                 .map(|item| journal_hash_data(item, is_keyed_hash, file_id)),
//         );

//         for payload in items.iter() {
//             self.add_data(journal_file, payload)?;
//         }

//         Ok(0)
//     }
// }
