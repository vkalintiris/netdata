//! The query window (`query_target_calculate_window()`, `src/web/api/queries/query-window.c` in C) and the row
//! timestamps. Spec §4.1-4.2. Arithmetic follows C's `time_t` truncation toward zero.

use netdata_agent_text::json::API_RELATIVE_TIME_MAX;
use netdata_agent_text::time_window::relative_window_to_absolute_query;

use crate::tables::{TimeGrouping, options};
use crate::target::QueryTarget;

/// `qt->window` after the calculation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub after: i64,
    pub before: i64,
    pub relative: bool,
    pub points: u64,
    pub group: i64,
    pub query_granularity: i64,
    pub resampling_group: i64,
    pub resampling_divisor: f64,
    pub options: u64,
    pub aligned: bool,
}

impl Window {
    /// `view.update_every`: one row's duration.
    pub fn view_update_every(&self) -> i64 {
        self.group * self.query_granularity
    }

    /// The timestamp of row `i`: the end of its group; the last row ends at `before`.
    pub fn row_time(&self, i: u64) -> i64 {
        let vue = self.view_update_every();
        self.after + (vue - self.query_granularity) + i as i64 * vue
    }
}

fn is_rel(v: i64) -> bool {
    (-API_RELATIVE_TIME_MAX..=API_RELATIVE_TIME_MAX).contains(&v)
}

