//! The facet comparison under a selection: per value, its share of the
//! selection's rows against its share of the rest of the scope (the
//! baseline), ranked exactly (QRY-15).
//!
//! For a field, `S` scope rows and `C` selection rows (both without the
//! field's own chips) leave `B = S − C` baseline rows; a value with `s` scope
//! and `c` selection rows has `b = s − c` baseline rows. Its difference is
//! `c/C − b/B`, or `c/C` when the baseline is empty. Only values with at
//! least [`MIN_SUPPORT`] selection rows are eligible for a rank. A field the
//! selection is made of is not compared
//! ([`ExploreSelection::made_of`](super::query::ExploreSelection::made_of)).

use std::cmp::Ordering;

/// Selection rows a value needs before its difference is ranked.
pub const MIN_SUPPORT: u64 = 5;

/// A share difference `num / den` (`den > 0`), ordered exactly.
#[derive(Debug, Clone, Copy)]
pub struct ShareDiff {
    pub num: i128,
    pub den: i128,
}

impl ShareDiff {
    pub fn to_f64(self) -> f64 {
        self.num as f64 / self.den as f64
    }
}

impl Ord for ShareDiff {
    fn cmp(&self, other: &Self) -> Ordering {
        match (
            self.num.checked_mul(other.den),
            other.num.checked_mul(self.den),
        ) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => self.to_f64().total_cmp(&other.to_f64()),
        }
    }
}

impl PartialOrd for ShareDiff {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for ShareDiff {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for ShareDiff {}

/// The rows the comparison is out of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ComparisonTotals {
    pub scope: u64,
    pub selection: u64,
}

/// A field under a selection: its totals, and its rank among the fields by
/// best eligible difference (`None` without an eligible value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldComparison {
    pub totals: ComparisonTotals,
    pub rank: Option<u32>,
    pub best: Option<ShareDiff>,
}

/// A value under a selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueComparison {
    pub selection: u64,
    pub baseline: u64,
    pub eligible: bool,
    /// Among the field's eligible values, by difference.
    pub rank: Option<u32>,
    /// `None` when the selection holds no row of the field.
    pub diff: Option<ShareDiff>,
}

/// One value's rows: `(value, scope, selection)`; `None` is the unset value.
pub(super) type ValueRows<'a> = (Option<&'a str>, u64, u64);

/// Values in byte order, the unset value after every value.
pub(super) fn value_order(a: Option<&str>, b: Option<&str>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Compares a field's values, index-parallel to `values`, and returns its
/// best eligible difference.
pub(super) fn compare_values(
    totals: ComparisonTotals,
    values: &[ValueRows<'_>],
) -> (Vec<ValueComparison>, Option<ShareDiff>) {
    let scope = i128::from(totals.scope);
    let selection = i128::from(totals.selection);
    let base = scope.saturating_sub(selection).max(0);
    let mut out = Vec::with_capacity(values.len());
    for &(_, s, c) in values {
        let b = s.saturating_sub(c);
        let diff = (selection > 0).then(|| {
            let (c, b) = (i128::from(c), i128::from(b));
            if base == 0 {
                ShareDiff {
                    num: c,
                    den: selection,
                }
            } else {
                ShareDiff {
                    num: c * base - b * selection,
                    den: selection * base,
                }
            }
        });
        out.push(ValueComparison {
            selection: c,
            baseline: b,
            eligible: selection > 0 && c >= MIN_SUPPORT,
            rank: None,
            diff,
        });
    }

    let mut eligible: Vec<usize> = (0..out.len()).filter(|&i| out[i].eligible).collect();
    eligible.sort_by(|&a, &b| {
        out[b]
            .diff
            .cmp(&out[a].diff)
            .then_with(|| value_order(values[a].0, values[b].0))
    });
    let best = eligible.first().and_then(|&i| out[i].diff);
    for (rank, index) in eligible.into_iter().enumerate() {
        out[index].rank = Some(rank as u32 + 1);
    }
    (out, best)
}

/// The fields' ranks by best eligible difference, then by name; fields
/// without one are not ranked.
pub(super) fn rank_fields(fields: &[(&str, Option<ShareDiff>)]) -> Vec<Option<u32>> {
    let mut ranked: Vec<usize> = (0..fields.len())
        .filter(|&i| fields[i].1.is_some())
        .collect();
    ranked.sort_by(|&a, &b| {
        fields[b]
            .1
            .cmp(&fields[a].1)
            .then_with(|| fields[a].0.cmp(fields[b].0))
    });
    let mut out = vec![None; fields.len()];
    for (rank, index) in ranked.into_iter().enumerate() {
        out[index] = Some(rank as u32 + 1);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totals(scope: u64, selection: u64) -> ComparisonTotals {
        ComparisonTotals { scope, selection }
    }

    #[test]
    fn comparison_ranks_by_exact_share_difference() {
        // Scope 100, selection 20, baseline 80.
        let values = [
            (Some("a"), 30, 15),
            (Some("b"), 50, 4),
            (Some("c"), 20, 1),
            (Some("d"), 10, 5),
        ];
        let (got, best) = compare_values(totals(100, 20), &values);
        let fraction = |num, den| Some(ShareDiff { num, den });
        type Summary = (u64, u64, bool, Option<u32>, Option<ShareDiff>);
        let summary: Vec<Summary> = got
            .iter()
            .map(|v| (v.selection, v.baseline, v.eligible, v.rank, v.diff))
            .collect();
        assert_eq!(
            summary,
            [
                (15, 15, true, Some(1), fraction(9, 16)),
                (4, 46, false, None, fraction(-3, 8)),
                (1, 19, false, None, fraction(-3, 16)),
                (5, 5, true, Some(2), fraction(3, 16)),
            ]
        );
        assert_eq!(best, got[0].diff);

        let (tied, _) = compare_values(
            totals(30, 15),
            &[(None, 10, 5), (Some("y"), 10, 5), (Some("x"), 10, 5)],
        );
        assert_eq!(
            [tied[0].rank, tied[1].rank, tied[2].rank],
            [Some(3), Some(2), Some(1)],
            "ties by value, the unset value last"
        );

        let (whole, best) = compare_values(totals(10, 10), &[(Some("a"), 6, 6), (Some("b"), 4, 4)]);
        assert_eq!(
            whole[0].diff.unwrap().to_f64(),
            0.6,
            "an empty baseline gives c/C"
        );
        assert_eq!(best.unwrap().to_f64(), 0.6);

        let (none, best) = compare_values(totals(10, 0), &[(Some("a"), 10, 0)]);
        assert!(none[0].diff.is_none() && !none[0].eligible && best.is_none());
    }

    #[test]
    fn field_ranking_is_exact_where_floats_tie() {
        let close = ShareDiff {
            num: 1_000_000_000_000_000_001,
            den: 3_000_000_000_000_000_000,
        };
        let third = ShareDiff { num: 1, den: 3 };
        assert_eq!(close.to_f64(), third.to_f64());
        assert!(close > third);
        assert_eq!(ShareDiff { num: 2, den: 6 }, third);

        let ranks = rank_fields(&[
            ("b", Some(third)),
            ("a", None),
            ("c", Some(close)),
            ("a2", Some(third)),
        ]);
        assert_eq!(ranks, [Some(3), None, Some(1), Some(2)]);
    }
}
