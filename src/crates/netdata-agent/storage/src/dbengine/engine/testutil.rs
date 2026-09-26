//! Fixtures for the engine's tests: synthetic tier directories built with the format layer's encoders.

use std::path::Path;

use super::load::TierConfig;
use crate::dbengine::format::descriptor::{PAGE_TYPE_ARRAY_32BIT, PageDescriptor};
use crate::dbengine::format::journal_v1::{StoreData, encode_transaction};
use crate::dbengine::format::{BLOCK_SIZE, FileKind, file_name, superblock};

pub const NOW: i64 = 1_800_000_000;
pub const A: [u8; 16] = [0xaa; 16];
pub const B: [u8; 16] = [0xbb; 16];

pub fn cfg(dir: &Path) -> TierConfig {
    TierConfig {
        tier: 0,
        path: dir.to_path_buf(),
        direct_io: false,
        max_disk_space: 256 * 1024 * 1024,
        journal_check: false,
    }
}

/// An array page of `entries` points one second apart, its first point at `start_s`.
pub fn page(uuid: [u8; 16], start_s: i64, entries: u64) -> PageDescriptor {
    PageDescriptor::array(
        PAGE_TYPE_ARRAY_32BIT,
        uuid,
        (entries * 4) as u32,
        start_s as u64 * 1_000_000,
        (start_s as u64 + entries - 1) * 1_000_000,
    )
}

/// A pair: a data file of `blocks` blocks after its superblock, and a journal with one transaction of these pages
/// (at most 109, one extent's worth).
pub fn pair(dir: &Path, fileno: u32, blocks: usize, pages: Vec<PageDescriptor>) {
    let mut data = superblock::encode_datafile().to_vec();
    data.resize(BLOCK_SIZE * (1 + blocks), 0);
    std::fs::write(dir.join(file_name(FileKind::Datafile, 1, fileno)), data).unwrap();
    let mut journal = superblock::encode_journal().to_vec();
    for (i, chunk) in pages.chunks(109).enumerate() {
        let store = StoreData {
            extent_offset: (BLOCK_SIZE * (1 + i)) as u64,
            extent_size: BLOCK_SIZE as u32,
            descriptors: chunk.to_vec(),
        };
        journal.extend_from_slice(&encode_transaction(
            u64::from(fileno) * 1000 + i as u64,
            &store,
        ));
    }
    std::fs::write(dir.join(file_name(FileKind::Journal, 1, fileno)), journal).unwrap();
}
