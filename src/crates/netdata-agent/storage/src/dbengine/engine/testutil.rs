//! Fixtures for the engine's tests: synthetic tier directories built with the format layer's encoders.

use std::path::Path;

use std::sync::Arc;

use netdata_agent_evloop::work::WorkPool;
use serde_json::Value;

use super::cache::CachedPage;
use super::load::{TierConfig, load};
use super::mrg::{Handle, Mrg};
use super::query::{Dbengine, EngineConfig, Query};
use crate::dbengine::format::descriptor::{PAGE_TYPE_ARRAY_32BIT, PageDescriptor};
use crate::dbengine::format::extent::COMPRESSION_NONE;
use crate::dbengine::format::inspect::{for_each_page, tier_report};
use crate::dbengine::format::journal_v1::{StoreData, encode_transaction};
use crate::dbengine::format::page::{DiskPage, PageBuilder};
use crate::dbengine::format::{BLOCK_SIZE, FileKind, file_name, superblock};
use crate::storage_number::{SN_DEFAULT_FLAGS, pack};

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

/// A tier with array pages, uncompressed extents, and a 25 MiB quota: data files of 524,288 bytes.
pub fn write_cfg(tier: usize, dir: &Path) -> TierConfig {
    TierConfig {
        max_disk_space: 25 << 20,
        page_type: PAGE_TYPE_ARRAY_32BIT,
        compression: COMPRESSION_NONE,
        ..TierConfig::new(tier, dir.to_path_buf())
    }
}

/// A write engine on `dirs`, one tier each; without `keep_clean` the main cache keeps no clean page nobody holds, so
/// that queries read from disk.
pub fn write_engine(dirs: &[&Path], keep_clean: bool, pool: Option<WorkPool>) -> Arc<Dbengine> {
    let mrg = Mrg::new();
    let tiers = dirs
        .iter()
        .enumerate()
        .map(|(t, d)| load(write_cfg(t, d), &mrg, NOW).unwrap())
        .collect();
    let e = Dbengine::new(
        mrg,
        tiers,
        EngineConfig {
            pool,
            ..EngineConfig::new(|| NOW)
        },
    );
    if !keep_clean {
        e.main.keep_no_clean_pages();
    }
    e
}

/// A tier-0 write engine on `dir` as a restart finds it: the files loaded and their v2 indexes populated into a fresh
/// registry, no main cache.
pub fn restarted_engine(dir: &Path) -> Arc<Dbengine> {
    let mrg = Mrg::new();
    let mut tier = load(write_cfg(0, dir), &mrg, NOW).unwrap();
    let ((), _) = netdata_agent_log::capture(|| {
        super::v2index::populate(&mut tier, &mrg, &WorkPool::new(2, 256 * 1024), 2, NOW)
    });
    let e = Dbengine::new(mrg, vec![tier], EngineConfig::new(|| NOW));
    e.main.keep_no_clean_pages();
    e
}

pub fn nth(n: usize) -> [u8; 16] {
    let mut u = [0u8; 16];
    u[..8].copy_from_slice(&(n as u64 + 1).to_be_bytes());
    u
}

/// A dirty page of `uuid` with these values one second apart from `start`; the metric registered with its retention.
pub fn dirty_page(e: &Dbengine, tier: usize, uuid: [u8; 16], start: i64, values: &[f64]) -> Handle {
    let mut builder = PageBuilder::new(PAGE_TYPE_ARRAY_32BIT, values.len()).unwrap();
    for &v in values {
        builder.append(v, v, v, 1, 0, SN_DEFAULT_FLAGS);
    }
    let end = start + values.len() as i64 - 1;
    let page = CachedPage::collected(start, 1, builder);
    page.hot_set_end_time_s(end);
    let page = e.main.add(tier, &uuid, page).unwrap();
    e.main.hot_to_dirty(tier, &page);
    let (metric, _) = e.mrg.add_and_acquire(&uuid, tier, start, end, 1);
    metric
}

pub fn seq_values(n: usize, base: usize) -> Vec<f64> {
    (0..n).map(|i| (base + i) as f64).collect()
}

/// Every page a tier's files hold, in journal order: metric, start, storage numbers.
pub fn stored_pages(dir: &Path) -> Vec<([u8; 16], i64, Vec<u32>)> {
    let mut out = Vec::new();
    for_each_page(dir, |d, bytes| {
        let numbers = DiskPage::load(d.page_type, bytes)
            .ok()
            .and_then(|p| p.storage_numbers())
            .unwrap_or_default();
        out.push((d.uuid, (d.start_time_ut / 1_000_000) as i64, numbers));
    })
    .unwrap();
    out
}

pub fn packed(values: &[f64]) -> Vec<u32> {
    values.iter().map(|&v| pack(v, SN_DEFAULT_FLAGS)).collect()
}

pub fn file_reports(dir: &Path) -> Vec<Value> {
    tier_report(dir).unwrap()["files"]
        .as_array()
        .unwrap()
        .clone()
}

pub fn messages(records: Vec<netdata_agent_log::Captured>) -> Vec<String> {
    records.into_iter().filter_map(|r| r.message).collect()
}
