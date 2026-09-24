//! What runs on a result after every metric was executed and before it is rendered: percentage of total
//! (`rrd2rrdr_convert_values_to_percentage_of_total()`, `src/web/api/queries/query-group-by-finalize.c`) and the
//! cardinality limit (`rrd2rrdr_cardinality_limit()`, `query-cardinality-limit.c`). Spec §8.1, §8.5, §8.6. The v2
//! group-by arrays (per-dimension statistics, counts, hidden values) join these with the v2 passes.

use netdata_agent_storage::storage_point::StoragePoint;

use crate::rrdr::{GroupLabels, Rrdr, value_flags};
use crate::tables::options;
use crate::target::metric_status;

/// `rrd2rrdr_convert_values_to_percentage_of_total()`: every queried, non-empty cell of every allocated row
/// becomes its share of the row total (hidden columns included; a zero total counts as 1) and the view's min/max
/// follow; a v2 result also recomputes each column's statistics over the shown rows.
pub fn percentage_of_total(r: &mut Rrdr, window_options: u64) {
    if window_options & options::PERCENTAGE == 0 || window_options & options::RETURN_RAW != 0 {
        return;
    }
    let (mut seen, mut min, mut max) = (false, f64::NAN, f64::NAN);
    let queried: Vec<usize> = (0..r.columns)
        .filter(|&d| r.od[d] & metric_status::QUERIED != 0)
        .collect();
    for i in 0..r.n {
        let base = i * r.columns;
        let cells: Vec<usize> = queried
            .iter()
            .map(|d| base + d)
            .filter(|&idx| r.o[idx] & value_flags::EMPTY == 0)
            .collect();
        let mut total = 0.0;
        for &idx in &cells {
            total += r.v[idx];
        }
        if total == 0.0 {
            total = 1.0;
        }
        for idx in cells {
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
    if r.dview.is_empty() {
        // v1
        return;
    }
    for &d in &queried {
        let (mut count, mut min, mut max, mut sum, mut ars) = (0u32, 0.0, 0.0, 0.0, 0.0);
        for i in 0..r.rows {
            let idx = r.index(i, d);
            if r.o[idx] & value_flags::EMPTY != 0 {
                continue;
            }
            ars += r.ar[idx];
            let n = r.v[idx];
            sum += n;
            if count == 0 {
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
            count += 1;
        }
        r.dview[d] = StoragePoint {
            sum,
            count,
            min,
            max,
            // C: through size_t into the uint32_t field.
            anomaly_count: (ars * f64::from(count)) as u64 as u32,
            ..StoragePoint::default()
        };
    }
}

/// `strcmp()` order of the ids, for equal contributions.
fn by_contribution(a: &(usize, f64, &str), b: &(usize, f64, &str)) -> std::cmp::Ordering {
    b.1.partial_cmp(&a.1)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.2.as_bytes().cmp(b.2.as_bytes()))
}

/// `RRDR_DVIEW_ANOMALY_COUNT_MULTIPLIER`.
pub const DVIEW_ANOMALY_COUNT_MULTIPLIER: f64 = 1000.0;

/// `rrd2rrdr_cardinality_limit()`: with more queried columns than `limit`, keeps the `limit - 1` that contribute
/// most (their view sum, else the sum of their absolute values) and folds the rest into `remaining N dimensions`.
/// The new result mirrors the arrays the old one has.
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
        let contribution = if !r.dview.is_empty() && !r.dview[d].sum.is_nan() {
            r.dview[d].sum.abs()
        } else {
            let mut sum = 0.0;
            for i in 0..r.rows {
                let idx = r.index(i, d);
                if r.o[idx] & value_flags::EMPTY == 0 && !r.v[idx].is_nan() {
                    sum += r.v[idx].abs();
                }
            }
            sum
        };
        ranked.push((d, contribution, r.di[d].as_str()));
    }
    if ranked.len() <= limit {
        return r;
    }
    // A total order: C's qsort ranks the same way and its ties fall to the id.
    ranked.sort_by(by_contribution);
    let kept = limit - 1;
    let folded = ranked.len() - kept;
    let cells = r.n * limit;
    let per_column = |present: bool| if present { limit } else { 0 };
    let mut new_r = Rrdr {
        n: r.n,
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
        du: vec![String::new(); per_column(!r.du.is_empty())],
        dp: vec![0; per_column(!r.dp.is_empty())],
        dview: vec![StoragePoint::default(); per_column(!r.dview.is_empty())],
        dgbc: vec![0; per_column(!r.dgbc.is_empty())],
        dqp: vec![StoragePoint::default(); per_column(!r.dqp.is_empty())],
        dgbs: Vec::new(),
        dl: r.dl.as_ref().map(|_| vec![GroupLabels::new(); limit]),
        label_keys: r.label_keys.clone(),
        gbc: if r.gbc.is_empty() {
            Vec::new()
        } else {
            vec![0; cells]
        },
        arc: Vec::new(),
        vh: if r.vh.is_empty() {
            Vec::new()
        } else {
            vec![f64::NAN; cells]
        },
        hgbc: Vec::new(),
        trimming: r.trimming,
    };
    for (i, &(src, _, _)) in ranked[..kept].iter().enumerate() {
        new_r.di[i] = r.di[src].clone();
        new_r.dn[i] = r.dn[src].clone();
        new_r.od[i] = r.od[src];
        if !new_r.du.is_empty() {
            new_r.du[i] = r.du[src].clone();
        }
        if !new_r.dp.is_empty() {
            new_r.dp[i] = r.dp[src];
        }
        if !new_r.dgbc.is_empty() {
            new_r.dgbc[i] = r.dgbc[src];
        }
        if !new_r.dqp.is_empty() {
            new_r.dqp[i] = r.dqp[src];
        }
        if let (Some(to), Some(from)) = (new_r.dl.as_mut(), r.dl.as_ref()) {
            to[i] = from[src].clone();
        }
        for row in 0..r.rows {
            let (from, to) = (r.index(row, src), new_r.index(row, i));
            new_r.v[to] = r.v[from];
            new_r.ar[to] = r.ar[from];
            new_r.o[to] = r.o[from];
            if !new_r.gbc.is_empty() {
                new_r.gbc[to] = r.gbc[from];
            }
            if !new_r.vh.is_empty() {
                new_r.vh[to] = r.vh[from];
            }
        }
        if !new_r.dview.is_empty() {
            new_r.dview[i] = r.dview[src];
        }
    }

    let name = format!("remaining {folded} dimensions");
    new_r.di[kept] = name.clone();
    new_r.dn[kept] = name;
    new_r.od[kept] = metric_status::QUERIED | metric_status::NONZERO;
    let first_folded = ranked[kept].0;
    if !new_r.du.is_empty() {
        new_r.du[kept] = r.du[first_folded].clone();
    }
    if !new_r.dp.is_empty() {
        new_r.dp[kept] = r.dp[first_folded];
    }
    if !new_r.dqp.is_empty() {
        new_r.dqp[kept] = StoragePoint::UNSET;
    }
    for &(src, _, _) in &ranked[kept..] {
        if !new_r.dgbc.is_empty() {
            new_r.dgbc[kept] += r.dgbc[src];
        }
        if !new_r.dqp.is_empty() {
            new_r.dqp[kept].merge_to(&r.dqp[src]);
        }
        if let (Some(to), Some(from)) = (new_r.dl.as_mut(), r.dl.as_ref()) {
            merge_group_labels(&mut to[kept], &from[src]);
        }
    }
    let (mut sum, mut min, mut max, mut ars, mut count) = (0.0, f64::NAN, f64::NAN, 0.0, 0u32);
    for row in 0..r.rows {
        let to = new_r.index(row, kept);
        let (mut value, mut ar, mut hidden, mut gbc) = (0.0, 0.0, f64::NAN, 0u32);
        let mut flags = value_flags::NOTHING;
        let (mut has_values, mut has_empty) = (false, false);
        for &(src, _, _) in &ranked[kept..] {
            let idx = r.index(row, src);
            if !r.vh.is_empty() && !r.vh[idx].is_nan() {
                hidden = if hidden.is_nan() {
                    r.vh[idx]
                } else {
                    hidden + r.vh[idx]
                };
            }
            if r.o[idx] & value_flags::EMPTY == 0 && !r.v[idx].is_nan() {
                value += r.v[idx];
                ar += r.ar[idx];
                flags |= r.o[idx] & (value_flags::RESET | value_flags::PARTIAL);
                if !r.gbc.is_empty() {
                    gbc += r.gbc[idx];
                }
                has_values = true;
            } else {
                has_empty = true;
            }
        }
        if has_values && has_empty {
            flags |= value_flags::PARTIAL;
        }
        if !new_r.vh.is_empty() {
            new_r.vh[to] = hidden;
        }
        if has_values {
            new_r.v[to] = value;
            new_r.ar[to] = ar;
            new_r.o[to] = flags & !value_flags::EMPTY;
            if !new_r.gbc.is_empty() {
                new_r.gbc[to] = gbc;
            }
            sum += value;
            ars += ar;
            if count == 0 {
                min = value;
                max = value;
            } else {
                if value < min {
                    min = value;
                }
                if value > max {
                    max = value;
                }
            }
            count += 1;
        }
    }
    if !new_r.dview.is_empty() {
        new_r.dview[kept] = StoragePoint {
            sum,
            count,
            min,
            max,
            anomaly_count: (ars * DVIEW_ANOMALY_COUNT_MULTIPLIER / 100.0) as u64 as u32,
            ..StoragePoint::default()
        };
    }
    new_r
}

/// `group_by_labels_merge()`: the folded group's keys and values join the remaining group's.
pub fn merge_group_labels(dst: &mut GroupLabels, src: &GroupLabels) {
    for (key, values) in src {
        let at = match dst.iter().position(|(k, _)| k == key) {
            Some(at) => at,
            None => {
                dst.push((key.clone(), Vec::new()));
                dst.len() - 1
            }
        };
        for value in values {
            if !dst[at].1.contains(value) {
                dst[at].1.push(value.clone());
            }
        }
    }
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
            n: rows,
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
            ..Rrdr::new(&crate::window::Window::default(), 0)
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
