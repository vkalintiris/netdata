//! The page formats of dbengine extents (`src/database/engine/page.c`): ARRAY_32BIT storage numbers (tier 0 raw),
//! ARRAY_TIER1 records (tiers above 0) and GORILLA_32BIT (tier 0 default). Brief `knowledge/brief-dbengine-s0.md` §3
//! in the status repository.

pub mod gorilla;
pub mod tier1;

use super::descriptor::{
    PAGE_TYPE_ARRAY_32BIT, PAGE_TYPE_ARRAY_TIER1, PAGE_TYPE_GORILLA_32BIT, point_size,
};
use crate::storage_number::{SN_USER_FLAGS, exists, is_anomalous, pack, unpack};
use crate::storage_point::StoragePoint;
use tier1::Tier1Record;

/// ARRAY_32BIT pages: `u32` storage numbers, little-endian, up to the used slots.
pub fn array32_encode(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// `pgd_create_from_disk_data()` for ARRAY_32BIT: `None` (`PGD_EMPTY`) below one slot; the slots are `size / 4`,
/// trailing bytes ignored.
pub fn array32_decode(bytes: &[u8]) -> Option<Vec<u32>> {
    if bytes.len() < 4 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| u32::from_le_bytes(*c))
            .collect(),
    )
}

/// The page cursor's point for a stored storage number: every slot is a point (an empty slot reads NaN, count 1).
pub fn array32_point(n: u32) -> StoragePoint {
    let v = unpack(n);
    StoragePoint {
        min: v,
        max: v,
        sum: v,
        count: 1,
        anomaly_count: u32::from(is_anomalous(n)),
        flags: n & SN_USER_FLAGS,
        ..StoragePoint::UNSET
    }
}

/// A page read from an extent (`pgd_create_from_disk_data()`).
#[derive(Debug, Clone, PartialEq)]
pub enum DiskPage {
    Array32(Vec<u32>),
    Tier1(Vec<Tier1Record>),
    Gorilla(gorilla::DiskPage),
}

/// Why a page from disk is C's `PGD_EMPTY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyPage {
    /// An unknown type, less than one slot, or a gorilla page C would read past: no record.
    Unfit,
    /// `gorilla_buffer_patch()` failed: C logs "invalid gorilla disk page chain."
    InvalidChain,
}

impl DiskPage {
    /// `pgd_create_from_disk_data()`.
    pub fn load(page_type: u8, bytes: &[u8]) -> Result<DiskPage, EmptyPage> {
        match page_type {
            PAGE_TYPE_ARRAY_32BIT => array32_decode(bytes)
                .map(DiskPage::Array32)
                .ok_or(EmptyPage::Unfit),
            PAGE_TYPE_ARRAY_TIER1 => tier1::decode(bytes)
                .map(DiskPage::Tier1)
                .ok_or(EmptyPage::Unfit),
            PAGE_TYPE_GORILLA_32BIT => gorilla::load(bytes).map(DiskPage::Gorilla),
            _ => Err(EmptyPage::Unfit),
        }
    }

    /// `load()` without the reason: `None` is C's `PGD_EMPTY`.
    pub fn from_disk(page_type: u8, bytes: &[u8]) -> Option<DiskPage> {
        Self::load(page_type, bytes).ok()
    }

    /// `pgd_slots_used()`, which is also `pgd_capacity()` for a page from disk.
    pub fn slots_used(&self) -> usize {
        match self {
            DiskPage::Array32(v) => v.len(),
            DiskPage::Tier1(v) => v.len(),
            DiskPage::Gorilla(g) => usize::from(g.slots),
        }
    }

    /// The bytes the page's data takes in memory, what the caches count against their budgets.
    pub fn footprint(&self) -> usize {
        match self {
            DiskPage::Array32(v) => std::mem::size_of_val(v.as_slice()),
            DiskPage::Tier1(v) => std::mem::size_of_val(v.as_slice()),
            DiskPage::Gorilla(g) => std::mem::size_of_val(g.buffers.as_slice()),
        }
    }

    /// `pgd_is_empty()`: a page from disk is empty only without slots.
    pub fn is_empty(&self) -> bool {
        self.slots_used() == 0
    }

    /// The storage numbers of a tier-0 page up to its slots; a gorilla chain stops at a failed read. `None` for
    /// tier-1 records.
    pub fn storage_numbers(&self) -> Option<Vec<u32>> {
        match self {
            DiskPage::Array32(v) => Some(v.clone()),
            DiskPage::Tier1(_) => None,
            DiskPage::Gorilla(g) => {
                let mut r = gorilla::Reader::new(&g.buffers);
                Some((0..g.slots).map_while(|_| r.read()).collect())
            }
        }
    }

    /// `pgdc_reset()` at `position`.
    pub fn cursor(&self, position: usize) -> Cursor<'_> {
        let (source, slots) = match self {
            DiskPage::Array32(v) => (Source::Array32(v), v.len()),
            DiskPage::Tier1(v) => (Source::Tier1(v), v.len()),
            DiskPage::Gorilla(g) => (
                Source::Gorilla(gorilla::Reader::new(&g.buffers)),
                usize::from(g.slots),
            ),
        };
        Cursor::new(source, slots, position)
    }
}

enum Filling {
    Array32(Vec<u32>),
    Tier1(Vec<Tier1Record>),
    Gorilla(gorilla::Writer),
}

