//! What runs on a result after every metric was executed and before it is rendered: percentage of total
//! (`rrd2rrdr_convert_values_to_percentage_of_total()`, `src/web/api/queries/query-group-by-finalize.c`) and the
//! cardinality limit (`rrd2rrdr_cardinality_limit()`, `query-cardinality-limit.c`). Spec §8.1, §8.5, §8.6. The v2
//! group-by arrays (per-dimension statistics, counts, hidden values) join these with the v2 passes.

use crate::rrdr::{Rrdr, value_flags};
use crate::tables::options;
use crate::target::metric_status;

/// `rrd2rrdr_convert_values_to_percentage_of_total()` for a v1 result: every queried, non-empty cell becomes its
/// share of the row total (hidden columns included; a zero total counts as 1), and the view's min/max follow.
pub fn percentage_of_total(r: &mut Rrdr, window_options: u64) {
    if window_options & options::PERCENTAGE == 0 || window_options & options::RETURN_RAW != 0 {
        return;
    }
    let (mut seen, mut min, mut max) = (false, f64::NAN, f64::NAN);
    let queried: Vec<usize> = (0..r.columns)
        .filter(|&d| r.od[d] & metric_status::QUERIED != 0)
        .collect();
    for i in 0..r.rows {
        let base = i * r.columns;
        let cells = || {
            queried
                .iter()
                .map(|d| base + d)
                .filter(|&idx| r.o[idx] & value_flags::EMPTY == 0)
        };
        let mut total: f64 = cells().map(|idx| r.v[idx]).sum();
        if total == 0.0 {
            total = 1.0;
        }
        let indexes: Vec<usize> = cells().collect();
        for idx in indexes {
            let n = r.v[idx] * 100.0 / total;
            r.v[idx] = n;
            if !seen {
                seen = true;
                min = n;
                max = n;
            } else {
                if n < min {
                    min = n;
                }
                if n > max {
                    max = n;
                }
            }
        }
    }
    r.view.min = min;
    r.view.max = max;
}

/// `strcmp()` order of the ids, for equal contributions.
fn by_contribution(a: &(usize, f64, &str), b: &(usize, f64, &str)) -> std::cmp::Ordering {
    b.1.partial_cmp(&a.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.2.as_bytes().cmp(b.2.as_bytes()))
}