/// `query_target_calculate_window()`: `None` where C fails (overflow or a negative duration). `wall_s` is the
/// wall clock of the second relative conversion.
pub fn calculate(qt: &QueryTarget, wall_s: i64) -> Option<Window> {
    let req = &qt.request;
    let (pr, ar, br) = (req.points, req.after, req.before);
    let rs = req.resampling_time;
    let mut opts = qt.window.options;
    let ue = if qt.db.minimum_latest_update_every_s != 0 {
        qt.db.minimum_latest_update_every_s
    } else {
        1
    };
    let (mut pw, mut aw, mut bw) = (pr as i64, ar, br);
    let aligned = opts & options::NOT_ALIGNED == 0;
    let auto_nat = pw == 0;
    let mut natural = opts & options::NATURAL_POINTS != 0 || auto_nat;
    let mut relative = false;
    let mut before_db_end = false;
    if is_rel(bw) || is_rel(aw) {
        relative = true;
        natural = true;
        opts |= options::NATURAL_POINTS;
    }
    if opts & options::VIRTUAL_POINTS != 0 {
        natural = false;
    }
    if natural {
        opts = (opts | options::NATURAL_POINTS) & !options::VIRTUAL_POINTS;
    } else {
        opts = (opts | options::VIRTUAL_POINTS) & !options::NATURAL_POINTS;
    }
    if aw == 0 || bw == 0 {
        relative = true;
        let (f, l) = (qt.db.first_time_s, qt.db.last_time_s);
        if f == 0 || l == 0 {
            aw = qt.window.after;
            bw = qt.window.before;
            if aw == bw {
                aw = bw - ue;
            }
            if pw == 0 {
                pw = (bw - aw) / ue;
            }
        } else {
            if aw == 0 {
                aw = f;
            }
            if bw == 0 {
                bw = l;
                before_db_end = true;
            }
            if pw == 0 {
                pw = (l - f) / ue;
            }
        }
    }
    if pw == 0 {
        pw = 600;
    }
    let (a, b, _) = relative_window_to_absolute_query(aw, bw, wall_s);
    (aw, bw) = (a, b);
    let mut qg = if natural { ue } else { 1 };
    if qg <= 0 {
        qg = 1;
    }
    bw -= bw % qg;
    aw -= aw % qg;
    if auto_nat {
        pw = (bw - aw + 1) / qg;
        if pw <= 0 {
            pw = 1;
        }
    }
    let mut duration = bw.checked_sub(aw)?;
    if duration < 0 {
        return None;
    }
    // C also moves `after` here; the final `after` is recomputed from `before` below, so only the duration counts.
    if rs > duration {
        duration = rs;
    }
    if rs > qg && duration % rs != 0 {
        let delta = duration % rs;
        if delta > rs / 10 {
            duration += rs - delta;
        }
    }
    let mut pa = duration / qg + i64::from(duration % qg == qg - 1);
    if pa == 0 {
        pa = 1;
    }
    // size_t in C: a negative requested count is huge, so it is clamped here.
    let mut points = pw as u64;
    if points > pa as u64 {
        points = pa as u64;
    }
    if points > 86400 {
        points = 86400;
    }
    let pw = points as i64;
    let mut group = pa / pw;
    if group == 0 {
        group = 1;
    }
    if pa % pw > pw / 2 {
        group += 1;
    }
    let required = duration / qg + i64::from(duration % qg != 0);
    let mut pw = pw;
    if pw * group < required {
        pw = pa / group;
        if pw * group < pa {
            pw += 1;
        }
        if pw == 0 {
            pw = 1;
        }
    }
    let mut divisor = 1.0;
    let mut rgroup = 1;
    if rs > qg {
        rgroup = rs / qg + i64::from(rs % qg != 0);
        group = group.max(rgroup);
        if group % rgroup != 0 {
            group += rgroup - group % rgroup;
        }
    }
    let vue = group * qg;
    if rs > qg {
        divisor = vue as f64 / rs as f64;
    }
    let latest_end =
        req.time_group == TimeGrouping::Latest && pr == 1 && br == 0 && qt.db.last_time_s > 0;
    if latest_end {
        bw = qt.db.last_time_s;
    }
    if aligned && !latest_end && bw % vue != 0 {
        if before_db_end {
            bw -= bw % vue;
        } else {
            bw += vue - bw % vue;
        }
    }
    aw = bw - ((pw - 1) * vue + (vue - qg));
    Some(Window {
        after: aw,
        before: bw,
        relative,
        points: pw as u64,
        group,
        query_granularity: qg,
        resampling_group: rgroup,
        resampling_divisor: divisor,
        options: opts,
        aligned,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::request::parse_v1;
    use crate::target::{Db, QueryTarget, SelectionWindow};

    const T: i64 = 1_700_000_000;

    fn qt(query: &str, db: Db) -> QueryTarget {
        let request = parse_v1(query.as_bytes(), 1).request;
        let (after, before, absolute) =
            relative_window_to_absolute_query(request.after, request.before, T + 1);
        QueryTarget {
            window: SelectionWindow {
                after,
                before,
                relative: !absolute,
                options: request.options,
            },
            request,
            start_s: T + 1,
            nodes: Vec::new(),
            contexts: Vec::new(),
            instances: Vec::new(),
            dimensions: Vec::new(),
            query: Vec::new(),
            db,
            id: String::new(),
            chart_scoped: false,
            chart_label_key: None,
            preprocessed: std::time::Instant::now(),
            executed: None,
        }
    }

    fn db() -> Db {
        Db {
            first_time_s: T - 3599,
            last_time_s: T,
            minimum_latest_update_every_s: 1,
            ..Db::default()
        }
    }

    #[test]
    fn natural_absolute_window() {
        let w = calculate(&qt(&format!("after={}&before={T}", T - 60), db()), T + 1).unwrap();
        assert_eq!((w.after, w.before, w.points, w.group), (T - 60, T, 61, 1));
        assert_eq!(w.row_time(w.points - 1), T);
        assert!(!w.relative);
    }

    #[test]
    fn virtual_points_group_and_align_up() {
        let w = calculate(
            &qt(&format!("after={}&before={T}&points=10", T - 60), db()),
            T + 1,
        )
        .unwrap();
        // pa = 61 rows at 1s, 10 wanted: groups of 6, before aligned up to a multiple of 6.
        assert_eq!((w.points, w.group, w.view_update_every()), (10, 6, 6));
        assert_eq!(w.before % 6, 0);
        assert_eq!(w.before, T + 4);
        assert_eq!(w.after, w.before - (9 * 6 + 5));
    }

    #[test]
    fn before_zero_anchors_on_the_newest_sample() {
        let w = calculate(&qt("after=-60&points=6", db()), T + 1).unwrap();
        assert!(w.relative);
        assert_eq!(w.before % w.view_update_every(), 0);
        assert!(w.before <= T, "aligned down to the data end, not up");
        assert_eq!(w.points, 6);
    }

    #[test]
    fn latest_ends_at_the_last_sample() {
        let w = calculate(&qt("group=latest&points=1&after=-10", db()), T + 1).unwrap();
        assert_eq!(w.before, T);
    }
}
