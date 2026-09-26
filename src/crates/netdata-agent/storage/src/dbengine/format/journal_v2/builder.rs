//! Building a journal v2 image in memory as C indexes a journal (`pgc_open_cache_to_journal_v2()` in `cache.c`, then
//! `journalfile_migrate_to_v2_callback()`), from pages or from a replayed v1 journal (the startup rebuild:
//! `journalfile_restore_extent_metadata()` with its metric-registry and open-cache rules), and writing the image.
//! Brief `knowledge/brief-dbengine-s0.md` §4.2, §4.3 and §4.5 in the status repository.

use std::collections::{BTreeMap, HashMap, hash_map};
use std::fs::{self, OpenOptions};
use std::io;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::path::Path;

use super::super::BLOCK_SIZE;
use super::super::crc::{crc_bytes, crc32};
use super::super::descriptor::{
    PAGE_TYPE_GORILLA_32BIT, ValidatedPage, validate_extent_page_descr,
};
use super::super::journal_v1::{Event, Replay};
use super::{
    EXTENT_SIZE, ExtentEntry, HEADER_SIZE, Header, MAGIC, METRIC_SIZE, MetricEntry,
    PAGE_HEADER_SIZE, PAGE_SIZE, PageEntry, PageHeader, TRAILER_SIZE, file_size,
};

/// A page to index, as the open cache holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page {
    pub uuid: [u8; 16],
    pub start_time_s: i64,
    pub end_time_s: i64,
    pub update_every_s: u32,
    /// The extent's offset in the data file shifted down by 12 (`xio->block`).
    pub block: u64,
    /// The extent's size without its padding.
    pub extent_bytes: u32,
    /// Ranks duplicates only.
    pub page_length: usize,
}

#[derive(Debug)]
struct PageInfo {
    end_time_s: i64,
    update_every_s: u32,
    page_length: usize,
    block: u64,
}

#[derive(Debug)]
struct MetricInfo {
    first_time_s: i64,
    last_time_s: i64,
    /// Keyed by start time as C's `Word_t`.
    pages: BTreeMap<u64, PageInfo>,
}

#[derive(Debug)]
struct ExtentInfo {
    bytes: u32,
    pages: u32,
    index: u32,
}

/// C's journal v2 index of one data file: metrics by uuid, their pages by start time, extents by block.
#[derive(Debug, Default)]
pub struct Builder {
    v1_size: u32,
    metrics: HashMap<[u8; 16], MetricInfo>,
    extents: BTreeMap<u64, ExtentInfo>,
    next_index: u32,
}

/// `jv2_duplicate_page_wins()`: the longer page, then the larger one, then the one in the earlier extent.
fn wins(p: &Page, incumbent: &PageInfo) -> bool {
    if p.end_time_s != incumbent.end_time_s {
        return p.end_time_s > incumbent.end_time_s;
    }
    if p.page_length != incumbent.page_length {
        return p.page_length > incumbent.page_length;
    }
    p.block < incumbent.block
}

impl Builder {
    /// `v1_size` is stored in the header: the journal's replayed size.
    pub fn new(v1_size: u32) -> Self {
        Builder {
            v1_size,
            ..Builder::default()
        }
    }

    /// Indexes a page, in the order C walks the open cache's hot pages. `true` when a page with the same metric and
    /// start was already indexed and one of the two was dropped (C logs a notice).
    pub fn page(&mut self, p: Page) -> bool {
        let mi = self.metrics.entry(p.uuid).or_insert_with(|| MetricInfo {
            first_time_s: p.start_time_s,
            last_time_s: p.end_time_s,
            pages: BTreeMap::new(),
        });
        let key = p.start_time_s as u64;
        let incumbent = mi.pages.get(&key);
        if incumbent.is_some_and(|pi| !wins(&p, pi)) {
            return true;
        }
        let replaced = incumbent.map(|pi| pi.block);
        let next_index = &mut self.next_index;
        let ei = self.extents.entry(p.block).or_insert_with(|| {
            let index = *next_index;
            *next_index += 1;
            ExtentInfo {
                bytes: p.extent_bytes,
                pages: 0,
                index,
            }
        });
        ei.pages += 1;
        // the replaced page leaves its extent, which may be left empty; `build()` compacts
        if let Some(old) = replaced.and_then(|block| self.extents.get_mut(&block)) {
            old.pages -= 1;
        }
        let pi = PageInfo {
            end_time_s: p.end_time_s,
            update_every_s: p.update_every_s,
            page_length: p.page_length,
            block: p.block,
        };
        mi.pages.insert(key, pi);
        mi.first_time_s = mi.first_time_s.min(p.start_time_s);
        mi.last_time_s = mi.last_time_s.max(p.end_time_s);
        replaced.is_some()
    }

