//! The page formats of dbengine extents (`src/database/engine/page.c`): ARRAY_32BIT storage numbers (tier 0 raw),
//! ARRAY_TIER1 records (tiers above 0) and GORILLA_32BIT (tier 0 default). Brief `knowledge/brief-dbengine-s0.md` §3
//! in the status repository.

pub mod gorilla;
pub mod tier1;

use crate::storage_number::{SN_USER_FLAGS, is_anomalous, unpack};
use crate::storage_point::StoragePoint;

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
