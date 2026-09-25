//! The explorer's numbers by brute force: every span is looked at, every
//! count is an integer, nothing is indexed.

use std::collections::{BTreeMap, BTreeSet};

use crate::model::OracleSpan;

/// Bucket widths the explorer may use, seconds.
const BUCKET_WIDTHS_S: [u32; 25] = [
    1, 2, 5, 10, 15, 30, 60, 120, 180, 300, 600, 900, 1800, 3600, 7200, 21600, 28800, 43200, 86400,
    172800, 259200, 432000, 604800, 1209600, 2592000,
];

/// The widest bucket that still gives at least this many buckets.
const TARGET_BUCKETS: u32 = 60;

/// A time window cut into equal buckets: `[after_s, before_s)`, both multiples
/// of `width_s`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub after_s: u32,
    pub before_s: u32,
    pub width_s: u32,
}

impl Grid {
    /// The grid for a requested window: the widest listed width giving at
    /// least 60 buckets (else 1 s), `after` floored and `before` ceiled to it.
    pub fn for_window(after_s: u32, before_s: u32) -> Self {
        let span = before_s.saturating_sub(after_s);
        let mut width_s = 1;
        for w in BUCKET_WIDTHS_S {
            if span / w >= TARGET_BUCKETS {
                width_s = w;
            }
        }
        let after = after_s - after_s % width_s;
        let before = match before_s % width_s {
            0 => before_s,
            rest => before_s.saturating_add(width_s - rest),
        };
        Grid {
            after_s: after,
            before_s: before,
            width_s,
        }
    }

    pub fn buckets(&self) -> usize {
        ((self.before_s - self.after_s) / self.width_s) as usize
    }

    /// The bucket a row timestamp falls in, if inside the window.
    pub fn bucket_of(&self, start_ns: i64) -> Option<usize> {
        let after_ns = i64::from(self.after_s) * 1_000_000_000;
        let before_ns = i64::from(self.before_s) * 1_000_000_000;
        if start_ns < after_ns || start_ns >= before_ns {
            return None;
        }
        Some(((start_ns - after_ns) / (i64::from(self.width_s) * 1_000_000_000)) as usize)
    }
}

/// Field chips: a row matches when, for every field, it has at least one of
/// the listed values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub terms: BTreeMap<String, BTreeSet<String>>,
}

impl Scope {
    /// The explorer's default: spans where a request enters a service.
    pub fn entry_spans() -> Self {
        Scope::default().with("_role", &["root", "inbound"])
    }

    pub fn with(mut self, field: &str, values: &[&str]) -> Self {
        let set = self.terms.entry(field.to_string()).or_default();
        for v in values {
            set.insert(v.to_string());
        }
        self
    }

    pub fn matches(&self, span: &OracleSpan) -> bool {
        for (field, wanted) in &self.terms {
            let Some(values) = span.fields.get(field) else {
                return false;
            };
            if values.is_disjoint(wanted) {
                return false;
            }
        }
        true
    }
}

/// One bucket of a stacked histogram: rows per value of the stack field, and
/// rows without that field.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bucket {
    pub counts: BTreeMap<String, u64>,
    pub unset: u64,
}

/// Rows in scope per bucket, stacked by `stack_field`; a row with several
/// values counts once under each.
pub fn histogram(
    spans: &[OracleSpan],
    grid: &Grid,
    scope: &Scope,
    stack_field: &str,
) -> Vec<Bucket> {
    let mut buckets = vec![Bucket::default(); grid.buckets()];
    for span in spans {
        if !scope.matches(span) {
            continue;
        }
        let Some(index) = grid.bucket_of(span.start_ns) else {
            continue;
        };
        let bucket = &mut buckets[index];
        match span.fields.get(stack_field) {
            Some(values) => {
                for value in values {
                    *bucket.counts.entry(value.clone()).or_default() += 1;
                }
            }
            None => bucket.unset += 1,
        }
    }
    buckets
}

/// Rows in scope inside the window, and how many of them are errors.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub spans: u64,
    pub errors: u64,
}