    /// The v2 file's bytes, or `None` without metrics (C then writes nothing and reports success). The page area's
    /// slack is zero.
    pub fn build(mut self) -> Option<Vec<u8>> {
        if self.metrics.is_empty() {
            return None;
        }
        // extents a duplicate emptied are dropped and the rest renumbered in block order
        if self.extents.values().any(|ei| ei.pages == 0) {
            self.extents.retain(|_, ei| ei.pages != 0);
            for (index, ei) in (0u32..).zip(self.extents.values_mut()) {
                ei.index = index;
            }
        }
        let pages: usize = self.metrics.values().map(|mi| mi.pages.len()).sum();
        let (e, m) = (self.extents.len(), self.metrics.len());
        let size = file_size(e, m, pages);
        let mut b = vec![0u8; size];

        let extent_offset = BLOCK_SIZE;
        for (block, ei) in &self.extents {
            let entry = ExtentEntry {
                datafile_offset: block << 12,
                datafile_size: ei.bytes,
                file_index: 0,
                pages: ei.pages as u8,
            };
            let at = extent_offset + ei.index as usize * EXTENT_SIZE;
            b[at..at + EXTENT_SIZE].copy_from_slice(&entry.encode());
        }
        let extent_trailer = extent_offset + EXTENT_SIZE * e;
        put_crc(&mut b, extent_offset, extent_trailer);

        let first = self.metrics.values().map(|mi| mi.first_time_s).min();
        let last = self
            .metrics
            .values()
            .map(|mi| mi.last_time_s)
            .fold(0, i64::max);
        let start_time_ut = (first.unwrap_or(0) as u64).wrapping_mul(1_000_000);
        let base = (start_time_ut / 1_000_000) as i64;
        let delta = |t: i64| t.wrapping_sub(base) as u32;
        let mut sorted: Vec<_> = self.metrics.iter().collect();
        sorted.sort_unstable_by(|a, b| a.0.cmp(b.0));

        let metric_offset = extent_trailer + TRAILER_SIZE;
        let metric_trailer = metric_offset + METRIC_SIZE * m;
        let page_offset = metric_trailer + TRAILER_SIZE;
        let mut at = page_offset;
        for (i, (uuid, mi)) in sorted.into_iter().enumerate() {
            let n = mi.pages.len();
            let uuid_offset = metric_offset + i * METRIC_SIZE;
            let entry = MetricEntry {
                uuid: *uuid,
                entries: n as u32,
                page_offset: at as u32,
                delta_start_s: delta(mi.first_time_s),
                delta_end_s: delta(mi.last_time_s),
                update_every_s: mi.pages.values().last().map_or(0, |pi| pi.update_every_s),
            };
            b[uuid_offset..uuid_offset + METRIC_SIZE].copy_from_slice(&entry.encode());
            let mut ph = PageHeader {
                crc: 0,
                uuid_offset: uuid_offset as u32,
                entries: n as u32,
                uuid: *uuid,
            };
            ph.crc = ph.compute_crc();
            b[at..at + PAGE_HEADER_SIZE].copy_from_slice(&ph.encode());
            let list = at + PAGE_HEADER_SIZE;
            for (j, (&start, pi)) in mi.pages.iter().enumerate() {
                let entry = PageEntry {
                    delta_start_s: delta(start as i64),
                    delta_end_s: delta(pi.end_time_s),
                    extent_index: self.extents[&pi.block].index,
                    update_every_s: pi.update_every_s,
                    page_length: 0,
                    page_type: 0,
                };
                let pa = list + j * PAGE_SIZE;
                b[pa..pa + PAGE_SIZE].copy_from_slice(&entry.encode());
            }
            put_crc(&mut b, list, list + n * PAGE_SIZE);
            at = list + n * PAGE_SIZE + TRAILER_SIZE;
        }
        put_crc(&mut b, metric_offset, metric_trailer);

        let h = Header {
            magic: MAGIC,
            start_time_ut,
            end_time_ut: (last as u64).wrapping_mul(1_000_000),
            extent_count: e as u32,
            extent_offset: extent_offset as u32,
            metric_count: m as u32,
            metric_offset: metric_offset as u32,
            page_count: pages as u32,
            page_offset: page_offset as u32,
            extent_trailer_offset: extent_trailer as u32,
            metric_trailer_offset: metric_trailer as u32,
            journal_v1_file_size: self.v1_size,
            journal_v2_file_size: size as u32,
        };
        let header = h.encode();
        b[..HEADER_SIZE].copy_from_slice(&header);
        b[size - TRAILER_SIZE..].copy_from_slice(&crc_bytes(crc32(&header)));
        Some(b)
    }
}

