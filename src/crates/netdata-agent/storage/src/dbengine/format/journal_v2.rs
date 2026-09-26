//! Journal v2 (`.njfv2`) files (`journalfile.h`): the indexed form of a journal, a 72-byte header and three CRC-checked
//! sections (extents, metrics, per-metric page lists). Validation follows `journalfile_v2_load()` and
//! `journalfile_v2_validate()` with their list checks (`journalfile.c`). Brief `knowledge/brief-dbengine-s0.md` §2.5
//! and §4 in the status repository.

use std::io;

use super::ReadAt;
use super::crc::{crc_matches, crc32};

mod builder;

pub use builder::{
    Builder, MetricRetention, OpenCache, Page, Retention, UeSource, expand, from_v1,
    open_cache_pages, write_in_place,
};

/// `JOURVAL_V2_MAGIC`, `JOURVAL_V2_REBUILD_MAGIC`, `JOURVAL_V2_SKIP_MAGIC`.
pub const MAGIC: u32 = 0x0123_0317;
pub const REBUILD_MAGIC: u32 = 0x0023_0317;
pub const SKIP_MAGIC: u32 = 0x0223_0317;

pub const HEADER_SIZE: usize = 72;
pub const EXTENT_SIZE: usize = 16;
pub const METRIC_SIZE: usize = 36;
pub const PAGE_HEADER_SIZE: usize = 28;
pub const PAGE_SIZE: usize = 20;
pub const TRAILER_SIZE: usize = 4;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[at..at + 8]);
    u64::from_le_bytes(v)
}

fn uuid_at(b: &[u8], at: usize) -> [u8; 16] {
    let mut v = [0u8; 16];
    v.copy_from_slice(&b[at..at + 16]);
    v
}

fn put_u32s(b: &mut [u8], fields: &[(usize, u32)]) {
    for &(at, v) in fields {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
}

/// `struct journal_v2_header`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Header {
    pub magic: u32,
    pub start_time_ut: u64,
    pub end_time_ut: u64,
    pub extent_count: u32,
    pub extent_offset: u32,
    pub metric_count: u32,
    pub metric_offset: u32,
    pub page_count: u32,
    pub page_offset: u32,
    pub extent_trailer_offset: u32,
    pub metric_trailer_offset: u32,
    pub journal_v1_file_size: u32,
    pub journal_v2_file_size: u32,
}

impl Header {
    pub fn decode(b: &[u8; HEADER_SIZE]) -> Self {
        Header {
            magic: u32_at(b, 0),
            start_time_ut: u64_at(b, 8),
            end_time_ut: u64_at(b, 16),
            extent_count: u32_at(b, 24),
            extent_offset: u32_at(b, 28),
            metric_count: u32_at(b, 32),
            metric_offset: u32_at(b, 36),
            page_count: u32_at(b, 40),
            page_offset: u32_at(b, 44),
            extent_trailer_offset: u32_at(b, 48),
            metric_trailer_offset: u32_at(b, 52),
            journal_v1_file_size: u32_at(b, 56),
            journal_v2_file_size: u32_at(b, 60),
        }
    }

    /// The 72 bytes as C stores them (its pad and `data` pointer zero).
    pub fn encode(&self) -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        b[8..16].copy_from_slice(&self.start_time_ut.to_le_bytes());
        b[16..24].copy_from_slice(&self.end_time_ut.to_le_bytes());
        put_u32s(
            &mut b,
            &[
                (0, self.magic),
                (24, self.extent_count),
                (28, self.extent_offset),
                (32, self.metric_count),
                (36, self.metric_offset),
                (40, self.page_count),
                (44, self.page_offset),
                (48, self.extent_trailer_offset),
                (52, self.metric_trailer_offset),
                (56, self.journal_v1_file_size),
                (60, self.journal_v2_file_size),
            ],
        );
        b
    }
}

/// `struct journal_extent_list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExtentEntry {
    /// The extent's offset in the data file: its block number shifted back.
    pub datafile_offset: u64,
    /// The extent's size without its padding.
    pub datafile_size: u32,
    pub file_index: u16,
    pub pages: u8,
}

