//! The page descriptor of an extent (`struct rrdeng_extent_page_descr`, 37 packed bytes in `rrddiskprotocol.h`) and C's
//! page validation (`validate_extent_page_descr()`, `validate_page()` in `pdc.c`). Brief
//! `knowledge/brief-dbengine-s0.md` §2.2 and §2.6 in the status repository.

/// `RRDENG_PAGE_TYPE_*`.
pub const PAGE_TYPE_ARRAY_32BIT: u8 = 0;
pub const PAGE_TYPE_ARRAY_TIER1: u8 = 1;
pub const PAGE_TYPE_GORILLA_32BIT: u8 = 2;

/// The descriptor's size on disk.
pub const DESCRIPTOR_SIZE: usize = 37;

/// `RRDENG_GORILLA_32BIT_BUFFER_SIZE`: a gorilla page grows by buffers of this size.
pub const GORILLA_BUFFER_SIZE: usize = 512;

/// `page_type_size[]`: the bytes of one point, 0 for an unknown type.
pub fn point_size(page_type: u8) -> usize {
    match page_type {
        PAGE_TYPE_ARRAY_32BIT | PAGE_TYPE_GORILLA_32BIT => 4,
        PAGE_TYPE_ARRAY_TIER1 => 16,
        _ => 0,
    }
}

/// One page of an extent, as its descriptor stores it. The last 8 bytes stay raw: an end time for array pages,
/// entries and a duration for gorilla pages, whatever for unknown types (which then round-trip unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageDescriptor {
    pub page_type: u8,
    pub uuid: [u8; 16],
    pub page_length: u32,
    /// The end of the first point, in microseconds.
    pub start_time_ut: u64,
    pub tail: [u8; 8],
}

impl PageDescriptor {
    /// An array page's descriptor (`end_time_ut` in the tail).
    pub fn array(
        page_type: u8,
        uuid: [u8; 16],
        page_length: u32,
        start_time_ut: u64,
        end_time_ut: u64,
    ) -> Self {
        PageDescriptor {
            page_type,
            uuid,
            page_length,
            start_time_ut,
            tail: end_time_ut.to_le_bytes(),
        }
    }

    /// A gorilla page's descriptor: its entries and `(end_ut - start_ut) / 1e6` seconds.
    pub fn gorilla(
        uuid: [u8; 16],
        page_length: u32,
        start_time_ut: u64,
        entries: u32,
        delta_time_s: u32,
    ) -> Self {
        let mut tail = [0u8; 8];
        tail[..4].copy_from_slice(&entries.to_le_bytes());
        tail[4..].copy_from_slice(&delta_time_s.to_le_bytes());
        PageDescriptor {
            page_type: PAGE_TYPE_GORILLA_32BIT,
            uuid,
            page_length,
            start_time_ut,
            tail,
        }
    }

    /// Array pages: the end of the last point, in microseconds.
    pub fn end_time_ut(&self) -> u64 {
        u64::from_le_bytes(self.tail)
    }

    /// Gorilla pages: the number of points.
    pub fn gorilla_entries(&self) -> u32 {
        u32::from_le_bytes([self.tail[0], self.tail[1], self.tail[2], self.tail[3]])
    }

    /// Gorilla pages: the seconds from the first point to the last.
    pub fn gorilla_delta_s(&self) -> u32 {
        u32::from_le_bytes([self.tail[4], self.tail[5], self.tail[6], self.tail[7]])
    }

    /// The descriptor at the start of `b` (at least 37 bytes).
    pub fn decode(b: &[u8]) -> Option<Self> {
        let b = b.get(..DESCRIPTOR_SIZE)?;
        let mut uuid = [0u8; 16];
        uuid.copy_from_slice(&b[1..17]);
        let mut start = [0u8; 8];
        start.copy_from_slice(&b[21..29]);
        let mut tail = [0u8; 8];
        tail.copy_from_slice(&b[29..37]);
        Some(PageDescriptor {
            page_type: b[0],
            uuid,
            page_length: u32::from_le_bytes([b[17], b[18], b[19], b[20]]),
            start_time_ut: u64::from_le_bytes(start),
            tail,
        })
    }

    pub fn encode(&self) -> [u8; DESCRIPTOR_SIZE] {
        let mut b = [0u8; DESCRIPTOR_SIZE];
        b[0] = self.page_type;
        b[1..17].copy_from_slice(&self.uuid);
        b[17..21].copy_from_slice(&self.page_length.to_le_bytes());
        b[21..29].copy_from_slice(&self.start_time_ut.to_le_bytes());
        b[29..37].copy_from_slice(&self.tail);
        b
    }
}

/// `VALIDATED_PAGE_DESCRIPTOR`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedPage {
    pub start_time_s: i64,
    pub end_time_s: i64,
    pub update_every_s: u32,
    pub page_length: usize,
    pub point_size: usize,
    pub entries: usize,
    pub page_type: u8,
    pub valid: bool,
    /// Whether validation repaired the page's end or update every (C then logs it).
    pub updated: bool,
}