/// Stores the CRC of `b[from..to]` at `to`.
fn put_crc(b: &mut [u8], from: usize, to: usize) {
    let crc = crc_bytes(crc32(&b[from..to]));
    b[to..to + TRAILER_SIZE].copy_from_slice(&crc);
}

/// What C's replay reads and updates per metric in its metric registry: the update every a single-point page takes
/// (`mrg_metric_get_update_every_s()`), and the add-or-expand of each replayed page.
pub trait UeSource {
    /// 0 when the metric is unknown.
    fn update_every(&self, uuid: &[u8; 16]) -> u32;
    /// A valid page was replayed.
    fn replayed(&mut self, uuid: &[u8; 16], vd: &ValidatedPage);
}

/// A metric's registry fields that C's replay reads or sets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricRetention {
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub update_every_s: u32,
}

impl MetricRetention {
    /// `mrg_metric_expand_retention()`.
    pub fn expand(&mut self, first_time_s: i64, last_time_s: i64, update_every_s: u32) {
        if first_time_s > 0
            && first_time_s != i64::MAX
            && (self.first_time_s <= 0 || first_time_s < self.first_time_s)
        {
            self.first_time_s = first_time_s;
        }
        if last_time_s > 0 {
            if (self.last_time_s <= 0 || last_time_s > self.last_time_s)
                && last_time_s != self.last_time_s
            {
                self.last_time_s = last_time_s;
                if update_every_s > 0 {
                    self.update_every_s = update_every_s;
                }
            }
        } else if update_every_s > 0 && self.update_every_s == 0 {
            self.update_every_s = update_every_s;
        }
    }
}

/// An in-memory metric registry with C's rules; the default knows no metric, as at a first start.
#[derive(Debug, Clone, Default)]
pub struct Retention {
    pub metrics: HashMap<[u8; 16], MetricRetention>,
}

impl UeSource for Retention {
    fn update_every(&self, uuid: &[u8; 16]) -> u32 {
        self.metrics.get(uuid).map_or(0, |m| m.update_every_s)
    }

    fn replayed(&mut self, uuid: &[u8; 16], vd: &ValidatedPage) {
        match self.metrics.entry(*uuid) {
            // metric_add_and_acquire()
            hash_map::Entry::Vacant(v) => {
                v.insert(MetricRetention {
                    first_time_s: vd.start_time_s.max(0),
                    last_time_s: vd.end_time_s.max(0),
                    update_every_s: vd.update_every_s,
                });
            }
            hash_map::Entry::Occupied(mut o) => {
                o.get_mut()
                    .expand(vd.start_time_s, vd.end_time_s, vd.update_every_s);
            }
        }
    }
}

/// What a replayed v1 journal leaves in C's open cache (`journalfile_restore_extent_metadata()`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenCache {
    /// The hot pages in the order C walks them.
    pub pages: Vec<Page>,
    /// The journal's `v2.first_time_s` and `v2.last_time_s`: the valid pages' first start and last end, dropped pages
    /// included. A journal with transactions but no valid page keeps C's `LONG_MAX` first time; one without
    /// transactions keeps 0 for both.
    pub first_time_s: i64,
    pub last_time_s: i64,
}