impl ExtentEntry {
    pub fn decode(b: &[u8; EXTENT_SIZE]) -> Self {
        ExtentEntry {
            datafile_offset: u64_at(b, 0),
            datafile_size: u32_at(b, 8),
            file_index: u16_at(b, 12),
            pages: b[14],
        }
    }

    pub fn encode(&self) -> [u8; EXTENT_SIZE] {
        let mut b = [0u8; EXTENT_SIZE];
        b[0..8].copy_from_slice(&self.datafile_offset.to_le_bytes());
        b[8..12].copy_from_slice(&self.datafile_size.to_le_bytes());
        b[12..14].copy_from_slice(&self.file_index.to_le_bytes());
        b[14] = self.pages;
        b
    }
}

/// `struct journal_metric_list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MetricEntry {
    pub uuid: [u8; 16],
    pub entries: u32,
    /// The absolute offset of the metric's page header.
    pub page_offset: u32,
    pub delta_start_s: u32,
    pub delta_end_s: u32,
    pub update_every_s: u32,
}

impl MetricEntry {
    pub fn decode(b: &[u8; METRIC_SIZE]) -> Self {
        MetricEntry {
            uuid: uuid_at(b, 0),
            entries: u32_at(b, 16),
            page_offset: u32_at(b, 20),
            delta_start_s: u32_at(b, 24),
            delta_end_s: u32_at(b, 28),
            update_every_s: u32_at(b, 32),
        }
    }

    pub fn encode(&self) -> [u8; METRIC_SIZE] {
        let mut b = [0u8; METRIC_SIZE];
        b[0..16].copy_from_slice(&self.uuid);
        put_u32s(
            &mut b,
            &[
                (16, self.entries),
                (20, self.page_offset),
                (24, self.delta_start_s),
                (28, self.delta_end_s),
                (32, self.update_every_s),
            ],
        );
        b
    }
}

/// `struct journal_page_header`: before each metric's page list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageHeader {
    pub crc: u32,
    /// The absolute offset of the metric's entry.
    pub uuid_offset: u32,
    pub entries: u32,
    pub uuid: [u8; 16],
}

impl PageHeader {
    pub fn decode(b: &[u8; PAGE_HEADER_SIZE]) -> Self {
        PageHeader {
            crc: u32_at(b, 0),
            uuid_offset: u32_at(b, 4),
            entries: u32_at(b, 8),
            uuid: uuid_at(b, 12),
        }
    }

    pub fn encode(&self) -> [u8; PAGE_HEADER_SIZE] {
        let mut b = [0u8; PAGE_HEADER_SIZE];
        put_u32s(
            &mut b,
            &[(0, self.crc), (4, self.uuid_offset), (8, self.entries)],
        );
        b[12..28].copy_from_slice(&self.uuid);
        b
    }

    /// The header's CRC: over its 28 bytes with `crc` set to the magic.
    pub fn compute_crc(&self) -> u32 {
        crc32(
            &PageHeader {
                crc: MAGIC,
                ..*self
            }
            .encode(),
        )
    }
}

/// `struct journal_page_list`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageEntry {
    pub delta_start_s: u32,
    pub delta_end_s: u32,
    pub extent_index: u32,
    pub update_every_s: u32,
    pub page_length: u16,
    pub page_type: u8,
}

impl PageEntry {
    pub fn decode(b: &[u8; PAGE_SIZE]) -> Self {
        PageEntry {
            delta_start_s: u32_at(b, 0),
            delta_end_s: u32_at(b, 4),
            extent_index: u32_at(b, 8),
            update_every_s: u32_at(b, 12),
            page_length: u16_at(b, 16),
            page_type: b[18],
        }
    }

    pub fn encode(&self) -> [u8; PAGE_SIZE] {
        let mut b = [0u8; PAGE_SIZE];
        put_u32s(
            &mut b,
            &[
                (0, self.delta_start_s),
                (4, self.delta_end_s),
                (8, self.extent_index),
                (12, self.update_every_s),
            ],
        );
        b[16..18].copy_from_slice(&self.page_length.to_le_bytes());
        b[18] = self.page_type;
        b
    }
}

