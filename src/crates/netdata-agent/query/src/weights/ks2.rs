//! The two-sample Kolmogorov-Smirnov test of the `ks2` weights method (`weights.c`, "KS2 algorithm functions"):
//! the changes between consecutive points of the baseline and of the highlight, as integers; the largest distance
//! between the two sets' distributions; and the probability [`ks_fbar`] gives for a distance that large.

use super::ks::ks_fbar;

/// `MAX_POINTS`: the most points a weights request may ask of the baseline, after its shifts.
pub const MAX_POINTS: usize = 10_000;

/// `weights_points_exceed_max()`: whether `points << shifts` passes [`MAX_POINTS`], asked without shifting the
/// points, so nothing overflows.
pub fn points_exceed_max(points: usize, shifts: u32) -> bool {
    if shifts >= usize::BITS {
        return points != 0;
    }
    points > (MAX_POINTS >> shifts)
}

/// `DOUBLE_TO_INT_MULTIPLIER`: a change is kept to five decimals.
const DOUBLE_TO_INT_MULTIPLIER: f64 = 100_000.0;

/// C's `(long)` of a double as x86-64 makes it (`cvttsd2si`): the value cut towards zero, and the smallest integer
/// for a NaN or a value out of range, where Rust's `as` gives 0 and the nearest end.
fn c_long(value: f64) -> i64 {
    const TWO_63: f64 = (1_u64 << 63) as f64;
    if value.is_nan() || value >= TWO_63 || value < -TWO_63 {
        return i64::MIN;
    }
    value as i64
}

/// `calculate_pairs_diff()`: each value's change from the one before it, from the last pair back, times 100000, as
/// an integer: one fewer than the values.
pub fn pairs_diff(values: &[f64]) -> Vec<i64> {
    let diff = |pair: &[f64]| c_long((pair[0] - pair[1]) * DOUBLE_TO_INT_MULTIPLIER);
    values.windows(2).rev().map(diff).collect()
}

/// `cursor_bigger_than()`: the first index from `cursor` on whose value is above `key`.
fn cursor_bigger_than(sorted: &[i64], mut cursor: usize, key: i64) -> usize {
    while cursor < sorted.len() && sorted[cursor] <= key {
        cursor += 1;
    }
    cursor
}

/// `ks_2samp()`: the probability that two sets of changes as far apart as these come from one distribution. Both
/// sets are sorted in place. The baseline is `1 << base_shifts` times as long as the highlight when the request's
/// windows are whole; whatever their lengths, the statistic is the largest distance between the two step functions
/// "how much of the set is at most this value", found at the baseline's values first, then the highlight's.
///
/// NaN for a shift that an `i64` cannot hold and for an empty set (C's caller refuses the second before it calls).
pub fn ks_2samp(baseline_diffs: &mut [i64], highlight_diffs: &mut [i64], base_shifts: u32) -> f64 {
    // the highlight's index is scaled by the ratio; keep the scaled comparison representable
    if base_shifts >= i64::BITS - 1 {
        return f64::NAN;
    }
    let high_multiplier = 1i64 << base_shifts;
    let (base_len, high_len) = (baseline_diffs.len(), highlight_diffs.len());
    if base_len == 0 || high_len == 0 || high_len as i64 > i64::MAX / high_multiplier {
        return f64::NAN;
    }
    baseline_diffs.sort_unstable();
    highlight_diffs.sort_unstable();
    let (baseline, highlight): (&[i64], &[i64]) = (baseline_diffs, highlight_diffs);

    // For each value of either set: the count of each set's values at most it (an upper bound in each sorted set),
    // and the difference of the two counts once the highlight's is scaled to the baseline's length. C compares the
    // scaled counts as integers, and keeps the indexes of the smallest and of the largest difference to divide
    // them properly afterwards. Sorted values let each upper bound only advance.
    let visit = |base_idx: usize, high_idx: usize| -> Visit {
        (base_idx as i64 - high_idx as i64 * high_multiplier, base_idx, high_idx)
    };
    let mut base_idx = cursor_bigger_than(baseline, 1, baseline[0]);
    let mut high_idx = cursor_bigger_than(highlight, 0, baseline[0]);
    let mut min = visit(base_idx, high_idx);
    let mut max = min;
    for &key in &baseline[1..] {
        base_idx = cursor_bigger_than(baseline, base_idx, key);
        high_idx = cursor_bigger_than(highlight, high_idx, key);
        keep_extremes(&mut min, &mut max, visit(base_idx, high_idx));
    }
    // the highlight's values after the baseline's, both cursors from the start again
    (base_idx, high_idx) = (0, 0);
    for &key in highlight {
        base_idx = cursor_bigger_than(baseline, base_idx, key);
        high_idx = cursor_bigger_than(highlight, high_idx, key);
        keep_extremes(&mut min, &mut max, visit(base_idx, high_idx));
    }
    probability(baseline.len(), highlight.len(), min, max)
}