/// The pages a v1 journal's replay puts in the open cache: each descriptor of known type is validated against `now_s`
/// (`max_acceptable_collected_time()`, 0 for no future check) with the registry's update every and recorded in `ue`;
/// a page whose metric and start are already held replaces the held one (moving to the end) only if it ends later.
/// Exact for one data file; pages of other files in C's open cache are not modelled.
pub fn open_cache_pages(replay: &Replay, now_s: i64, ue: &mut dyn UeSource) -> OpenCache {
    let mut hot: Vec<Option<Page>> = Vec::new();
    let mut held: HashMap<([u8; 16], i64), usize> = HashMap::new();
    let (mut first_time_s, mut last_time_s) = (0i64, 0i64);
    for event in &replay.events {
        let Event::StoreData { data, .. } = event else {
            continue;
        };
        let mut extent_first = if first_time_s != 0 {
            first_time_s
        } else {
            i64::MAX
        };
        let mut extent_last = last_time_s;
        for d in &data.descriptors {
            if d.page_type > PAGE_TYPE_GORILLA_32BIT {
                continue;
            }
            let vd = validate_extent_page_descr(d, now_s, ue.update_every(&d.uuid), false);
            if !vd.valid {
                continue;
            }
            ue.replayed(&d.uuid, &vd);
            let page = Page {
                uuid: d.uuid,
                start_time_s: vd.start_time_s,
                end_time_s: vd.end_time_s,
                update_every_s: vd.update_every_s,
                block: data.extent_offset >> 12,
                extent_bytes: data.extent_size,
                page_length: 0,
            };
            let key = (d.uuid, vd.start_time_s);
            match held.get(&key).copied() {
                Some(i) if hot[i].is_some_and(|p| page.end_time_s <= p.end_time_s) => {}
                old => {
                    if let Some(i) = old {
                        hot[i] = None;
                    }
                    held.insert(key, hot.len());
                    hot.push(Some(page));
                }
            }
            extent_first = extent_first.min(vd.start_time_s);
            extent_last = extent_last.max(vd.end_time_s);
        }
        first_time_s = extent_first;
        last_time_s = extent_last;
    }
    OpenCache {
        pages: hot.into_iter().flatten().collect(),
        first_time_s,
        last_time_s,
    }
}

/// The v2 file C builds at startup from a replayed v1 journal of `v1_st_size` bytes: its open-cache pages
/// (`open_cache_pages()`) indexed.
pub fn from_v1(
    replay: &Replay,
    v1_st_size: u64,
    now_s: i64,
    ue: &mut dyn UeSource,
) -> Option<Vec<u8>> {
    let v1_size = v1_st_size / BLOCK_SIZE as u64 * BLOCK_SIZE as u64;
    let mut b = Builder::new(v1_size as u32);
    for p in open_cache_pages(replay, now_s, ue).pages {
        b.page(p);
    }
    b.build()
}

/// Writes an image as C's migration does: created 0664, sized to the image, the body first and the header last, no
/// sync, and removed if a write fails. The file is emptied first, so no stale bytes of a longer earlier file stay in
/// the page area's slack (C keeps them). An image shorter than its 4096-byte header block is refused.
pub fn write_in_place(path: &Path, image: &[u8]) -> io::Result<()> {
    if image.len() < BLOCK_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a journal v2 image",
        ));
    }
    let write = || -> io::Result<()> {
        let f = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o664)
            .open(path)?;
        f.set_len(image.len() as u64)?;
        f.write_all_at(&image[BLOCK_SIZE..], BLOCK_SIZE as u64)?;
        f.write_all_at(&image[..HEADER_SIZE], 0)
    };
    write().inspect_err(|_| {
        let _ = fs::remove_file(path);
    })
}

#[cfg(test)]
mod tests {
    use super::super::super::descriptor::{PAGE_TYPE_ARRAY_32BIT, PageDescriptor};
    use super::super::super::journal_v1::StoreData;
    use super::super::{Verdict, extents, metrics, pages, validate};
    use super::*;