/// `rrd2rrdr_cardinality_limit()`: with more queried columns than `limit`, keeps the `limit - 1` that contribute
/// most (the sum of their absolute values) and folds the rest into `remaining N dimensions`.
pub fn cardinality_limit(r: Rrdr, limit: u64) -> Rrdr {
    let limit = usize::try_from(limit).unwrap_or(usize::MAX);
    if limit == 0 || r.columns <= limit {
        return r;
    }
    let mut ranked: Vec<(usize, f64, &str)> = Vec::new();
    for d in 0..r.columns {
        if r.od[d] & metric_status::QUERIED == 0 {
            continue;
        }
        let contribution: f64 = (0..r.rows)
            .map(|i| r.index(i, d))
            .filter(|&idx| r.o[idx] & value_flags::EMPTY == 0 && !r.v[idx].is_nan())
            .map(|idx| r.v[idx].abs())
            .sum();
        ranked.push((d, contribution, r.di[d].as_str()));
    }
    if ranked.len() <= limit {
        return r;
    }
    // A total order (C's qsort ranks the same way; its ties fall to the id).
    ranked.sort_by(by_contribution);
    let kept = limit - 1;
    let folded = ranked.len() - kept;
    let cells = r.rows * limit;
    let mut new_r = Rrdr {
        rows: r.rows,
        columns: limit,
        t: r.t.clone(),
        v: vec![f64::NAN; cells],
        o: vec![value_flags::EMPTY; cells],
        ar: vec![0.0; cells],
        od: vec![0; limit],
        di: vec![String::new(); limit],
        dn: vec![String::new(); limit],
        view: r.view,
        queries_count: 0,
        result_points_generated: r.result_points_generated,
        db_points_read: r.db_points_read,
        cardinality_folded: folded,
        cardinality_cut: ranked[kept].1,
    };
    for (i, &(src, _, _)) in ranked[..kept].iter().enumerate() {
        new_r.di[i] = r.di[src].clone();
        new_r.dn[i] = r.dn[src].clone();
        new_r.od[i] = r.od[src];
        for row in 0..r.rows {
            let (from, to) = (r.index(row, src), new_r.index(row, i));
            new_r.v[to] = r.v[from];
            new_r.ar[to] = r.ar[from];
            new_r.o[to] = r.o[from];
        }
    }
    let name = format!("remaining {folded} dimensions");
    new_r.di[kept] = name.clone();
    new_r.dn[kept] = name;
    new_r.od[kept] = metric_status::QUERIED | metric_status::NONZERO;
    for row in 0..r.rows {
        let (mut value, mut ar, mut flags) = (0.0, 0.0, value_flags::NOTHING);
        let (mut has_values, mut has_empty) = (false, false);
        for &(src, _, _) in &ranked[kept..] {
            let idx = r.index(row, src);
            if r.o[idx] & value_flags::EMPTY == 0 && !r.v[idx].is_nan() {
                value += r.v[idx];
                ar += r.ar[idx];
                flags |= r.o[idx] & (value_flags::RESET | value_flags::PARTIAL);
                has_values = true;
            } else {
                has_empty = true;
            }
        }
        if has_values && has_empty {
            flags |= value_flags::PARTIAL;
        }
        if has_values {
            let to = new_r.index(row, kept);
            new_r.v[to] = value;
            new_r.ar[to] = ar;
            new_r.o[to] = flags & !value_flags::EMPTY;
        }
    }
    new_r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rrdr::View;

    fn rrdr(columns: &[(&str, &[f64])]) -> Rrdr {
        let rows = columns[0].1.len();
        let mut v = Vec::new();
        let mut o = Vec::new();
        for i in 0..rows {
            for (_, values) in columns {
                v.push(if values[i].is_nan() { 0.0 } else { values[i] });
                o.push(if values[i].is_nan() {
                    value_flags::EMPTY
                } else {
                    0
                });
            }
        }
        Rrdr {
            rows,
            columns: columns.len(),
            t: (0..rows as i64).collect(),
            ar: vec![0.0; v.len()],
            v,
            o,
            od: vec![metric_status::QUERIED; columns.len()],
            di: columns.iter().map(|(id, _)| id.to_string()).collect(),
            dn: columns.iter().map(|(id, _)| id.to_uppercase()).collect(),
            view: View {
                after: 0,
                before: 0,
                group: 1,
                update_every: 1,
                min: 0.0,
                max: 0.0,
                flags: 0,
            },
            queries_count: columns.len(),
            result_points_generated: 0,
            db_points_read: 0,
            cardinality_folded: 0,
            cardinality_cut: 0.0,
        }
    }

    #[test]
    fn percentage_of_total_counts_hidden_columns_and_skips_empty_cells() {
        let e = f64::NAN;
        let mut r = rrdr(&[("a", &[1.0, 0.0, e]), ("b", &[3.0, 0.0, -2.0])]);
        r.od[1] |= metric_status::HIDDEN;
        percentage_of_total(&mut r, options::PERCENTAGE);
        assert_eq!(r.v, vec![25.0, 75.0, 0.0, 0.0, 0.0, 100.0]);
        assert_eq!((r.view.min, r.view.max), (0.0, 100.0));
        let before = r.v.clone();
        percentage_of_total(&mut r, options::PERCENTAGE | options::RETURN_RAW);
        assert_eq!(r.v, before, "raw is left alone");
    }

    #[test]
    fn the_limit_folds_the_smallest_contributors() {
        let e = f64::NAN;
        let r = rrdr(&[
            ("a", &[1.0, 1.0]),
            ("b", &[-9.0, 1.0]),
            ("c", &[2.0, e]),
            ("d", &[e, e]),
        ]);
        let r = cardinality_limit(r, 3);
        // contributions: b 10, a 2, c 2 (a wins the tie by id), d 0
        assert_eq!(r.di, ["b", "a", "remaining 2 dimensions"]);
        assert_eq!(r.od[2], metric_status::QUERIED | metric_status::NONZERO);
        // row 0: c=2 folded with an empty d is partial; row 1: both folded cells empty
        assert_eq!(r.v[..5], [-9.0, 1.0, 2.0, 1.0, 1.0]);
        assert!(r.v[5].is_nan());
        assert_eq!(r.o, [0, 0, value_flags::PARTIAL, 0, 0, value_flags::EMPTY]);
        assert_eq!((r.cardinality_folded, r.cardinality_cut), (2, 2.0));
        // ties fall to the id
        let r = cardinality_limit(rrdr(&[("y", &[1.0]), ("x", &[1.0]), ("z", &[1.0])]), 2);
        assert_eq!(r.di, ["x", "remaining 2 dimensions"]);
        assert_eq!(
            cardinality_limit(rrdr(&[("y", &[1.0]), ("x", &[1.0])]), 2).columns,
            2,
            "within the limit"
        );
    }
}