/// A page as the collector fills it (`pgd_create()`, `pgd_append_point()`), until it goes into an extent.
pub struct PageBuilder {
    page_type: u8,
    filling: Filling,
    slots: usize,
    used: usize,
    /// `PAGE_OPTION_ALL_VALUES_EMPTY`: cleared by the first value that exists.
    all_values_empty: bool,
}

impl PageBuilder {
    /// `pgd_create()` with room for `slots` points; `None` for an unknown type.
    pub fn new(page_type: u8, slots: usize) -> Option<PageBuilder> {
        let filling = match page_type {
            PAGE_TYPE_ARRAY_32BIT => Filling::Array32(Vec::with_capacity(slots)),
            PAGE_TYPE_ARRAY_TIER1 => Filling::Tier1(Vec::with_capacity(slots)),
            PAGE_TYPE_GORILLA_32BIT => Filling::Gorilla(gorilla::Writer::default()),
            _ => return None,
        };
        Some(PageBuilder {
            page_type,
            filling,
            slots,
            used: 0,
            all_values_empty: true,
        })
    }

    pub fn page_type(&self) -> u8 {
        self.page_type
    }

    /// `pgd_append_point()`: tier 0 stores `n` packed with `flags`, tier 1 the aggregate. `None` when the page is
    /// full (C calls `fatal()`); otherwise whether a gorilla buffer was added.
    pub fn append(
        &mut self,
        n: f64,
        min: f64,
        max: f64,
        count: u16,
        anomaly_count: u16,
        flags: u32,
    ) -> Option<bool> {
        if self.used >= self.slots {
            return None;
        }
        self.used += 1;
        let (grew, value_exists) = match &mut self.filling {
            Filling::Array32(v) => {
                let t = pack(n, flags);
                v.push(t);
                (false, exists(t))
            }
            Filling::Tier1(v) => {
                v.push(Tier1Record::from_aggregate(
                    n,
                    min,
                    max,
                    count,
                    anomaly_count,
                ));
                (false, !n.is_nan())
            }
            Filling::Gorilla(w) => {
                let t = pack(n, flags);
                (w.append(t), exists(t))
            }
        };
        if value_exists {
            self.all_values_empty = false;
        }
        Some(grew)
    }

    /// The writer of a GORILLA_32BIT page.
    pub fn gorilla(&self) -> Option<&gorilla::Writer> {
        match &self.filling {
            Filling::Gorilla(w) => Some(w),
            _ => None,
        }
    }

    /// `pgd_slots_used()`.
    pub fn slots_used(&self) -> usize {
        self.used
    }

    /// `pgd_capacity()`.
    pub fn capacity(&self) -> usize {
        self.slots
    }

    /// `pgd_is_empty()`: no points, or none that exists.
    pub fn is_empty(&self) -> bool {
        self.used == 0 || self.all_values_empty
    }

    /// `pgd_disk_footprint()`: 0 without points; whole buffers for gorilla, the used slots for arrays.
    pub fn disk_footprint(&self) -> usize {
        if self.used == 0 {
            return 0;
        }
        match &self.filling {
            Filling::Gorilla(w) => w.buffers().len() * gorilla::BUFFER_SIZE,
            _ => self.used * point_size(self.page_type),
        }
    }

    /// `pgd_copy_to_extent()`: the page's bytes as an extent holds them, `disk_footprint()` long.
    pub fn to_extent_bytes(&self) -> Vec<u8> {
        if self.used == 0 {
            return Vec::new();
        }
        match &self.filling {
            Filling::Array32(v) => array32_encode(v),
            Filling::Tier1(v) => tier1::encode(v),
            Filling::Gorilla(w) => w.serialize(),
        }
    }

    /// `pgdc_reset()` at `position` on the page being filled; a gorilla page reads what its writer holds.
    pub fn cursor(&self, position: usize) -> Cursor<'_> {
        let (source, slots) = match &self.filling {
            Filling::Array32(v) => (Source::Array32(v), v.len()),
            Filling::Tier1(v) => (Source::Tier1(v), v.len()),
            Filling::Gorilla(w) => (Source::Gorilla(w.reader()), w.entries() as usize),
        };
        Cursor::new(source, slots, position)
    }
}

enum Source<'a> {
    Array32(&'a [u32]),
    Tier1(&'a [Tier1Record]),
    Gorilla(gorilla::Reader<'a>),
}

/// `PGDC`: a page's points from a position. The points carry no times; the caller sets them.
pub struct Cursor<'a> {
    source: Source<'a>,
    slots: usize,
    position: usize,
}

impl<'a> Cursor<'a> {
    /// `pgdc_seek()`: a gorilla cursor reads its way to `position` (at most the slots), stopping at a failed read.
    fn new(mut source: Source<'a>, slots: usize, position: usize) -> Self {
        if let Source::Gorilla(r) = &mut source {
            for _ in 0..position.min(slots) {
                if r.read().is_none() {
                    break;
                }
            }
        }
        Cursor {
            source,
            slots,
            position,
        }
    }

    /// `pgdc_get_next_point()`: `false` with an empty point past the slots or where a gorilla chain fails.
    pub fn next_point(&mut self) -> (bool, StoragePoint) {
        let empty = (false, StoragePoint::empty(0, 0));
        if self.position >= self.slots {
            return empty;
        }
        let at = self.position;
        self.position += 1;
        match &mut self.source {
            Source::Array32(v) => (true, array32_point(v[at])),
            Source::Tier1(v) => (true, v[at].to_point()),
            Source::Gorilla(r) => r.read().map_or(empty, |n| (true, array32_point(n))),
        }
    }
}