    const S: u64 = 1_000_000;

    fn page(uuid: u8, start: i64, end: i64, block: u64) -> Page {
        Page {
            uuid: [uuid; 16],
            start_time_s: start,
            end_time_s: end,
            update_every_s: 1,
            block,
            extent_bytes: 100 + block as u32,
            page_length: 16,
        }
    }

    struct Parsed {
        header: Header,
        extents: Vec<ExtentEntry>,
        metrics: Vec<MetricEntry>,
        pages: Vec<Vec<PageEntry>>,
    }

    fn parse(b: &[u8]) -> Parsed {
        assert_eq!(validate(b, 0, true).unwrap(), Verdict::Ok);
        let header = Header::decode(b[..HEADER_SIZE].try_into().unwrap());
        let at = header.extent_offset as usize;
        let extents = extents(&b[at..header.extent_trailer_offset as usize]).collect();
        let at = header.metric_offset as usize;
        let metrics: Vec<_> = metrics(&b[at..header.metric_trailer_offset as usize]).collect();
        let pages = metrics
            .iter()
            .map(|m| {
                let at = m.page_offset as usize + PAGE_HEADER_SIZE;
                pages(&b[at..at + m.entries as usize * PAGE_SIZE]).collect()
            })
            .collect();
        Parsed {
            header,
            extents,
            metrics,
            pages,
        }
    }

    fn built(v1: u32, input: &[Page]) -> Parsed {
        let mut b = Builder::new(v1);
        for p in input {
            b.page(*p);
        }
        parse(&b.build().unwrap())
    }

    #[test]
    fn layout_as_c() {
        let p = built(
            8192,
            &[
                page(2, 100, 103, 5),
                page(1, 104, 107, 5),
                page(1, 100, 103, 3),
            ],
        );
        let h = p.header;
        assert_eq!(
            (
                h.extent_count,
                h.metric_count,
                h.page_count,
                h.journal_v1_file_size
            ),
            (2, 2, 3, 8192)
        );
        assert_eq!((h.start_time_ut, h.end_time_ut), (100 * S, 107 * S));
        // extents in first-seen order; metrics by uuid bytes; pages by start
        let offsets: Vec<_> = p
            .extents
            .iter()
            .map(|e| (e.datafile_offset, e.pages))
            .collect();
        assert_eq!(offsets, [(5 << 12, 2), (3 << 12, 1)]);
        assert_eq!(p.extents[0].datafile_size, 105);
        assert_eq!(p.metrics[0].uuid, [1; 16]);
        assert_eq!(
            (p.metrics[0].delta_start_s, p.metrics[0].delta_end_s),
            (0, 7)
        );
        let starts: Vec<_> = p.pages[0]
            .iter()
            .map(|e| (e.delta_start_s, e.extent_index))
            .collect();
        assert_eq!(starts, [(0, 1), (4, 0)]);
        assert_eq!(p.pages[1][0].delta_end_s, 3);
    }

    #[test]
    fn a_metrics_update_every_is_its_last_pages() {
        let mut later = page(1, 110, 120, 1);
        later.update_every_s = 5;
        let p = built(0, &[later, page(1, 100, 103, 1)]);
        assert_eq!(p.metrics[0].update_every_s, 5);
        assert_eq!(p.pages[0][0].update_every_s, 1);
    }

    #[test]
    fn duplicates_rank_by_end_then_length_then_block() {
        let longer = page(1, 100, 110, 9);
        let mut larger = page(1, 100, 103, 9);
        larger.page_length = 32;
        for (first, second, winner) in [
            (page(1, 100, 103, 2), longer, longer),
            (longer, page(1, 100, 103, 2), longer),
            (page(1, 100, 103, 2), larger, larger),
            (
                page(1, 100, 103, 9),
                page(1, 100, 103, 2),
                page(1, 100, 103, 2),
            ),
            (
                page(1, 100, 103, 2),
                page(1, 100, 103, 9),
                page(1, 100, 103, 2),
            ),
        ] {
            let mut b = Builder::new(0);
            assert!(!b.page(first));
            assert!(b.page(second));
            let p = parse(&b.build().unwrap());
            assert_eq!(p.header.page_count, 1);
            assert_eq!(p.extents.len(), 1, "the loser's emptied extent is dropped");
            assert_eq!(p.extents[0].datafile_offset, winner.block << 12);
            assert_eq!(p.pages[0][0].delta_end_s, (winner.end_time_s - 100) as u32);
        }
    }

