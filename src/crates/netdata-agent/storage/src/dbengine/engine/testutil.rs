//! Fixtures for the engine's tests: synthetic tier directories built with the format layer's encoders.

use std::path::Path;

use super::load::TierConfig;
use super::query::Query;
use crate::dbengine::format::descriptor::{PAGE_TYPE_ARRAY_32BIT, PageDescriptor};
use crate::dbengine::format::journal_v1::{StoreData, encode_transaction};
use crate::dbengine::format::{BLOCK_SIZE, FileKind, file_name, superblock};

pub const NOW: i64 = 1_800_000_000;
pub const A: [u8; 16] = [0xaa; 16];
pub const B: [u8; 16] = [0xbb; 16];

pub fn cfg(dir: &Path) -> TierConfig {
    TierConfig {
        max_disk_space: 256 * 1024 * 1024,
        ..TierConfig::new(0, dir.to_path_buf())
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

/// An array page of `values` (storage numbers) one second apart, its first point at `start_s`, with its bytes.
pub fn array_page(uuid: [u8; 16], start_s: i64, values: &[u32]) -> (PageDescriptor, Vec<u8>) {
    let d = page(uuid, start_s, values.len() as u64);
    (d, crate::dbengine::format::page::array32_encode(values))
}

/// A pair whose data file holds real extents, one after the other from block 1, each described by a journal
/// transaction.
pub fn pair_with_extents(dir: &Path, fileno: u32, extents: &[Vec<(PageDescriptor, Vec<u8>)>]) {
    use crate::dbengine::format::extent::{COMPRESSION_NONE, encode};
    let mut data = superblock::encode_datafile().to_vec();
    let mut journal = superblock::encode_journal().to_vec();
    for (i, pages) in extents.iter().enumerate() {
        let refs: Vec<(PageDescriptor, &[u8])> =
            pages.iter().map(|(d, b)| (*d, b.as_slice())).collect();
        let e = encode(&refs, COMPRESSION_NONE);
        let store = StoreData {
            extent_offset: data.len() as u64,
            extent_size: e.size_bytes as u32,
            descriptors: pages.iter().map(|(d, _)| *d).collect(),
        };
        data.extend_from_slice(&e.bytes);
        journal.extend_from_slice(&encode_transaction(
            u64::from(fileno) * 1000 + i as u64,
            &store,
        ));
    }
    std::fs::write(dir.join(file_name(FileKind::Datafile, 1, fileno)), data).unwrap();
    std::fs::write(dir.join(file_name(FileKind::Journal, 1, fileno)), journal).unwrap();
}

/// Every point of a query, as (end time, value or NaN); each counts as one point, empty ones too.
pub fn points(q: &mut Query) -> Vec<(i64, f64)> {
    let mut out = Vec::new();
    while !q.is_finished() {
        let p = q.next_metric();
        assert_eq!(p.count, 1, "at {}", p.end_time_s);
        out.push((p.end_time_s, p.sum));
    }
    out
}

pub fn same(got: &[(i64, f64)], want: &[(i64, f64)]) -> bool {
    got.len() == want.len()
        && got
            .iter()
            .zip(want)
            .all(|(g, w)| g.0 == w.0 && ((g.1 - w.1).abs() < 1e-9 || g.1.is_nan() && w.1.is_nan()))
}