/// The extent list as read from `extent_offset`.
pub fn extents(b: &[u8]) -> impl Iterator<Item = ExtentEntry> + '_ {
    b.as_chunks().0.iter().map(ExtentEntry::decode)
}

/// The metric list as read from `metric_offset`.
pub fn metrics(b: &[u8]) -> impl Iterator<Item = MetricEntry> + '_ {
    b.as_chunks().0.iter().map(MetricEntry::decode)
}

/// A page list as read after its header.
pub fn pages(b: &[u8]) -> impl Iterator<Item = PageEntry> + '_ {
    b.as_chunks().0.iter().map(PageEntry::decode)
}

/// The size C gives a v2 file (`journalfile_migrate_to_v2_callback()`): the page area assumes one page list per page.
pub fn file_size(extents: usize, metrics: usize, pages: usize) -> usize {
    4096 + EXTENT_SIZE * extents
        + TRAILER_SIZE
        + METRIC_SIZE * metrics
        + TRAILER_SIZE
        + (PAGE_HEADER_SIZE + TRAILER_SIZE + PAGE_SIZE) * pages
        + TRAILER_SIZE
}

/// Why a v2 file is not usable; each maps to what C logs, and C then replays the v1 journal and rebuilds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    /// `Invalid file "%s". Not the expected size`: shorter than the header.
    TooShort,
    Magic,
    /// `journal_v2_file_size` is not the file's size.
    FileSize,
    /// `journal_v1_file_size` is not the v1 journal's size.
    JournalSize,
    /// "file CRC32 check: FAILED".
    HeaderCrc,
    /// "extent list header offsets out of range".
    ExtentBounds,
    /// "extent list CRC32 check: FAILED".
    ExtentCrc,
    /// "metric list header offsets out of range".
    MetricBounds,
    /// "metric list CRC32 check: FAILED".
    MetricCrc,
    /// "verification failed invalid page list header offset -- index %u at offset %u".
    PageListOffset {
        index: u32,
        offset: u32,
    },
    /// "verification failed invalid page list entries -- index %u entries %u at offset %u".
    PageListEntries {
        index: u32,
        entries: u32,
        offset: u32,
    },
    /// "verification failed -- total entries %u, verified %u": page headers or lists whose CRC failed.
    Unverified {
        total: u32,
        verified: u32,
    },
}

/// The outcome of loading a v2 file (`journalfile_v2_load()`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// "is invalid and it will be rebuilt".
    Invalid(Invalid),
    /// The rebuild magic: "needs to be rebuilt".
    Rebuild,
    /// The skip magic: "will be skipped".
    Skip,
    /// Valid but without metrics, which C rebuilds without logging.
    NoMetrics,
}

/// A list of `count` entries of `entry` bytes at `offset` inside a file of `size` bytes, its trailer right after it, in
/// the order C checks it (nothing can overflow).
fn list_in_bounds(
    size: u64,
    (offset, count, trailer_offset): (u32, u32, u32),
    entry: usize,
) -> bool {
    let (off, trailer_at, entry) = (u64::from(offset), u64::from(trailer_offset), entry as u64);
    off <= size
        && u64::from(count) <= (size - off) / entry
        && trailer_at <= size
        && size - trailer_at >= TRAILER_SIZE as u64
        && trailer_at == off + u64::from(count) * entry
}

/// `journalfile_check_v2_extent_list()` and `_metric_list()`: `count` entries of `entry` bytes at `offset` whose CRC
/// is at `trailer_offset`, bounded before anything is read. The list's bytes when it holds.
fn check_list<R: ReadAt + ?Sized>(
    r: &R,
    size: u64,
    (offset, count, trailer_offset): (u32, u32, u32),
    entry: usize,
    (bounds, crc): (Invalid, Invalid),
) -> io::Result<Result<Vec<u8>, Invalid>> {
    if !list_in_bounds(size, (offset, count, trailer_offset), entry) {
        return Ok(Err(bounds));
    }
    let (off, trailer_at) = (u64::from(offset), u64::from(trailer_offset));
    let mut list = vec![0u8; (trailer_at - off) as usize];
    r.read_exact_at(&mut list, off)?;
    let mut stored = [0u8; TRAILER_SIZE];
    r.read_exact_at(&mut stored, trailer_at)?;
    Ok(if crc_matches(&stored, crc32(&list)) {
        Ok(list)
    } else {
        Err(crc)
    })
}