    #[test]
    fn emptied_extents_are_dropped_and_the_rest_renumbered_by_block() {
        // extents seen as 7, 4, 2; the duplicate in 2 beats the page of 4, which empties 4
        let p = built(
            0,
            &[
                page(1, 100, 103, 7),
                page(2, 100, 103, 4),
                page(2, 100, 105, 2),
            ],
        );
        let blocks: Vec<_> = p.extents.iter().map(|e| e.datafile_offset >> 12).collect();
        assert_eq!(blocks, [2, 7]);
        assert_eq!(
            (p.pages[0][0].extent_index, p.pages[1][0].extent_index),
            (1, 0)
        );
        // without an emptied extent the first-seen order stays
        let p = built(
            0,
            &[
                page(1, 100, 103, 7),
                page(2, 100, 103, 4),
                page(3, 100, 103, 2),
            ],
        );
        let blocks: Vec<_> = p.extents.iter().map(|e| e.datafile_offset >> 12).collect();
        assert_eq!(blocks, [7, 4, 2]);
    }

    #[test]
    fn no_metrics_no_file() {
        assert_eq!(Builder::new(0).build(), None);
        let replay = Replay {
            events: Vec::new(),
            max_id: 1,
            read_error: false,
        };
        assert_eq!(from_v1(&replay, 4096, 0, &mut Retention::default()), None);
    }

    fn store(extent_offset: u64, descriptors: Vec<PageDescriptor>) -> Event {
        Event::StoreData {
            pos: 0,
            id: 1,
            data: StoreData {
                extent_offset,
                extent_size: 200,
                descriptors,
            },
        }
    }

    /// An ARRAY_32BIT page of `points` points from `start` to `end` seconds.
    fn array(uuid: u8, points: u32, start: u64, end: u64) -> PageDescriptor {
        PageDescriptor::array(
            PAGE_TYPE_ARRAY_32BIT,
            [uuid; 16],
            points * 4,
            start * S,
            end * S,
        )
    }

    fn replay(events: Vec<Event>) -> Replay {
        Replay {
            events,
            max_id: 1,
            read_error: false,
        }
    }

    #[test]
    fn from_v1_as_the_startup_rebuild() {
        let mut unknown = array(3, 4, 100, 103);
        unknown.page_type = 3;
        let r = replay(vec![
            store(
                4096 + 100,
                vec![array(1, 4, 100, 103), array(2, 4, 100, 103), unknown],
            ),
            // the same metric and start: a later end replaces and moves to the end, an earlier one is dropped
            store(
                3 * 4096,
                vec![
                    array(1, 5, 100, 104),
                    array(2, 3, 100, 102),
                    array(4, 0, 100, 103),
                ],
            ),
            Event::Corrupt { pos: 0 },
        ]);
        let b = from_v1(&r, 3 * 4096 + 100, 0, &mut Retention::default()).unwrap();
        let p = parse(&b);
        assert_eq!(p.header.journal_v1_file_size, 3 * 4096);
        let uuids: Vec<_> = p.metrics.iter().map(|m| m.uuid[0]).collect();
        assert_eq!(uuids, [1, 2], "unknown types and invalid pages are skipped");
        // the unaligned extent offset indexes as its block; the replacement put block 3 after block 1
        let blocks: Vec<_> = p
            .extents
            .iter()
            .map(|e| (e.datafile_offset, e.pages))
            .collect();
        assert_eq!(blocks, [(4096, 1), (3 * 4096, 1)]);
        assert_eq!(
            (p.pages[0][0].delta_end_s, p.pages[0][0].extent_index),
            (4, 1)
        );
        assert_eq!(
            (p.pages[1][0].delta_end_s, p.pages[1][0].extent_index),
            (3, 0)
        );
    }