pub fn totals(spans: &[OracleSpan], grid: &Grid, scope: &Scope) -> Totals {
    let mut out = Totals::default();
    for span in spans {
        if scope.matches(span) && grid.bucket_of(span.start_ns).is_some() {
            out.spans += 1;
            if span.is_error() {
                out.errors += 1;
            }
        }
    }
    out
}

/// The explorer's duration percentiles, from its documented fixed histogram:
/// zero and negative durations in bucket 0, one bucket per nanosecond below
/// 128, then 64 equal sub-buckets per power of two; a bucket reports its
/// midpoint (exact when one nanosecond wide); percentiles by nearest rank
/// (rank = ceil(q·n/100)).
pub mod fixed_histogram {
    use std::collections::BTreeMap;

    /// Largest relative error of a reported value, by definition.
    pub const MAX_RELATIVE_ERROR: f64 = 1.0 / 128.0;

    pub fn bucket(duration_ns: i64) -> u16 {
        if duration_ns <= 0 {
            return 0;
        }
        if duration_ns < 128 {
            return duration_ns as u16;
        }
        let mut exponent = 0u32;
        let mut rest = duration_ns;
        while rest > 1 {
            rest >>= 1;
            exponent += 1;
        }
        let top_seven_bits = (duration_ns >> (exponent - 6)) as u16;
        (exponent as u16 - 5) * 64 + (top_seven_bits - 64)
    }

    pub fn value(bucket: u16) -> i64 {
        if bucket < 128 {
            return i64::from(bucket);
        }
        let exponent = u32::from(bucket / 64) + 5;
        let width = 1u128 << (exponent - 6);
        let low = (64 + u128::from(bucket % 64)) * width;
        let mid = if width == 1 { low } else { low + width / 2 };
        i64::try_from(mid).unwrap_or(i64::MAX)
    }

    /// p50, p95 and p99 of `durations`, or `None` when there are none.
    pub fn percentiles(durations: &[i64]) -> Option<[i64; 3]> {
        if durations.is_empty() {
            return None;
        }
        let mut counts: BTreeMap<u16, u64> = BTreeMap::new();
        for d in durations {
            *counts.entry(bucket(*d)).or_default() += 1;
        }
        let n = durations.len() as u64;
        let mut out = [0i64; 3];
        for (slot, q) in [50u64, 95, 99].into_iter().enumerate() {
            let rank = (q * n).div_ceil(100).max(1);
            let mut seen = 0;
            for (bucket, count) in &counts {
                seen += count;
                if seen >= rank {
                    out[slot] = value(*bucket);
                    break;
                }
            }
        }
        Some(out)
    }

    /// The exact nearest-rank p50, p95 and p99, for checking the bound.
    pub fn exact_percentiles(durations: &[i64]) -> Option<[i64; 3]> {
        if durations.is_empty() {
            return None;
        }
        let mut sorted: Vec<i64> = durations.iter().map(|d| (*d).max(0)).collect();
        sorted.sort_unstable();
        let n = sorted.len() as u64;
        let mut out = [0i64; 3];
        for (slot, q) in [50u64, 95, 99].into_iter().enumerate() {
            let rank = (q * n).div_ceil(100).max(1);
            out[slot] = sorted[(rank - 1) as usize];
        }
        Some(out)
    }
}

/// Rows in the window per value of `field`, counted under the scope without
/// its own terms on `field`; a row with several values counts once under each.
pub fn facet_counts(
    spans: &[OracleSpan],
    grid: &Grid,
    scope: &Scope,
    field: &str,
) -> BTreeMap<String, u64> {
    let mut others = scope.clone();
    others.terms.remove(field);
    let mut counts = BTreeMap::new();
    for span in spans {
        if !others.matches(span) || grid.bucket_of(span.start_ns).is_none() {
            continue;
        }
        if let Some(values) = span.fields.get(field) {
            for value in values {
                *counts.entry(value.clone()).or_default() += 1;
            }
        }
    }
    counts
}

