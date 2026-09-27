//! A fixed log-linear histogram of span durations for the explorer's
//! percentiles.
//!
//! The layout never depends on the data, so histograms from any files,
//! buckets or agents merge by adding counts, and a percentile read from the
//! merge is the same as one read from all the durations at once. Values are
//! reported as their bucket's midpoint, within [`MAX_RELATIVE_ERROR`] of every
//! duration in the bucket (exact below 128 ns); the UI marks them "≈".
//!
//! Layout: bucket 0 holds zero (and any negative input); 1..127 ns get one
//! bucket each; above that, every power of two is split into 64 equal
//! sub-buckets.

use std::collections::BTreeMap;

/// Bits of a power of two resolved into sub-buckets.
pub const SUB_BUCKET_BITS: u32 = 6;
/// Sub-buckets per power of two.
pub const SUB_BUCKETS: u64 = 1 << SUB_BUCKET_BITS;
/// Buckets in the layout: 128 exact ones, then 64 per power of two up to 2^63.
pub const BUCKET_COUNT: usize = 3712;
/// Largest relative distance between a bucket's reported value and any
/// duration it holds.
pub const MAX_RELATIVE_ERROR: f64 = 1.0 / 128.0;

/// The bucket a duration falls in.
pub fn bucket_of(duration_ns: i64) -> u16 {
    if duration_ns <= 0 {
        return 0;
    }
    let d = duration_ns as u64;
    if d < 2 * SUB_BUCKETS {
        return d as u16;
    }
    let e = 63 - u64::from(d.leading_zeros());
    let sub = (d >> (e - u64::from(SUB_BUCKET_BITS))) - SUB_BUCKETS;
    (((e - 5) << SUB_BUCKET_BITS) + sub) as u16
}

/// The half-open range `[low, high)` of durations in bucket `index`.
pub fn bucket_bounds(index: u16) -> (u64, u64) {
    let i = u64::from(index);
    if i < 2 * SUB_BUCKETS {
        return (i, i + 1);
    }
    let e = (i >> SUB_BUCKET_BITS) + 5;
    let width = 1u64 << (e - u64::from(SUB_BUCKET_BITS));
    let low = (SUB_BUCKETS + (i & (SUB_BUCKETS - 1))) * width;
    (low, low.saturating_add(width))
}

/// The value reported for bucket `index`: its midpoint, or the exact value
/// when the bucket is one nanosecond wide.
pub fn bucket_value(index: u16) -> i64 {
    let (low, high) = bucket_bounds(index);
    let width = high - low;
    let mid = if width == 1 { low } else { low + width / 2 };
    i64::try_from(mid).unwrap_or(i64::MAX)
}

/// Rows of the coarse layout the explorer's duration heatmap draws: row 0
/// holds everything under 2^10 ns (about 1 µs), row `k` then holds
/// `[2^(9+k), 2^(10+k))` ns, and the last row everything from 2^40 ns (about
/// 18 minutes).
pub const HEATMAP_ROWS: usize = 32;
const HEATMAP_FIRST_BIT: u32 = 10;

/// The heatmap row a bucket falls in; a bucket never straddles a power of two.
fn heatmap_row(index: u16) -> usize {
    let (low, _) = bucket_bounds(index);
    if low < 1 << HEATMAP_FIRST_BIT {
        return 0;
    }
    let bit = 63 - low.leading_zeros();
    ((bit - HEATMAP_FIRST_BIT + 1) as usize).min(HEATMAP_ROWS - 1)
}

/// Each heatmap row's exclusive upper bound in nanoseconds; `None` for the
/// last, unbounded row.
pub fn heatmap_row_bounds() -> Vec<Option<u64>> {
    (0..HEATMAP_ROWS)
        .map(|row| (row + 1 < HEATMAP_ROWS).then(|| 1u64 << (HEATMAP_FIRST_BIT + row as u32)))
        .collect()
}

/// Durations counted per bucket.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DurationHistogram {
    counts: BTreeMap<u16, u64>,
}

impl DurationHistogram {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record(&mut self, duration_ns: i64) {
        *self.counts.entry(bucket_of(duration_ns)).or_default() += 1;
    }

    pub fn merge(&mut self, other: &DurationHistogram) {
        for (&index, &count) in &other.counts {
            *self.counts.entry(index).or_default() += count;
        }
    }

    pub fn count(&self) -> u64 {
        self.counts.values().sum()
    }

    /// Counts per heatmap row ([`HEATMAP_ROWS`] of them).
    pub fn heatmap_rows(&self) -> Vec<u64> {
        let mut rows = vec![0; HEATMAP_ROWS];
        for (&index, &count) in &self.counts {
            rows[heatmap_row(index)] += count;
        }
        rows
    }