/// `journalfile_v2_load()` from its size check on, with `journalfile_v2_validate()`: the header, the file trailer and
/// the extent list; with `integrity_check` (`db_engine_journal_check`) also every metric and page list. `v1_size` is
/// the journal's size truncated to 32 bits, 0 when it could not be read (the check is then skipped).
pub fn validate<R: ReadAt + ?Sized>(
    r: &R,
    v1_size: u32,
    integrity_check: bool,
) -> io::Result<Verdict> {
    let size = r.size()?;
    if size < HEADER_SIZE as u64 {
        return Ok(Verdict::Invalid(Invalid::TooShort));
    }
    let mut hb = [0u8; HEADER_SIZE];
    r.read_exact_at(&mut hb, 0)?;
    let h = Header::decode(&hb);
    match h.magic {
        REBUILD_MAGIC => return Ok(Verdict::Rebuild),
        SKIP_MAGIC => return Ok(Verdict::Skip),
        MAGIC => {}
        _ => return Ok(Verdict::Invalid(Invalid::Magic)),
    }
    if u64::from(h.journal_v2_file_size) != size {
        return Ok(Verdict::Invalid(Invalid::FileSize));
    }
    if v1_size != 0 && h.journal_v1_file_size != v1_size {
        return Ok(Verdict::Invalid(Invalid::JournalSize));
    }
    let mut trailer = [0u8; TRAILER_SIZE];
    r.read_exact_at(&mut trailer, size - TRAILER_SIZE as u64)?;
    if !crc_matches(&trailer, crc32(&hb)) {
        return Ok(Verdict::Invalid(Invalid::HeaderCrc));
    }
    let extents = (h.extent_offset, h.extent_count, h.extent_trailer_offset);
    let reasons = (Invalid::ExtentBounds, Invalid::ExtentCrc);
    if let Err(invalid) = check_list(r, size, extents, EXTENT_SIZE, reasons)? {
        return Ok(Verdict::Invalid(invalid));
    }
    if integrity_check && let Err(invalid) = validate_metric_list(r, &h)? {
        return Ok(Verdict::Invalid(invalid));
    }
    Ok(if h.metric_count == 0 {
        Verdict::NoMetrics
    } else {
        Verdict::Ok
    })
}

/// The metric list's bounds, which `journalfile_v2_populate_retention_to_mrg()` checks on every load of a file
/// that validated.
pub fn metric_list_in_bounds(h: &Header, size: u64) -> bool {
    list_in_bounds(
        size,
        (h.metric_offset, h.metric_count, h.metric_trailer_offset),
        METRIC_SIZE,
    )
}

/// `journalfile_check_v2_metric_list()`: the metric list's bounds and CRC, which C checks once per file at its first
/// retention load when the integrity check is off (D29). The list's bytes when they hold.
pub fn check_metric_list<R: ReadAt + ?Sized>(
    r: &R,
    h: &Header,
) -> io::Result<Result<Vec<u8>, Invalid>> {
    let list = (h.metric_offset, h.metric_count, h.metric_trailer_offset);
    check_list(
        r,
        r.size()?,
        list,
        METRIC_SIZE,
        (Invalid::MetricBounds, Invalid::MetricCrc),
    )
}