/// One value's upper bounds in the two sorted sets: the scaled difference of the two, the baseline's index, the
/// highlight's.
type Visit = (i64, usize, usize);

/// The smallest and the largest difference so far: the first to reach a difference keeps it.
fn keep_extremes(min: &mut Visit, max: &mut Visit, visit: Visit) {
    if visit.0 < min.0 {
        *min = visit;
    } else if visit.0 > max.0 {
        *max = visit;
    }
}

/// The end of `ks_2samp()`: the two extreme differences as fractions of their sets, the statistic (the larger of
/// the two distances, within 0 and 1), the sample size of the two sets together, and the probability.
fn probability(base_size: usize, high_size: usize, min: Visit, max: Visit) -> f64 {
    let (base_size, high_size) = (base_size as f64, high_size as f64);
    let dmin = (min.1 as f64 / base_size) - (min.2 as f64 / high_size);
    let dmax = (max.1 as f64 / base_size) - (max.2 as f64 / high_size);
    let dmin = -dmin;
    // C's two tests, not a clamp: a negative zero (the two fractions equal) becomes zero
    #[allow(clippy::manual_clamp)]
    let dmin = if dmin <= 0.0 {
        0.0
    } else if dmin >= 1.0 {
        1.0
    } else {
        dmin
    };
    let d = if dmin >= dmax { dmin } else { dmax };

    let en = (base_size * high_size / (base_size + high_size)).round();
    // under these conditions, KSfbar() crashes
    if en.is_nan() || en.is_infinite() || en == 0.0 || d.is_nan() || d.is_infinite() {
        return f64::NAN;
    }
    // at most the smaller set's length: C's `(int)` cuts nothing
    ks_fbar(en as i32, d)
}