    #[test]
    fn the_open_cache_keeps_the_journal_times() {
        let empty = replay(vec![Event::Corrupt { pos: 0 }]);
        let none = open_cache_pages(&empty, 0, &mut Retention::default());
        assert_eq!(
            (none.pages.len(), none.first_time_s, none.last_time_s),
            (0, 0, 0)
        );
        // a transaction without a valid page leaves C's LONG_MAX first time
        let invalid = replay(vec![store(4096, vec![array(1, 0, 100, 103)])]);
        let got = open_cache_pages(&invalid, 0, &mut Retention::default());
        assert_eq!((got.first_time_s, got.last_time_s), (i64::MAX, 0));
        // a dropped page still counts
        let r = replay(vec![
            store(4096, vec![array(1, 4, 100, 106)]),
            store(8192, vec![array(1, 3, 100, 102), array(2, 4, 90, 93)]),
        ]);
        let got = open_cache_pages(&r, 0, &mut Retention::default());
        assert_eq!(got.pages.len(), 2);
        assert_eq!((got.first_time_s, got.last_time_s), (90, 106));
    }

    #[test]
    fn single_point_pages_take_the_registrys_update_every() {
        let r = replay(vec![store(
            4096,
            vec![array(1, 4, 100, 106), array(1, 1, 110, 110)],
        )]);
        let mut known = Retention::default();
        let p = parse(&from_v1(&r, 8192, 0, &mut known).unwrap());
        let ues: Vec<_> = p.pages[0].iter().map(|e| e.update_every_s).collect();
        assert_eq!(ues, [2, 2]);
        assert_eq!(p.metrics[0].update_every_s, 2);
        assert_eq!(
            known.metrics[&[1; 16]],
            MetricRetention {
                first_time_s: 100,
                last_time_s: 110,
                update_every_s: 2
            }
        );
        // a registry that knows the metric already
        let mut known = Retention::default();
        known.metrics.insert(
            [1; 16],
            MetricRetention {
                first_time_s: 50,
                last_time_s: 90,
                update_every_s: 7,
            },
        );
        let r = replay(vec![store(4096, vec![array(1, 1, 110, 110)])]);
        let p = parse(&from_v1(&r, 8192, 0, &mut known).unwrap());
        assert_eq!(p.pages[0][0].update_every_s, 7);
    }

    #[test]
    fn expand_as_the_registry() {
        let mut m = MetricRetention {
            first_time_s: 100,
            last_time_s: 200,
            update_every_s: 1,
        };
        m.expand(150, 180, 5);
        assert_eq!(
            (m.first_time_s, m.last_time_s, m.update_every_s),
            (100, 200, 1)
        );
        m.expand(90, 250, 5);
        assert_eq!(
            (m.first_time_s, m.last_time_s, m.update_every_s),
            (90, 250, 5)
        );
        m.expand(0, 260, 0);
        assert_eq!(
            (m.first_time_s, m.last_time_s, m.update_every_s),
            (90, 260, 5)
        );
        let mut m = MetricRetention::default();
        m.expand(0, 0, 3);
        assert_eq!(m.update_every_s, 3);
        m.expand(0, 0, 4);
        assert_eq!(
            m.update_every_s, 3,
            "a known update every is kept without a later end"
        );
    }

    #[test]
    fn write_in_place_replaces_a_longer_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("journalfile-1-0000000001.njfv2");
        fs::write(&path, vec![0xAA; 3 * BLOCK_SIZE + 17]).unwrap();
        let mut b = Builder::new(4096);
        b.page(page(1, 100, 103, 1));
        let image = b.build().unwrap();
        write_in_place(&path, &image).unwrap();
        assert_eq!(fs::read(&path).unwrap(), image);
        write_in_place(&path, &image).unwrap();
        assert_eq!(fs::read(&path).unwrap(), image);
        // a failed write leaves no file
        let missing = dir
            .path()
            .join("none")
            .join("journalfile-1-0000000001.njfv2");
        assert!(write_in_place(&missing, &image).is_err());
        assert!(!missing.exists());
    }
}