/// The integrity half of `journalfile_v2_validate()`: the metric list and each metric's page header and list. On
/// success the pages verified (C's "total pages", wrapping as its `unsigned` does).
pub fn validate_metric_list<R: ReadAt + ?Sized>(
    r: &R,
    h: &Header,
) -> io::Result<Result<u32, Invalid>> {
    let size = r.size()?;
    let list = match check_metric_list(r, h)? {
        Ok(list) => list,
        Err(invalid) => return Ok(Err(invalid)),
    };
    // C's "EOF reached" guard after each entry cannot fire: the bounds above keep the list inside the file.
    let (mut verified, mut total_pages) = (0u32, 0u32);
    for (index, m) in (0u32..).zip(metrics(&list)) {
        let (po, offset) = (u64::from(m.page_offset), m.page_offset);
        if po > size || size - po < PAGE_HEADER_SIZE as u64 {
            return Ok(Err(Invalid::PageListOffset { index, offset }));
        }
        let mut hb = [0u8; PAGE_HEADER_SIZE];
        r.read_exact_at(&mut hb, po)?;
        let ph = PageHeader::decode(&hb);
        if ph.crc != ph.compute_crc() {
            continue;
        }
        let room = size - po - PAGE_HEADER_SIZE as u64;
        let entries = ph.entries;
        if room < TRAILER_SIZE as u64
            || u64::from(entries) > (room - TRAILER_SIZE as u64) / PAGE_SIZE as u64
        {
            return Ok(Err(Invalid::PageListEntries {
                index,
                entries,
                offset,
            }));
        }
        let mut list = vec![0u8; entries as usize * PAGE_SIZE];
        let at = po + PAGE_HEADER_SIZE as u64;
        r.read_exact_at(&mut list, at)?;
        let mut stored = [0u8; TRAILER_SIZE];
        r.read_exact_at(&mut stored, at + list.len() as u64)?;
        if crc_matches(&stored, crc32(&list)) {
            total_pages = total_pages.wrapping_add(entries);
            verified += 1;
        }
    }
    if verified != h.metric_count {
        let total = h.metric_count;
        return Ok(Err(Invalid::Unverified { total, verified }));
    }
    Ok(Ok(total_pages))
}

#[cfg(test)]
mod tests {
    use super::super::crc::crc_bytes;
    use super::*;

    const V1: u32 = 8192;

    struct Layout {
        extent_offset: usize,
        metric_offset: usize,
        /// Page header offsets, one per metric.
        page_headers: Vec<usize>,
    }

    fn put_crc(b: &mut [u8], from: usize, to: usize) {
        let crc = crc_bytes(crc32(&b[from..to]));
        b[to..to + TRAILER_SIZE].copy_from_slice(&crc);
    }

    fn header(b: &[u8]) -> Header {
        Header::decode(b[..HEADER_SIZE].try_into().unwrap())
    }

    /// Stores `h` and the file CRC over it.
    fn seal(b: &mut [u8], h: &Header) {
        b[..HEADER_SIZE].copy_from_slice(&h.encode());
        let crc = crc_bytes(crc32(&b[..HEADER_SIZE]));
        let end = b.len();
        b[end - TRAILER_SIZE..].copy_from_slice(&crc);
    }