/// `kstwo()`: [`ks_2samp`] of the changes of two series of points; NaN unless each has two points at the least.
pub fn kstwo(baseline: &[f64], highlight: &[f64], base_shifts: u32) -> f64 {
    if baseline.len() <= 1 || highlight.len() <= 1 {
        return f64::NAN;
    }
    ks_2samp(&mut pairs_diff(baseline), &mut pairs_diff(highlight), base_shifts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weights::vector::{Vector, double};

    /// C's `ks2_test_random()`: xorshift64.
    fn random(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    /// The 20,000 trials of C's `ks2_cursor_unittest()`, from its seed, as (baseline, highlight, shifts): sets of 1
    /// to 64 changes between -8 and 8, so full of ties, of unequal lengths, under shifts up to 9; every third
    /// baseline starts with the smallest integer and every fifth highlight with the largest; the last twenty are
    /// as long as a request allows (9999, against 14 or 9999).
    fn trials() -> impl Iterator<Item = (Vec<i64>, Vec<i64>, u32)> {
        let mut state = 0x9182_abcd_1234_u64;
        (0..20_000_u64).map(move |trial| {
            let mut base_size = 1 + (random(&mut state) % 64) as usize;
            let mut high_size = 1 + (random(&mut state) % 64) as usize;
            let mut shifts = (random(&mut state) % 10) as u32;
            if trial >= 19_980 {
                base_size = MAX_POINTS - 1;
                high_size = if trial % 2 == 1 { 14 } else { MAX_POINTS - 1 };
                shifts = (trial % 10) as u32;
            }
            let mut change = |_| (random(&mut state) % 17) as i64 - 8;
            let mut base: Vec<i64> = (0..base_size).map(&mut change).collect();
            let mut high: Vec<i64> = (0..high_size).map(&mut change).collect();
            if trial % 3 == 0 {
                base[0] = i64::MIN;
            }
            if trial % 5 == 0 {
                high[0] = i64::MAX;
            }
            (base, high, shifts)
        })
    }

    /// C's `binary_search_bigger_than()`: the first index from `left` on whose value is above `key`, by halving.
    fn binary_search_bigger_than(sorted: &[i64], mut left: usize, key: i64) -> usize {
        let mut right = sorted.len();
        while left < right {
            let middle = (left + right) >> 1;
            if sorted[middle] > key {
                right = middle;
            } else {
                left = middle + 1;
            }
        }
        left
    }

    /// C's `ks_2samp_binary_reference()`: the walk as it was before the cursors, each upper bound found by halving.
    fn ks_2samp_binary_reference(baseline: &mut [i64], highlight: &mut [i64], base_shifts: u32) -> f64 {
        let high_multiplier = 1_i64 << base_shifts;
        baseline.sort_unstable();
        highlight.sort_unstable();
        let (baseline, highlight): (&[i64], &[i64]) = (baseline, highlight);
        let visit = |base_idx: usize, high_idx: usize| -> Visit {
            (base_idx as i64 - high_idx as i64 * high_multiplier, base_idx, high_idx)
        };
        let upper = binary_search_bigger_than;
        let mut min = visit(upper(baseline, 1, baseline[0]), upper(highlight, 0, baseline[0]));
        let mut max = min;
        for (i, &key) in baseline.iter().enumerate().skip(1) {
            keep_extremes(&mut min, &mut max, visit(upper(baseline, i + 1, key), upper(highlight, 0, key)));
        }
        for (i, &key) in highlight.iter().enumerate() {
            keep_extremes(&mut min, &mut max, visit(upper(baseline, 0, key), upper(highlight, i + 1, key)));
        }
        probability(baseline.len(), highlight.len(), min, max)
    }

    /// C's `ks2_cursor_unittest()`: over its trials the cursor walk answers as the binary search it replaced, and
    /// every pair of upper bounds the cursors reach is the binary search's, ties included.
    #[test]
    fn the_cursor_walk_equals_the_binary_search() {
        for (trial, (mut base, mut high, shifts)) in trials().enumerate() {
            let expected = ks_2samp_binary_reference(&mut base.clone(), &mut high.clone(), shifts);
            let actual = ks_2samp(&mut base, &mut high, shifts);
            let equal = !actual.is_nan() && actual.to_bits() == expected.to_bits();
            assert!(equal, "trial {trial}: {actual:e}, {expected:e}");
            // both are sorted now
            for keys in [&base, &high] {
                let (mut base_idx, mut high_idx) = (0, 0);
                for &key in keys {
                    base_idx = cursor_bigger_than(&base, base_idx, key);
                    high_idx = cursor_bigger_than(&high, high_idx, key);
                    let halved = (binary_search_bigger_than(&base, 0, key), binary_search_bigger_than(&high, 0, key));
                    assert_eq!((base_idx, high_idx), halved, "trial {trial}, key {key}");
                }
            }
        }
    }

    /// The same trials against the answers C's own `ks_2samp()` gave (`tests/oracle/gen-ks-vectors.c` runs the
    /// function's text, cut from `weights.c`): bit for bit on the vector's class of machine ([`Vector`]). Then the
    /// changes `calculate_pairs_diff()` made of pairs of doubles, among them those no integer holds: the cast is
    /// x86-64's on every machine.
    #[test]
    fn ks_2samp_answers_as_c() {
        let vector = Vector::read("ks2samp.txt");
        let mut lines = vector.lines.iter();
        for (trial, (mut base, mut high, shifts)) in trials().enumerate() {
            let line = lines.next().and_then(|line| line.strip_prefix("trial "));
            let expected = double(line.unwrap_or_else(|| panic!("no line for trial {trial}")));
            vector.assert_as_c(ks_2samp(&mut base, &mut high, shifts), expected, format_args!("trial {trial}"));
        }
        let mut casts = 0;
        for line in lines {
            let mut fields = line.strip_prefix("diff ").expect("a diff line").split(' ');
            let (first, second) = (double(fields.next().unwrap()), double(fields.next().unwrap()));
            let change = u64::from_str_radix(fields.next().unwrap(), 16).unwrap() as i64;
            assert_eq!(pairs_diff(&[first, second]), [change], "{first:e} - {second:e}");
            casts += 1;
        }
        assert!(casts > 100, "{casts} casts");
        vector.report("ks2samp", 20_000);
    }

    /// C's `mc_unittest1()` to `mc_unittest4()`: four known answers, compared as C compares them, by their six
    /// decimals; and a shift an `i64` cannot hold is NaN.
    #[test]
    fn c_s_four_known_answers() {
        let answer = |baseline: &[i64], highlight: &[i64], shifts: u32| {
            format!("{:.6}", ks_2samp(&mut baseline.to_vec(), &mut highlight.to_vec(), shifts))
        };
        assert_eq!(answer(&[1, 2, 3], &[3, 4, 6], 0), "0.222222");
        assert_eq!(answer(&[1, 2, 3, 10, 10, 15], &[3, 4, 6], 1), "0.500000");
        assert_eq!(answer(&[1, 2, 3, 10, 10, 15, 111, 19999, 8, 55, -1, -73], &[3, 4, 6], 2), "0.347222");
        let base = [1111, -2222, 33, 100, 100, 15555, -1, 19999, 888, 755, -1, -730];
        assert_eq!(answer(&base, &[365, -123, 0], 2), "0.777778");
        assert!(ks_2samp(&mut [1], &mut [1], i64::BITS).is_nan());
        assert!(ks_2samp(&mut [1], &mut [1], i64::BITS - 1).is_nan());
        assert!(!ks_2samp(&mut [1], &mut [1], i64::BITS - 2).is_nan());
        // more highlight values than the scaled comparison can hold
        assert!(ks_2samp(&mut [1], &mut [1, 2], i64::BITS - 2).is_nan());
        // an empty set, which C's caller refuses before it calls
        assert!(ks_2samp(&mut [], &mut [1], 0).is_nan() && ks_2samp(&mut [1], &mut [], 0).is_nan());
    }

    /// C's `weights_points_exceed_max_unittest()`, its fifteen cases.
    #[test]
    fn the_point_limit_never_overflows() {
        let cases = [
            (0, u32::MAX, false),
            (MAX_POINTS - 1, 0, false),
            (MAX_POINTS, 0, false),
            (MAX_POINTS + 1, 0, true),
            (MAX_POINTS / 2, 1, false),
            (MAX_POINTS / 2 + 1, 1, true),
            (1, 13, false),
            (2, 13, true),
            (1, 14, true),
            (1 << (usize::BITS - 1), 1, true),
            (usize::MAX, 0, true),
            (usize::MAX, 29, true),
            (1, usize::BITS - 1, true),
            (1, usize::BITS, true),
            (1, u32::MAX, true),
        ];
        for (points, shifts, expected) in cases {
            assert_eq!(points_exceed_max(points, shifts), expected, "{points} points, {shifts} shifts");
        }
    }

    /// The changes run from the last pair back, are cut towards zero at five decimals, and a change no integer
    /// holds is the smallest integer, as C's cast makes it on x86-64 (Rust's own cast would give the nearest end,
    /// and 0 for a NaN).
    #[test]
    fn the_changes_are_c_s_integers() {
        assert_eq!(pairs_diff(&[1.0, 3.5, 3.0]), [50_000, -250_000]);
        assert_eq!(pairs_diff(&[0.000019, 0.0, -0.000019]), [1, 1]);
        assert_eq!(pairs_diff(&[1e14, 0.0]), [i64::MIN]);
        assert_eq!(pairs_diff(&[0.0, 1e14]), [i64::MIN]);
        assert_eq!(pairs_diff(&[f64::NAN, 0.0, f64::INFINITY]), [i64::MIN, i64::MIN]);
        assert_eq!(pairs_diff(&[9.2e13, 0.0]), [9_200_000_000_000_000_000]);
        assert!(pairs_diff(&[1.0]).is_empty() && pairs_diff(&[]).is_empty());
        // one point is no change: NaN, as for a missing series
        assert!(kstwo(&[1.0], &[1.0, 2.0], 0).is_nan() && kstwo(&[1.0, 2.0], &[], 0).is_nan());
        // the series of C's first known answer, as points
        // (each change sits half a unit above its integer, so the cut does not depend on the last bit)
        let p = kstwo(&[0.000075, 0.00006, 0.000035, 0.0], &[0.000145, 0.00008, 0.000035, 0.0], 0);
        assert_eq!(format!("{p:.6}"), "0.222222");
    }
}