/// Scope-row durations per bucket.
pub fn bucket_durations(spans: &[OracleSpan], grid: &Grid, scope: &Scope) -> Vec<Vec<i64>> {
    let mut buckets = vec![Vec::new(); grid.buckets()];
    for span in spans {
        if !scope.matches(span) {
            continue;
        }
        if let Some(index) = grid.bucket_of(span.start_ns) {
            buckets[index].push(span.duration_ns);
        }
    }
    buckets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DURATION_BAND_FIELD, ROLE_FIELD, STATUS_FIELD};

    fn row(start_s: i64, fields: &[(&str, &str)]) -> OracleSpan {
        let mut map: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (k, v) in fields {
            map.entry(k.to_string()).or_default().insert(v.to_string());
        }
        OracleSpan {
            trace_id: None,
            span_id: None,
            parent_span_id: None,
            start_ns: start_s * 1_000_000_000,
            duration_ns: 0,
            fields: map,
            unit: 0,
        }
    }

    #[test]
    fn grid_picks_the_widest_width_with_sixty_buckets_and_aligns_outward() {
        let cases: [(u32, u32, Grid); 4] = [
            (
                1_000_000,
                1_000_900,
                Grid {
                    after_s: 1_000_000 - 1_000_000 % 15,
                    before_s: 1_000_905,
                    width_s: 15,
                },
            ),
            (
                7_200,
                10_800,
                Grid {
                    after_s: 7_200,
                    before_s: 10_800,
                    width_s: 60,
                },
            ),
            (
                0,
                86_400,
                Grid {
                    after_s: 0,
                    before_s: 86_400,
                    width_s: 900,
                },
            ),
            (
                100,
                130,
                Grid {
                    after_s: 100,
                    before_s: 130,
                    width_s: 1,
                },
            ),
        ];
        for (after, before, want) in cases {
            assert_eq!(Grid::for_window(after, before), want, "{after}..{before}");
        }
    }

    #[test]
    fn histogram_counts_scope_rows_per_bucket_and_value() {
        let grid = Grid {
            after_s: 100,
            before_s: 130,
            width_s: 10,
        };
        let spans = [
            row(100, &[(ROLE_FIELD, "root"), (STATUS_FIELD, "ERROR")]),
            row(105, &[(ROLE_FIELD, "inbound")]),
            row(109, &[(ROLE_FIELD, "outbound"), (STATUS_FIELD, "OK")]),
            row(120, &[(ROLE_FIELD, "inbound"), (STATUS_FIELD, "OK")]),
            row(130, &[(ROLE_FIELD, "root"), (STATUS_FIELD, "ERROR")]),
            row(99, &[(ROLE_FIELD, "root")]),
        ];
        let got = histogram(&spans, &grid, &Scope::entry_spans(), STATUS_FIELD);
        let bucket = |pairs: &[(&str, u64)], unset| Bucket {
            counts: pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
            unset,
        };
        assert_eq!(
            got,
            vec![
                bucket(&[("ERROR", 1)], 1),
                bucket(&[], 0),
                bucket(&[("OK", 1)], 0)
            ]
        );
        assert_eq!(
            totals(&spans, &grid, &Scope::entry_spans()),
            Totals {
                spans: 3,
                errors: 1
            }
        );
        let by_band = histogram(&spans, &grid, &Scope::default(), DURATION_BAND_FIELD);
        assert_eq!(by_band.iter().map(|b| b.unset).sum::<u64>(), 4);
    }

    #[test]
    fn fixed_histogram_follows_its_definition() {
        use fixed_histogram::{bucket, exact_percentiles, percentiles, value};
        let cases: [(i64, u16, i64); 7] = [
            (0, 0, 0),
            (127, 127, 127),
            (128, 128, 129),
            (256, 192, 258),
            (1_000_000, 954, 1_003_520),
            (10_000_000_000, 1_802, 9_999_220_736),
            (i64::MAX, 3_711, 9_187_343_239_835_811_840),
        ];
        for (duration, index, reported) in cases {
            assert_eq!(bucket(duration), index, "{duration}");
            assert_eq!(value(index), reported, "{duration}");
        }
        let durations: Vec<i64> = (1..=100).collect();
        assert_eq!(percentiles(&durations), Some([50, 95, 99]));
        assert_eq!(exact_percentiles(&durations), Some([50, 95, 99]));
        assert_eq!(percentiles(&[]), None);
    }
}
