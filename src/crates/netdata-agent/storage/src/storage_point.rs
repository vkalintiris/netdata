//! `STORAGE_POINT`, ported from `src/libnetdata/storage-point.h`: what every storage engine query returns.

/// One point of a storage engine query, possibly aggregating several stored samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StoragePoint {
    /// The minimum among the aggregated samples.
    pub min: f64,
    /// The maximum among the aggregated samples.
    pub max: f64,
    /// The sum of the aggregated samples; divided by `count` it gives the average.
    pub sum: f64,
    pub start_time_s: i64,
    pub end_time_s: i64,
    /// The number of samples aggregated.
    pub count: u32,
    /// How many of them were anomalous.
    pub anomaly_count: u32,
    /// The user flags stored with the sample (`SN_USER_FLAGS`).
    pub flags: u32,
}

impl StoragePoint {
    /// `STORAGE_POINT_UNSET`.
    pub const UNSET: StoragePoint = StoragePoint {
        min: f64::NAN,
        max: f64::NAN,
        sum: f64::NAN,
        start_time_s: 0,
        end_time_s: 0,
        count: 0,
        anomaly_count: 0,
        flags: 0,
    };

    /// `storage_point_empty()`: a gap that still counts as one point.
    pub fn empty(start_time_s: i64, end_time_s: i64) -> Self {
        StoragePoint {
            start_time_s,
            end_time_s,
            count: 1,
            ..Self::UNSET
        }
    }

    /// `storage_point_is_unset()`.
    pub fn is_unset(&self) -> bool {
        self.count == 0
    }

    /// `storage_point_is_gap()`.
    pub fn is_gap(&self) -> bool {
        !self.sum.is_finite()
    }
}