    /// A valid file laid out as C lays it (§4.1): `e` extents and one metric per entry of `pages`.
    fn image(e: usize, pages: &[u32]) -> (Vec<u8>, Layout) {
        let (m, p) = (pages.len(), pages.iter().sum::<u32>() as usize);
        let size = file_size(e, m, p);
        let mut b = vec![0u8; size];
        let extent_offset = 4096;
        for i in 0..e {
            let x = ExtentEntry {
                datafile_offset: 4096 * (i as u64 + 1),
                datafile_size: 100,
                file_index: 0,
                pages: 1,
            };
            let at = extent_offset + i * EXTENT_SIZE;
            b[at..at + EXTENT_SIZE].copy_from_slice(&x.encode());
        }
        let extent_trailer = extent_offset + EXTENT_SIZE * e;
        put_crc(&mut b, extent_offset, extent_trailer);
        let metric_offset = extent_trailer + TRAILER_SIZE;
        let metric_trailer = metric_offset + METRIC_SIZE * m;
        let page_offset = metric_trailer + TRAILER_SIZE;
        let (mut at, mut page_headers) = (page_offset, Vec::new());
        for (i, &n) in pages.iter().enumerate() {
            let uuid = [i as u8 + 1; 16];
            let entry = MetricEntry {
                uuid,
                entries: n,
                page_offset: at as u32,
                delta_start_s: 0,
                delta_end_s: n,
                update_every_s: 1,
            };
            let mo = metric_offset + i * METRIC_SIZE;
            b[mo..mo + METRIC_SIZE].copy_from_slice(&entry.encode());
            let mut ph = PageHeader {
                crc: 0,
                uuid_offset: mo as u32,
                entries: n,
                uuid,
            };
            ph.crc = ph.compute_crc();
            b[at..at + PAGE_HEADER_SIZE].copy_from_slice(&ph.encode());
            page_headers.push(at);
            at += PAGE_HEADER_SIZE;
            let first = at;
            for j in 0..n {
                let pe = PageEntry {
                    delta_start_s: j,
                    delta_end_s: j,
                    update_every_s: 1,
                    ..PageEntry::default()
                };
                b[at..at + PAGE_SIZE].copy_from_slice(&pe.encode());
                at += PAGE_SIZE;
            }
            put_crc(&mut b, first, at);
            at += TRAILER_SIZE;
        }
        put_crc(&mut b, metric_offset, metric_trailer);
        let h = Header {
            magic: MAGIC,
            start_time_ut: 1_000_000,
            end_time_ut: 2_000_000,
            extent_count: e as u32,
            extent_offset: extent_offset as u32,
            metric_count: m as u32,
            metric_offset: metric_offset as u32,
            page_count: p as u32,
            page_offset: page_offset as u32,
            extent_trailer_offset: extent_trailer as u32,
            metric_trailer_offset: metric_trailer as u32,
            journal_v1_file_size: V1,
            journal_v2_file_size: size as u32,
        };
        seal(&mut b, &h);
        let layout = Layout {
            extent_offset,
            metric_offset,
            page_headers,
        };
        (b, layout)
    }

    fn verdict(b: &[u8], integrity: bool) -> Verdict {
        validate(b, V1, integrity).unwrap()
    }

    fn invalid(why: Invalid) -> Verdict {
        Verdict::Invalid(why)
    }

    #[test]
    fn sizes_and_structures_as_c() {
        assert_eq!(file_size(11, 122, 1020), 61_716);
        assert_eq!(file_size(19, 20, 2071), 112_824);
        // the page area holds 32M + 20P bytes; the 32(P - M) left before the file trailer stay zero
        let (b, layout) = image(2, &[3, 1]);
        let used_end = layout.page_headers[1] + PAGE_HEADER_SIZE + PAGE_SIZE + TRAILER_SIZE;
        assert_eq!(b.len() - TRAILER_SIZE - used_end, 32 * (4 - 2));
        let h = header(&b);
        assert_eq!(Header::decode(&h.encode()), h);
        let at = layout.extent_offset;
        let listed: Vec<_> = extents(&b[at..at + 2 * EXTENT_SIZE]).collect();
        assert_eq!(listed[1].datafile_offset, 8192);
        let at = layout.metric_offset;
        let listed: Vec<_> = metrics(&b[at..at + 2 * METRIC_SIZE]).collect();
        assert_eq!(listed[0].page_offset as usize, layout.page_headers[0]);
        let at = layout.page_headers[0] + PAGE_HEADER_SIZE;
        let listed: Vec<_> = pages(&b[at..at + 3 * PAGE_SIZE]).collect();
        assert_eq!(listed[2].delta_start_s, 2);
        let at = layout.page_headers[1];
        let ph = PageHeader::decode(b[at..at + PAGE_HEADER_SIZE].try_into().unwrap());
        assert_eq!(
            (ph.uuid_offset as usize, ph.entries),
            (layout.metric_offset + METRIC_SIZE, 1)
        );
    }

    #[test]
    fn a_valid_file_validates() {
        let (b, _) = image(2, &[3, 1]);
        assert_eq!(verdict(&b, false), Verdict::Ok);
        assert_eq!(verdict(&b, true), Verdict::Ok);
        let h = header(&b);
        assert_eq!(validate_metric_list(&b[..], &h).unwrap(), Ok(4));
    }