/// What `validate_page()` needs about a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageFacts {
    pub start_time_s: i64,
    pub end_time_s: i64,
    /// 0 when unknown (loading from disk).
    pub update_every_s: u32,
    pub page_length: usize,
    pub page_type: u8,
    /// 0 when unknown.
    pub entries: usize,
}

/// `validate_page()` without its log record: C's integer types and their wrapping arithmetic kept (`time_t`, `uint32_t`
/// update every, `size_t` entries). `now_s` 0 skips the future check; `overwrite_ue` is the update every to fall back
/// on (the metric registry's, or 0).
pub fn validate_page(
    p: PageFacts,
    now_s: i64,
    overwrite_ue: u32,
    read_error: bool,
) -> ValidatedPage {
    let point_size = point_size(p.page_type);
    let mut vd = ValidatedPage {
        start_time_s: p.start_time_s,
        end_time_s: p.end_time_s,
        update_every_s: p.update_every_s,
        page_length: p.page_length,
        point_size,
        entries: 0,
        page_type: p.page_type,
        valid: true,
        updated: false,
    };
    let mut entries = p.entries;
    let known = match p.page_type {
        PAGE_TYPE_ARRAY_32BIT | PAGE_TYPE_ARRAY_TIER1 => {
            vd.entries = vd.page_length / point_size;
            if entries == 0 {
                entries = vd.entries;
            }
            true
        }
        PAGE_TYPE_GORILLA_32BIT => {
            vd.entries = entries;
            true
        }
        _ => false,
    };
    let mut update_every_s = p.update_every_s;
    if update_every_s == 0 {
        vd.update_every_s = if vd.entries > 1 {
            ((vd.end_time_s.wrapping_sub(vd.start_time_s) as u32 as u64) / (vd.entries as u64 - 1))
                as u32
        } else {
            overwrite_ue
        };
        update_every_s = vd.update_every_s;
    }
    let max_page_length =
        4096 + usize::from(p.page_type == PAGE_TYPE_GORILLA_32BIT) * 2 * GORILLA_BUFFER_SIZE;
    if !known
        || read_error
        || vd.page_length == 0
        || vd.page_length > max_page_length
        || vd.start_time_s > vd.end_time_s
        || (now_s != 0 && vd.end_time_s > now_s)
        || vd.start_time_s <= 0
        || vd.end_time_s <= 0
        || (vd.start_time_s == vd.end_time_s && vd.entries > 1)
        || (vd.update_every_s == 0 && vd.entries > 1)
    {
        vd.valid = false;
        return vd;
    }
    let mut updated = vd.entries != entries || vd.update_every_s != update_every_s;
    if vd.update_every_s != 0 {
        // page_entries_by_time(): time_t arithmetic, compared as size_t
        let ue = i64::from(vd.update_every_s);
        let entries_by_time = vd.end_time_s.wrapping_sub(vd.start_time_s.wrapping_sub(ue)) / ue;
        if vd.entries as u64 != entries_by_time as u64 {
            if overwrite_ue < vd.update_every_s {
                vd.update_every_s = overwrite_ue;
            }
            let end_after = |ue: u32| {
                (vd.start_time_s as u64).wrapping_add(
                    (vd.entries as u64)
                        .wrapping_sub(1)
                        .wrapping_mul(u64::from(ue)),
                ) as i64
            };
            let new_end_time_s = end_after(vd.update_every_s);
            if new_end_time_s <= vd.end_time_s {
                // the end time is wrong
                vd.end_time_s = new_end_time_s;
            } else {
                // the update every is wrong
                vd.update_every_s = overwrite_ue;
                vd.end_time_s = end_after(overwrite_ue);
            }
            updated = true;
        }
    } else if overwrite_ue != 0 {
        vd.update_every_s = overwrite_ue;
        updated = true;
    }
    vd.updated = updated;
    vd
}

