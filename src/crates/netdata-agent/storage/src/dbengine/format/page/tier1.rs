//! ARRAY_TIER1 records (`storage_number_tier1_t`): 16 bytes, `f32` sum, min and max, `u16` count and anomaly count.

use crate::storage_number::SN_FLAG_NOT_ANOMALOUS;
use crate::storage_point::StoragePoint;

/// The size of one record.
pub const RECORD_SIZE: usize = 16;

/// `storage_number_tier1_t`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tier1Record {
    pub sum: f32,
    pub min: f32,
    pub max: f32,
    pub count: u16,
    pub anomaly_count: u16,
}

impl Tier1Record {
    /// What the tier writer stores for an aggregate: the values cast to `float`.
    pub fn from_aggregate(sum: f64, min: f64, max: f64, count: u16, anomaly_count: u16) -> Self {
        Tier1Record {
            sum: sum as f32,
            min: min as f32,
            max: max as f32,
            count,
            anomaly_count,
        }
    }

    pub fn encode(&self) -> [u8; RECORD_SIZE] {
        let mut b = [0u8; RECORD_SIZE];
        b[0..4].copy_from_slice(&self.sum.to_le_bytes());
        b[4..8].copy_from_slice(&self.min.to_le_bytes());
        b[8..12].copy_from_slice(&self.max.to_le_bytes());
        b[12..14].copy_from_slice(&self.count.to_le_bytes());
        b[14..16].copy_from_slice(&self.anomaly_count.to_le_bytes());
        b
    }

    pub fn decode(b: &[u8; RECORD_SIZE]) -> Self {
        let f = |at: usize| f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        Tier1Record {
            sum: f(0),
            min: f(4),
            max: f(8),
            count: u16::from_le_bytes([b[12], b[13]]),
            anomaly_count: u16::from_le_bytes([b[14], b[15]]),
        }
    }

    /// The page cursor's point: the values as doubles, the count as stored, not anomalous unless counted so.
    pub fn to_point(&self) -> StoragePoint {
        StoragePoint {
            sum: f64::from(self.sum),
            min: f64::from(self.min),
            max: f64::from(self.max),
            count: u32::from(self.count),
            anomaly_count: u32::from(self.anomaly_count),
            flags: if self.anomaly_count != 0 {
                0
            } else {
                SN_FLAG_NOT_ANOMALOUS
            },
            ..StoragePoint::UNSET
        }
    }
}

/// An ARRAY_TIER1 page up to its used records.
pub fn encode(records: &[Tier1Record]) -> Vec<u8> {
    records.iter().flat_map(Tier1Record::encode).collect()
}

/// `pgd_create_from_disk_data()` for ARRAY_TIER1: `None` below one record; trailing bytes ignored.
pub fn decode(bytes: &[u8]) -> Option<Vec<Tier1Record>> {
    if bytes.len() < RECORD_SIZE {
        return None;
    }
    Some(
        bytes
            .as_chunks::<RECORD_SIZE>()
            .0
            .iter()
            .map(Tier1Record::decode)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_as_c_stores_them() {
        let r = Tier1Record::from_aggregate(10.0, 1.0, 5.0, 3, 0);
        assert_eq!(
            r.encode(),
            [
                0x00, 0x00, 0x20, 0x41, 0x00, 0x00, 0x80, 0x3f, 0x00, 0x00, 0xa0, 0x40, 0x03, 0x00,
                0x00, 0x00
            ]
        );
        assert_eq!(Tier1Record::decode(&r.encode()), r);
        let gap = Tier1Record::from_aggregate(f64::NAN, f64::NAN, f64::NAN, 1, 0);
        assert_eq!(gap.encode()[..4], 0x7FC0_0000u32.to_le_bytes());
    }
}