    #[test]
    fn magics_and_sizes() {
        let (mut b, _) = image(1, &[1]);
        let h = header(&b);
        for (magic, want) in [
            (REBUILD_MAGIC, Verdict::Rebuild),
            (SKIP_MAGIC, Verdict::Skip),
            (0x0123_0318, invalid(Invalid::Magic)),
        ] {
            let mut c = b.clone();
            c[..4].copy_from_slice(&magic.to_le_bytes());
            assert_eq!(verdict(&c, false), want);
        }
        assert_eq!(
            verdict(&b[..HEADER_SIZE - 1], false),
            invalid(Invalid::TooShort)
        );
        let mut longer = b.clone();
        longer.extend_from_slice(&[0; 4]);
        assert_eq!(verdict(&longer, false), invalid(Invalid::FileSize));
        assert_eq!(
            validate(&b[..], V1 + 1, false).unwrap(),
            invalid(Invalid::JournalSize)
        );
        // an unreadable v1 journal skips its check
        seal(
            &mut b,
            &Header {
                journal_v1_file_size: 1,
                ..h
            },
        );
        assert_eq!(validate(&b[..], 0, false).unwrap(), Verdict::Ok);
    }

    #[test]
    fn the_file_crc_covers_the_header_as_stored() {
        let (mut b, _) = image(1, &[1]);
        b[4] = 1;
        assert_eq!(verdict(&b, false), invalid(Invalid::HeaderCrc));
        let (mut b, _) = image(1, &[1]);
        b[HEADER_SIZE] = 1;
        assert_eq!(
            verdict(&b, false),
            Verdict::Ok,
            "bytes after the header are not covered"
        );
    }

    #[test]
    fn extent_list_bounds_and_crc() {
        let (b, layout) = image(2, &[1]);
        let h = header(&b);
        let size = b.len() as u32;
        let (off, trailer) = (h.extent_offset, h.extent_trailer_offset);
        for bad in [
            Header {
                extent_offset: size + 1,
                ..h
            },
            Header {
                extent_count: (size - off) / 16 + 1,
                ..h
            },
            Header {
                extent_trailer_offset: size + 1,
                ..h
            },
            Header {
                extent_trailer_offset: size - 3,
                ..h
            },
            Header {
                extent_count: 1,
                ..h
            },
            Header {
                extent_trailer_offset: trailer + 4,
                ..h
            },
        ] {
            let mut c = b.clone();
            seal(&mut c, &bad);
            assert_eq!(
                verdict(&c, false),
                invalid(Invalid::ExtentBounds),
                "{bad:?}"
            );
        }
        // the pad byte of an entry is covered
        let mut c = b.clone();
        c[layout.extent_offset + 15] = 1;
        assert_eq!(verdict(&c, false), invalid(Invalid::ExtentCrc));
        // an empty extent list is valid
        let (mut c, _) = image(0, &[1]);
        assert_eq!(verdict(&c, true), Verdict::Ok);
        let h = header(&c);
        seal(
            &mut c,
            &Header {
                extent_trailer_offset: 4100,
                ..h
            },
        );
        assert_eq!(verdict(&c, false), invalid(Invalid::ExtentBounds));
    }

    #[test]
    fn metric_lists_count_only_with_the_integrity_check() {
        let (b, layout) = image(1, &[2, 1]);
        let mut c = b.clone();
        c[layout.metric_offset] ^= 1;
        assert_eq!(verdict(&c, false), Verdict::Ok);
        assert_eq!(verdict(&c, true), invalid(Invalid::MetricCrc));
        let h = header(&b);
        let size = b.len() as u32;
        for bad in [
            Header {
                metric_offset: size + 1,
                ..h
            },
            Header {
                metric_count: size,
                ..h
            },
            Header {
                metric_trailer_offset: size - 1,
                ..h
            },
            Header {
                metric_count: 1,
                ..h
            },
        ] {
            let mut c = b.clone();
            seal(&mut c, &bad);
            assert_eq!(verdict(&c, false), Verdict::Ok);
            assert_eq!(verdict(&c, true), invalid(Invalid::MetricBounds), "{bad:?}");
        }
    }