/// `validate_extent_page_descr()`: a page read from an extent, its update every derived.
pub fn validate_extent_page_descr(
    d: &PageDescriptor,
    now_s: i64,
    overwrite_ue: u32,
    read_error: bool,
) -> ValidatedPage {
    let start_time_s = (d.start_time_ut / 1_000_000) as i64;
    let (end_time_s, entries) = match d.page_type {
        PAGE_TYPE_ARRAY_32BIT | PAGE_TYPE_ARRAY_TIER1 => ((d.end_time_ut() / 1_000_000) as i64, 0),
        PAGE_TYPE_GORILLA_32BIT => (
            start_time_s.wrapping_add(i64::from(d.gorilla_delta_s())),
            d.gorilla_entries() as usize,
        ),
        _ => (0, 0),
    };
    validate_page(
        PageFacts {
            start_time_s,
            end_time_s,
            update_every_s: 0,
            page_length: d.page_length as usize,
            page_type: d.page_type,
            entries,
        },
        now_s,
        overwrite_ue,
        read_error,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u64 = 1_700_000_000;

    #[test]
    fn descriptors_round_trip_at_cs_offsets() {
        let d = PageDescriptor::array(
            PAGE_TYPE_ARRAY_TIER1,
            [7; 16],
            64,
            T * 1_000_000,
            (T + 3) * 1_000_000,
        );
        let b = d.encode();
        assert_eq!(b[0], 1);
        assert_eq!(&b[1..17], &[7; 16]);
        assert_eq!(&b[17..21], &64u32.to_le_bytes());
        assert_eq!(&b[21..29], &(T * 1_000_000).to_le_bytes());
        assert_eq!(&b[29..37], &((T + 3) * 1_000_000).to_le_bytes());
        assert_eq!(PageDescriptor::decode(&b), Some(d));
        let g = PageDescriptor::gorilla([1; 16], 512, T * 1_000_000, 60, 59);
        let b = g.encode();
        assert_eq!(&b[29..33], &60u32.to_le_bytes());
        assert_eq!(&b[33..37], &59u32.to_le_bytes());
        assert_eq!((g.gorilla_entries(), g.gorilla_delta_s()), (60, 59));
        assert_eq!(PageDescriptor::decode(&b[..36]), None);
        // an unknown type round-trips
        let mut raw = b;
        raw[0] = 9;
        assert_eq!(PageDescriptor::decode(&raw).unwrap().encode(), raw);
    }

    fn array(start: i64, end: i64, points: usize) -> PageFacts {
        PageFacts {
            start_time_s: start,
            end_time_s: end,
            update_every_s: 0,
            page_length: points * 4,
            page_type: PAGE_TYPE_ARRAY_32BIT,
            entries: 0,
        }
    }

    #[test]
    fn validation_as_c() {
        let t = T as i64;
        // a regular page: 60 points a second apart
        let v = validate_page(array(t, t + 59, 60), 0, 0, false);
        assert!(v.valid && !v.updated);
        assert_eq!((v.update_every_s, v.entries, v.end_time_s), (1, 60, t + 59));
        // each invalid rule
        let invalid =
            |p: PageFacts, now: i64, read_error: bool| !validate_page(p, now, 0, read_error).valid;
        assert!(invalid(
            PageFacts {
                page_type: 7,
                ..array(t, t + 59, 60)
            },
            0,
            false
        ));
        assert!(invalid(array(t, t + 59, 60), 0, true));
        assert!(invalid(array(t, t + 59, 0), 0, false));
        assert!(invalid(array(t, t + 59, 1025), 0, false));
        assert!(invalid(array(t + 60, t + 59, 60), 0, false));
        assert!(invalid(array(t, t + 59, 60), t + 58, false));
        assert!(invalid(array(0, t, 60), 0, false));
        assert!(invalid(array(t, t, 60), 0, false));
        // gorilla pages may be 1024 bytes longer
        let gorilla = |len: usize| PageFacts {
            page_type: PAGE_TYPE_GORILLA_32BIT,
            page_length: len,
            entries: 60,
            ..array(t, t + 59, 60)
        };
        assert!(validate_page(gorilla(5120), 0, 0, false).valid);
        assert!(!validate_page(gorilla(5121), 0, 0, false).valid);
        // the end is wrong: 60 points at 1 s cannot span 100 s, so the end moves back
        let v = validate_page(array(t, t + 100, 60), 0, 1, false);
        assert!(v.valid && v.updated);
        assert_eq!((v.update_every_s, v.end_time_s), (1, t + 59));
        // with no fallback the update every drops to 0 and the page ends where it starts, still valid
        let v = validate_page(array(t, t + 100, 60), 0, 0, false);
        assert!(v.valid && v.updated);
        assert_eq!((v.update_every_s, v.end_time_s), (0, t));
        // a single-point page takes the fallback update every
        let v = validate_page(array(t, t, 1), 0, 5, false);
        assert!(v.valid && !v.updated);
        assert_eq!(v.update_every_s, 5);
    }

    #[test]
    fn extent_descriptors_derive_their_times() {
        let d = PageDescriptor::gorilla([1; 16], 512, T * 1_000_000, 60, 59);
        let v = validate_extent_page_descr(&d, 0, 0, false);
        assert!(v.valid);
        assert_eq!(
            (v.start_time_s, v.end_time_s, v.update_every_s, v.entries),
            (T as i64, T as i64 + 59, 1, 60)
        );
        let d = PageDescriptor::array(
            PAGE_TYPE_ARRAY_32BIT,
            [1; 16],
            8,
            T * 1_000_000,
            (T + 2) * 1_000_000,
        );
        let v = validate_extent_page_descr(&d, 0, 0, false);
        assert_eq!((v.entries, v.update_every_s), (2, 2));
    }
}