    /// The `percent`-th percentile by nearest rank (rank `ceil(percent · n /
    /// 100)`), as its bucket's value; `None` when nothing was recorded.
    pub fn percentile(&self, percent: u32) -> Option<i64> {
        let n = self.count();
        if n == 0 {
            return None;
        }
        let rank = (u64::from(percent) * n).div_ceil(100).max(1);
        let mut seen = 0;
        for (&index, &count) in &self.counts {
            seen += count;
            if seen >= rank {
                return Some(bucket_value(index));
            }
        }
        self.counts
            .keys()
            .next_back()
            .map(|&index| bucket_value(index))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heatmap_rows_are_powers_of_two() {
        let mut h = DurationHistogram::new();
        for duration in [0, 500, 1_023, 1_024, 1_000_000, 2_500_000_000, 1 << 40, i64::MAX] {
            h.record(duration);
        }
        let mut expected = vec![0; HEATMAP_ROWS];
        expected[0] = 3; // 0, 500, 1023: under 2^10
        expected[1] = 1; // 1024: [2^10, 2^11)
        expected[10] = 1; // 1 ms: [2^19, 2^20)
        expected[22] = 1; // 2.5 s: [2^31, 2^32)
        expected[HEATMAP_ROWS - 1] = 2; // 2^40 and beyond
        assert_eq!(h.heatmap_rows(), expected);

        let bounds = heatmap_row_bounds();
        assert_eq!(bounds.len(), HEATMAP_ROWS);
        assert_eq!((bounds[0], bounds[22]), (Some(1 << 10), Some(1 << 32)));
        assert_eq!(bounds[HEATMAP_ROWS - 1], None);
    }

    #[test]
    fn duration_hist_layout_golden() {
        let cases: [(i64, u16, (u64, u64), i64); 11] = [
            (-5, 0, (0, 1), 0),
            (0, 0, (0, 1), 0),
            (1, 1, (1, 2), 1),
            (127, 127, (127, 128), 127),
            (128, 128, (128, 130), 129),
            (129, 128, (128, 130), 129),
            (255, 191, (254, 256), 255),
            (256, 192, (256, 260), 258),
            (1_000_000, 954, (999_424, 1_007_616), 1_003_520),
            (
                10_000_000_000,
                1_802,
                (9_932_111_872, 10_066_329_600),
                9_999_220_736,
            ),
            (
                i64::MAX,
                3_711,
                (9_151_314_442_816_847_872, 9_223_372_036_854_775_808),
                9_187_343_239_835_811_840,
            ),
        ];
        for (duration, index, bounds, value) in cases {
            assert_eq!(bucket_of(duration), index, "{duration}");
            assert_eq!(bucket_bounds(index), bounds, "{duration}");
            assert_eq!(bucket_value(index), value, "{duration}");
        }
        assert_eq!(usize::from(bucket_of(i64::MAX)) + 1, BUCKET_COUNT);
    }

    #[test]
    fn duration_hist_buckets_tile_the_range() {
        let mut expected_low = 0;
        for index in 0..BUCKET_COUNT as u16 {
            let (low, high) = bucket_bounds(index);
            assert_eq!(
                low, expected_low,
                "bucket {index} starts where the previous ends"
            );
            assert!(high > low);
            expected_low = high;
        }
        assert_eq!(expected_low, 1 << 63);
    }

    #[test]
    fn duration_hist_error_bound() {
        let mut d: i64 = 1;
        while d < i64::MAX / 3 {
            for probe in [d, d + d / 3, d + d / 2, 2 * d - 1] {
                let value = bucket_value(bucket_of(probe));
                let error = (value - probe).abs() as f64 / probe as f64;
                assert!(error <= MAX_RELATIVE_ERROR, "{probe} → {value}: {error}");
            }
            d *= 3;
        }
    }

    #[test]
    fn duration_hist_percentiles_by_nearest_rank() {
        let mut h = DurationHistogram::new();
        assert_eq!(h.percentile(50), None);
        for d in 1..=100 {
            h.record(d);
        }
        assert_eq!(h.percentile(50), Some(50));
        assert_eq!(h.percentile(95), Some(95));
        assert_eq!(h.percentile(99), Some(99));
        assert_eq!(h.percentile(100), Some(100));

        let mut one = DurationHistogram::new();
        one.record(5_000_000);
        assert_eq!(one.percentile(1), one.percentile(99));
    }

    #[test]
    fn duration_hist_merge_is_monoid() {
        let fill = |values: &[i64]| {
            let mut h = DurationHistogram::new();
            for v in values {
                h.record(*v);
            }
            h
        };
        let a = fill(&[0, 3, 900, 1_000_000]);
        let b = fill(&[3, 7_000_000_000]);
        let c = fill(&[12_345]);

        let mut ab_c = a.clone();
        ab_c.merge(&b);
        ab_c.merge(&c);
        let mut bc = b.clone();
        bc.merge(&c);
        let mut a_bc = a.clone();
        a_bc.merge(&bc);
        assert_eq!(ab_c, a_bc);

        let mut ba = b.clone();
        ba.merge(&a);
        let mut ab = a.clone();
        ab.merge(&b);
        assert_eq!(ab, ba);

        let mut with_empty = a.clone();
        with_empty.merge(&DurationHistogram::new());
        assert_eq!(with_empty, a);
        assert_eq!(
            ab_c,
            fill(&[0, 3, 900, 1_000_000, 3, 7_000_000_000, 12_345])
        );
    }
}