    /// Rewrites metric `i`'s entry and the metric list CRC.
    fn set_metric(b: &mut [u8], layout: &Layout, i: usize, f: impl Fn(&mut MetricEntry)) {
        let at = layout.metric_offset + i * METRIC_SIZE;
        let mut m = MetricEntry::decode(b[at..at + METRIC_SIZE].try_into().unwrap());
        f(&mut m);
        b[at..at + METRIC_SIZE].copy_from_slice(&m.encode());
        let h = header(b);
        put_crc(b, layout.metric_offset, h.metric_trailer_offset as usize);
    }

    #[test]
    fn page_lists_as_c() {
        let (b, layout) = image(1, &[2, 1]);
        let size = b.len() as u32;
        let mut c = b.clone();
        set_metric(&mut c, &layout, 1, |m| m.page_offset = size - 27);
        let want = Invalid::PageListOffset {
            index: 1,
            offset: size - 27,
        };
        assert_eq!(verdict(&c, true), invalid(want));
        assert_eq!(verdict(&c, false), Verdict::Ok);
        // a failed CRC does not stop the walk: the count decides at the end
        let want = invalid(Invalid::Unverified {
            total: 2,
            verified: 1,
        });
        let mut c = b.clone();
        c[layout.page_headers[0] + 12] ^= 1;
        assert_eq!(verdict(&c, true), want);
        let mut c = b.clone();
        c[layout.page_headers[1] + PAGE_HEADER_SIZE + 19] = 1;
        assert_eq!(verdict(&c, true), want);
        // more entries than the room left, under a valid header CRC; 30 bytes leave less room than a trailer, so
        // that header's uuid keeps the file trailer's bytes
        for (at, entries) in [(layout.page_headers[1], 100), (b.len() - 30, 0)] {
            let mut c = b.clone();
            let mut ph = PageHeader {
                entries,
                uuid: c[at + 12..at + PAGE_HEADER_SIZE].try_into().unwrap(),
                ..PageHeader::default()
            };
            ph.crc = ph.compute_crc();
            c[at..at + PAGE_HEADER_SIZE].copy_from_slice(&ph.encode());
            set_metric(&mut c, &layout, 1, |m| m.page_offset = at as u32);
            let offset = at as u32;
            let want = Invalid::PageListEntries {
                index: 1,
                entries,
                offset,
            };
            assert_eq!(verdict(&c, true), invalid(want));
        }
    }

    #[test]
    fn the_deferred_check_reads_only_the_metric_list() {
        let (mut b, layout) = image(1, &[2, 1]);
        let h = header(&b);
        assert!(metric_list_in_bounds(&h, b.len() as u64));
        assert_eq!(
            check_metric_list(&b[..], &h).unwrap().map(|l| l.len()),
            Ok(2 * METRIC_SIZE)
        );
        // a page list's CRC is the integrity check's business only
        b[layout.page_headers[1] + PAGE_HEADER_SIZE + 19] = 1;
        assert!(check_metric_list(&b[..], &h).unwrap().is_ok());
        let want = Invalid::Unverified {
            total: 2,
            verified: 1,
        };
        assert_eq!(validate_metric_list(&b[..], &h).unwrap(), Err(want));
        b[layout.metric_offset] ^= 1;
        assert_eq!(
            check_metric_list(&b[..], &h).unwrap(),
            Err(Invalid::MetricCrc)
        );
        let bad = Header {
            metric_count: 3,
            ..h
        };
        assert!(!metric_list_in_bounds(&bad, b.len() as u64));
        assert_eq!(
            check_metric_list(&b[..], &bad).unwrap(),
            Err(Invalid::MetricBounds)
        );
    }

    #[test]
    fn a_file_without_metrics_is_rebuilt() {
        let (b, _) = image(0, &[]);
        assert_eq!(b.len(), 4096 + 12);
        assert_eq!(verdict(&b, false), Verdict::NoMetrics);
        assert_eq!(verdict(&b, true), Verdict::NoMetrics);
    }
}
