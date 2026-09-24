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

    /// `storage_point_merge_to()`: an unset destination takes the source (even a gap); otherwise a set, non-gap
    /// source widens the times and extremes and adds its sums and counts (and its RESET flag).
    pub fn merge_to(&mut self, src: &StoragePoint) {
        if self.is_unset() {
            *self = *src;
        } else if !src.is_unset() && !src.is_gap() {
            if src.start_time_s < self.start_time_s {
                self.start_time_s = src.start_time_s;
            }
            if src.end_time_s > self.end_time_s {
                self.end_time_s = src.end_time_s;
            }
            if src.min < self.min {
                self.min = src.min;
            }
            if src.max > self.max {
                self.max = src.max;
            }
            self.sum += src.sum;
            self.count += src.count;
            self.anomaly_count += src.anomaly_count;
            self.flags |= src.flags & crate::storage_number::SN_FLAG_RESET;
        }
    }

    /// `storage_point_make_positive()`: negative sum, min and max flip sign one by one; inverted extremes swap.
    pub fn make_positive(&mut self) {
        if self.is_unset() || self.is_gap() {
            return;
        }
        for v in [&mut self.sum, &mut self.min, &mut self.max] {
            if v.is_sign_negative() {
                *v = -*v;
            }
        }
        if self.min > self.max {
            std::mem::swap(&mut self.min, &mut self.max);
        }
    }

    /// `storage_point_anomaly_rate()`.
    pub fn anomaly_rate(&self) -> f64 {
        if self.is_unset() {
            0.0
        } else {
            f64::from(self.anomaly_count) * 100.0 / f64::from(self.count)
        }
    }

    /// `storage_point_average_value()`.
    pub fn average_value(&self) -> f64 {
        if self.count != 0 {
            self.sum / f64::from(self.count)
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    fn point(v: f64, start: i64, end: i64) -> StoragePoint {
        StoragePoint {
            min: v,
            max: v,
            sum: v,
            start_time_s: start,
            end_time_s: end,
            count: 1,
            anomaly_count: 1,
            flags: 0,
        }
    }

    #[test]
    fn merge_positive_and_rates() {
        let mut p = StoragePoint::UNSET;
        p.merge_to(&StoragePoint::empty(1, 2));
        assert!(
            p.is_gap() && !p.is_unset(),
            "an unset destination takes even a gap"
        );
        let mut q = point(2.0, 10, 11);
        q.merge_to(&point(-4.0, 9, 10));
        q.merge_to(&StoragePoint::empty(0, 20));
        assert_eq!(
            (q.start_time_s, q.end_time_s, q.min, q.max, q.sum, q.count),
            (9, 11, -4.0, 2.0, -2.0, 2)
        );
        q.make_positive();
        assert_eq!((q.sum, q.min, q.max), (2.0, 2.0, 4.0));
        assert_eq!(q.anomaly_rate(), 100.0);
        assert_eq!(q.average_value(), 1.0);
        assert_eq!(StoragePoint::UNSET.anomaly_rate(), 0.0);
    }
}
