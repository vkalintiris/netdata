//! The time-grouping methods (`src/web/api/queries/<method>/`): create once per query, reset per metric, add each
//! finite value, flush once per row (which may only mark the row EMPTY). Spec §6.2-6.3.

use netdata_agent_text::parse::str2ndd;

use crate::tables::TimeGrouping;

/// What a row's flush may report (`RRDR_VALUE_EMPTY`).
pub const VALUE_EMPTY: u32 = 1 << 0;

/// `[web] ses max tg_des_window` and `des max tg_des_window`.
pub const DEFAULT_MAX_WINDOW: i64 = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CountifOp {
    Eq,
    Ne,
    Gt,
    Ge,
    Lt,
    Le,
}

#[derive(Debug, Clone)]
enum State {
    Average {
        sum: f64,
        count: u64,
    },
    Sum {
        sum: f64,
        count: u64,
    },
    Min {
        min: f64,
        count: u64,
    },
    Max {
        max: f64,
        count: u64,
    },
    Extremes {
        min: f64,
        max: f64,
        pos: u64,
        neg: u64,
        zeros: u64,
    },
    Latest {
        latest: f64,
        count: u64,
    },
    Countif {
        op: CountifOp,
        target: f64,
        matched: u64,
        count: u64,
    },
    IncrementalSum {
        first: f64,
        last: f64,
        count: u64,
    },
    Stddev {
        cv: bool,
        count: i64,
        m: f64,
        s: f64,
    },
    Ses {
        alpha: f64,
        level: f64,
        count: u64,
    },
    Des {
        alpha: f64,
        beta: f64,
        level: f64,
        trend: f64,
        count: u64,
    },
    Median {
        percent: f64,
        series: Vec<f64>,
    },
    Percentile {
        percent: f64,
        series: Vec<f64>,
    },
    TrimmedMean {
        percent: f64,
        series: Vec<f64>,
    },
}

/// One metric's grouping (`r->time_grouping`).
#[derive(Debug, Clone)]
pub struct Grouping {
    state: State,
    resampling_group: i64,
    resampling_divisor: f64,
}

/// The percentage a trimmed or percentile name carries, else the method's default.
fn named_percent(method: TimeGrouping) -> Option<f64> {
    use TimeGrouping as G;
    Some(match method {
        G::TrimmedMean1 | G::TrimmedMedian1 => 1.0,
        G::TrimmedMean2 | G::TrimmedMedian2 => 2.0,
        G::TrimmedMean3 | G::TrimmedMedian3 => 3.0,
        G::TrimmedMean | G::TrimmedMedian => 5.0,
        G::TrimmedMean10 | G::TrimmedMedian10 => 10.0,
        G::TrimmedMean15 | G::TrimmedMedian15 => 15.0,
        G::TrimmedMean20 | G::TrimmedMedian20 => 20.0,
        G::TrimmedMean25 | G::TrimmedMedian25 => 25.0,
        G::Median => 0.0,
        G::Percentile25 => 25.0,
        G::Percentile50 => 50.0,
        G::Percentile75 => 75.0,
        G::Percentile80 => 80.0,
        G::Percentile90 => 90.0,
        G::Percentile => 95.0,
        G::Percentile97 => 97.0,
        G::Percentile98 => 98.0,
        G::Percentile99 => 99.0,
        _ => return None,
    })
}

/// A percent option: `str2ndd`, non-finite gives 0, clamped.
fn percent_option(options: Option<&[u8]>, default: f64, max: f64) -> f64 {
    let p = match options.filter(|o| !o.is_empty()) {
        Some(o) => {
            let v = str2ndd(o).0;
            if v.is_finite() { v } else { 0.0 }
        }
        None => default,
    };
    p.clamp(0.0, max)
}

/// The countif condition, with C's one extra skipped character.
fn countif_condition(options: Option<&[u8]>) -> (CountifOp, f64) {
    let Some(s) = options.filter(|o| !o.is_empty()) else {
        return (CountifOp::Eq, 0.0);
    };
    let mut i = 0;
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    let at = |i: usize| s.get(i).copied().unwrap_or(0);
    let op = match at(i) {
        b'!' => {
            if matches!(at(i + 1), b'=' | b':') {
                i += 1;
            }
            CountifOp::Ne
        }
        b'>' => {
            if matches!(at(i + 1), b'=' | b':') {
                i += 1;
                CountifOp::Ge
            } else {
                CountifOp::Gt
            }
        }
        b'<' => match at(i + 1) {
            b'=' | b':' => {
                i += 1;
                CountifOp::Le
            }
            b'>' => {
                i += 1;
                CountifOp::Ne
            }
            _ => CountifOp::Lt,
        },
        b'=' => {
            if at(i + 1) == b'=' {
                i += 1;
            }
            CountifOp::Eq
        }
        _ => CountifOp::Eq,
    };
    // C skips one more character in every case (so `40` compares with 0).
    if i < s.len() {
        i += 1;
    }
    while i < s.len() && s[i].is_ascii_whitespace() {
        i += 1;
    }
    (op, str2ndd(&s[i..]).0)
}

/// `median_on_sorted_series()` / `percentile_on_sorted_series(s, n, 0.5)`.
fn median_sorted(s: &[f64]) -> f64 {
    match s.len() {
        0 => f64::NAN,
        1 => s[0],
        n => {
            let index = 0.5 * (n - 1) as f64;
            let (lo, hi) = (index.floor() as usize, index.ceil() as usize);
            if hi >= n || lo == hi || (index - lo as f64).abs() < 1e-7 {
                s[lo]
            } else {
                s[lo] + (index - lo as f64) * (s[hi] - s[lo])
            }
        }
    }
}

fn sort(s: &mut [f64]) {
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
}

impl Grouping {
    /// `create()`: `view_group` and `points_wanted` size the ses/des windows.
    pub fn new(
        method: TimeGrouping,
        options: Option<&[u8]>,
        view_group: i64,
        points_wanted: u64,
        resampling_group: i64,
        resampling_divisor: f64,
    ) -> Self {
        use TimeGrouping as G;
        let window = |max: i64| {
            let w = if view_group == 1 {
                points_wanted as i64
            } else {
                view_group
            };
            w.min(max)
        };
        let state = match method {
            G::Average => State::Average { sum: 0.0, count: 0 },
            G::Sum => State::Sum { sum: 0.0, count: 0 },
            G::Min => State::Min { min: 0.0, count: 0 },
            G::Max => State::Max { max: 0.0, count: 0 },
            G::Extremes => State::Extremes {
                min: 0.0,
                max: 0.0,
                pos: 0,
                neg: 0,
                zeros: 0,
            },
            G::Latest => State::Latest {
                latest: 0.0,
                count: 0,
            },
            G::Countif => {
                let (op, target) = countif_condition(options);
                State::Countif {
                    op,
                    target,
                    matched: 0,
                    count: 0,
                }
            }
            G::IncrementalSum => State::IncrementalSum {
                first: f64::NAN,
                last: f64::NAN,
                count: 0,
            },
            G::Stddev | G::Cv => State::Stddev {
                cv: method == G::Cv,
                count: 0,
                m: 0.0,
                s: 0.0,
            },
            G::Ses => State::Ses {
                alpha: 2.0 / (window(DEFAULT_MAX_WINDOW) as f64 + 1.0),
                level: 0.0,
                count: 0,
            },
            G::Des => {
                let a = 2.0 / (window(DEFAULT_MAX_WINDOW) as f64 + 1.0);
                State::Des {
                    alpha: a,
                    beta: a,
                    level: 0.0,
                    trend: 0.0,
                    count: 0,
                }
            }
            G::Median
            | G::TrimmedMedian1
            | G::TrimmedMedian2
            | G::TrimmedMedian3
            | G::TrimmedMedian
            | G::TrimmedMedian10
            | G::TrimmedMedian15
            | G::TrimmedMedian20
            | G::TrimmedMedian25 => State::Median {
                percent: percent_option(options, named_percent(method).unwrap_or(0.0), 50.0)
                    / 100.0,
                series: Vec::new(),
            },
            G::Percentile25
            | G::Percentile50
            | G::Percentile75
            | G::Percentile80
            | G::Percentile90
            | G::Percentile
            | G::Percentile97
            | G::Percentile98
            | G::Percentile99 => State::Percentile {
                percent: percent_option(options, named_percent(method).unwrap_or(95.0), 100.0)
                    / 100.0,
                series: Vec::new(),
            },
            G::TrimmedMean1
            | G::TrimmedMean2
            | G::TrimmedMean3
            | G::TrimmedMean
            | G::TrimmedMean10
            | G::TrimmedMean15
            | G::TrimmedMean20
            | G::TrimmedMean25 => State::TrimmedMean {
                percent: 1.0
                    - 2.0
                        * (percent_option(options, named_percent(method).unwrap_or(5.0), 50.0)
                            / 100.0),
                series: Vec::new(),
            },
        };
        Grouping {
            state,
            resampling_group,
            resampling_divisor,
        }
    }

    /// `reset()`: before each metric (ses and des keep alpha; incremental-sum restarts).
    pub fn reset(&mut self) {
        match &mut self.state {
            State::Average { sum, count } | State::Sum { sum, count } => (*sum, *count) = (0.0, 0),
            State::Min { min, count } => (*min, *count) = (0.0, 0),
            State::Max { max, count } => (*max, *count) = (0.0, 0),
            State::Extremes {
                min,
                max,
                pos,
                neg,
                zeros,
            } => (*min, *max, *pos, *neg, *zeros) = (0.0, 0.0, 0, 0, 0),
            State::Latest { latest, count } => (*latest, *count) = (0.0, 0),
            State::Countif { matched, count, .. } => (*matched, *count) = (0, 0),
            State::IncrementalSum { first, last, count } => {
                (*first, *last, *count) = (f64::NAN, f64::NAN, 0)
            }
            State::Stddev { count, m, s, .. } => (*count, *m, *s) = (0, 0.0, 0.0),
            State::Ses { level, count, .. } => (*level, *count) = (0.0, 0),
            State::Des {
                level,
                trend,
                count,
                ..
            } => (*level, *trend, *count) = (0.0, 0.0, 0),
            State::Median { series, .. }
            | State::Percentile { series, .. }
            | State::TrimmedMean { series, .. } => series.clear(),
        }
    }

    /// `add()`: one finite value.
    pub fn add(&mut self, v: f64) {
        match &mut self.state {
            State::Average { sum, count } | State::Sum { sum, count } => {
                *sum += v;
                *count += 1;
            }
            State::Min { min, count } => {
                if *count == 0 || v.abs() < min.abs() {
                    *min = v;
                    *count += 1;
                }
            }
            State::Max { max, count } => {
                if *count == 0 || v.abs() > max.abs() {
                    *max = v;
                    *count += 1;
                }
            }
            State::Extremes {
                min,
                max,
                pos,
                neg,
                zeros,
            } => {
                if v > 0.0 {
                    if *pos == 0 || v > *max {
                        *max = v;
                    }
                    *pos += 1;
                } else if v < 0.0 {
                    if *neg == 0 || v < *min {
                        *min = v;
                    }
                    *neg += 1;
                } else {
                    *zeros += 1;
                }
            }
            State::Latest { latest, count } => {
                *latest = v;
                *count += 1;
            }
            State::Countif {
                op,
                target,
                matched,
                count,
            } => {
                let hit = match op {
                    CountifOp::Eq => v == *target,
                    CountifOp::Ne => v != *target,
                    CountifOp::Gt => v > *target,
                    CountifOp::Ge => v >= *target,
                    CountifOp::Lt => v < *target,
                    CountifOp::Le => v <= *target,
                };
                if hit {
                    *matched += 1;
                }
                *count += 1;
            }
            State::IncrementalSum { first, last, count } => {
                if *count == 0 && first.is_nan() {
                    *first = v;
                } else {
                    *last = v;
                }
                *count += 1;
            }
            State::Stddev { count, m, s, .. } => {
                *count += 1;
                if *count == 1 {
                    *m = v;
                    *s = 0.0;
                } else {
                    let new_m = *m + (v - *m) / *count as f64;
                    *s += (v - *m) * (v - new_m);
                    *m = new_m;
                }
            }
            State::Ses {
                alpha,
                level,
                count,
            } => {
                if *count == 0 {
                    *level = v;
                }
                *level = *alpha * v + (1.0 - *alpha) * *level;
                *count += 1;
            }
            State::Des {
                alpha,
                beta,
                level,
                trend,
                count,
            } => {
                if *count == 0 {
                    *level = v;
                    *trend = v;
                } else {
                    if *count == 1 {
                        *trend = v - *trend;
                    }
                    let last = *level;
                    *level = *alpha * v + (1.0 - *alpha) * (*level + *trend);
                    *trend = *beta * (*level - last) + (1.0 - *beta) * *trend;
                }
                *count += 1;
            }
            State::Median { series, .. }
            | State::Percentile { series, .. }
            | State::TrimmedMean { series, .. } => series.push(v),
        }
    }

    /// `flush()`: the row's value; an empty row is 0.0 with EMPTY in `flags`.
    pub fn flush(&mut self, flags: &mut u32) -> f64 {
        let mut empty = || {
            *flags |= VALUE_EMPTY;
            0.0
        };
        let (rg, rd) = (self.resampling_group, self.resampling_divisor);
        match &mut self.state {
            State::Average { sum, count } => {
                let v = if *count == 0 {
                    empty()
                } else if rg != 1 {
                    *sum / rd
                } else {
                    *sum / *count as f64
                };
                (*sum, *count) = (0.0, 0);
                v
            }
            State::Sum { sum, count } => {
                let v = if *count == 0 { empty() } else { *sum };
                (*sum, *count) = (0.0, 0);
                v
            }
            State::Min { min, count } => {
                let v = if *count == 0 { empty() } else { *min };
                (*min, *count) = (0.0, 0);
                v
            }
            State::Max { max, count } => {
                let v = if *count == 0 { empty() } else { *max };
                (*max, *count) = (0.0, 0);
                v
            }
            State::Extremes {
                min,
                max,
                pos,
                neg,
                zeros,
            } => {
                let v = match (*pos > 0, *neg > 0) {
                    (true, true) => {
                        if max.abs() > min.abs() {
                            *max
                        } else {
                            *min
                        }
                    }
                    (true, false) => *max,
                    (false, true) => *min,
                    (false, false) if *zeros > 0 => 0.0,
                    _ => empty(),
                };
                (*min, *max, *pos, *neg, *zeros) = (0.0, 0.0, 0, 0, 0);
                v
            }
            State::Latest { latest, count } => {
                let v = if *count == 0 { empty() } else { *latest };
                (*latest, *count) = (0.0, 0);
                v
            }
            State::Countif { matched, count, .. } => {
                let v = if *count == 0 {
                    empty()
                } else {
                    *matched as f64 * 100.0 / *count as f64
                };
                (*matched, *count) = (0, 0);
                v
            }
            State::IncrementalSum { first, last, count } => {
                let v = if *count == 0 || first.is_nan() || last.is_nan() {
                    empty()
                } else {
                    let v = *last - *first;
                    *first = *last;
                    v
                };
                *last = f64::NAN;
                *count = 0;
                v
            }
            State::Stddev { cv, count, m, s } => {
                let v = match *count {
                    0 => empty(),
                    1 => 0.0,
                    n => {
                        let variance = *s / (n - 1) as f64;
                        let v = if *cv {
                            100.0 * variance.sqrt() / m.abs()
                        } else {
                            variance.sqrt()
                        };
                        if v.is_finite() { v } else { empty() }
                    }
                };
                (*count, *m, *s) = (0, 0.0, 0.0);
                v
            }
            State::Ses { level, count, .. } | State::Des { level, count, .. } => {
                // No state change: the level runs across rows.
                if *count == 0 || !level.is_finite() {
                    empty()
                } else {
                    *level
                }
            }
            State::Median { percent, series } => {
                let v = median_flush(series, *percent).unwrap_or_else(empty);
                series.clear();
                v
            }
            State::Percentile { percent, series } => {
                let v = percentile_flush(series, *percent, false).unwrap_or_else(empty);
                series.clear();
                v
            }
            State::TrimmedMean { percent, series } => {
                let v = percentile_flush(series, *percent, true).unwrap_or_else(empty);
                series.clear();
                v
            }
        }
    }
}

/// The median flush with a range trim.
fn median_flush(s: &mut [f64], percent: f64) -> Option<f64> {
    match s.len() {
        0 => None,
        1 => Some(s[0]),
        n => {
            sort(s);
            if percent > 0.0 {
                let (min, max) = (s[0], s[n - 1]);
                let delta = (max - min) * percent;
                let start = s.iter().position(|&v| v >= min + delta).unwrap_or(0);
                let end = (start + 1..n)
                    .rev()
                    .find(|&i| s[i] <= max - delta)
                    .unwrap_or(start);
                Some(if start == end {
                    s[start]
                } else {
                    median_sorted(&s[start..=end])
                })
            } else {
                Some(median_sorted(s))
            }
        }
    }
}

/// The percentile and trimmed-mean flush: the mean of the lowest (or, with a negative sample, highest) slots.
fn percentile_flush(s: &mut [f64], p: f64, trimmed: bool) -> Option<f64> {
    let n = s.len();
    match n {
        0 => return None,
        1 => return Some(s[0]),
        _ => {}
    }
    sort(s);
    let (min, max) = (s[0], s[n - 1]);
    if min == max {
        return Some(min);
    }
    let nf = n as f64;
    let mut slots = (nf * p) as usize;
    if slots == 0 {
        slots = 1;
    }
    let delta = p - slots as f64 / nf;
    let interp = if delta > 0.0 {
        delta / ((slots + 1) as f64 / nf - slots as f64 / nf)
    } else {
        0.0
    };
    // Walk up from the bottom, or down from the top when any sample is negative; trimmed means start inside.
    let offset = if trimmed { (n - slots) / 2 } else { 0 };
    let (start, step): (isize, isize) = if min >= 0.0 && max >= 0.0 {
        (offset as isize, 1)
    } else {
        ((n - 1 - offset) as isize, -1)
    };
    let stop = start + step * slots as isize;
    let (last, interpolation) = (stop - step, stop);
    let mut sum = 0.0;
    let mut slot = start;
    while slot != stop {
        sum += s[slot as usize];
        slot += step;
    }
    let mut counted = slots;
    if interp > 0.0 && interpolation >= 0 && (interpolation as usize) < n {
        sum += s[interpolation as usize] * interp;
        sum += s[last as usize] * (1.0 - interp);
        counted += 1;
    }
    Some(sum / counted as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(method: TimeGrouping, options: Option<&[u8]>, values: &[f64]) -> (f64, u32) {
        let mut g = Grouping::new(method, options, 1, 10, 1, 1.0);
        for &v in values {
            g.add(v);
        }
        let mut flags = 0;
        (g.flush(&mut flags), flags)
    }

    #[test]
    fn simple_methods() {
        use TimeGrouping as G;
        assert_eq!(run(G::Average, None, &[1.0, 2.0, 6.0]), (3.0, 0));
        assert_eq!(run(G::Average, None, &[]), (0.0, VALUE_EMPTY));
        assert_eq!(
            run(G::Sum, None, &[0.0]),
            (0.0, 0),
            "a single 0 is 0, not empty"
        );
        assert_eq!(
            run(G::Min, None, &[-5.0, 2.0, -1.0]).0,
            -1.0,
            "absolute comparison, sign kept"
        );
        assert_eq!(run(G::Max, None, &[-5.0, 2.0]).0, -5.0);
        assert_eq!(run(G::Extremes, None, &[-5.0, 3.0]).0, -5.0);
        assert_eq!(run(G::Extremes, None, &[0.0]), (0.0, 0));
        assert_eq!(run(G::Latest, None, &[1.0, 7.0]).0, 7.0);
        assert_eq!(run(G::Stddev, None, &[5.0]), (0.0, 0));
        assert_eq!(
            run(G::Stddev, None, &[2.0, 4.0, 4.0, 4.0, 5.0, 5.0, 7.0, 9.0]).0,
            (32.0f64 / 7.0).sqrt()
        );
        assert_eq!(run(G::Cv, None, &[-1.0, 1.0]).1, VALUE_EMPTY, "mean 0");
    }

    #[test]
    fn countif_quirks() {
        use TimeGrouping as G;
        // `40` skips one character after the operator: `== 0`.
        assert_eq!(run(G::Countif, Some(b"40"), &[0.0, 40.0]).0, 50.0);
        assert_eq!(run(G::Countif, Some(b">=2"), &[1.0, 2.0, 3.0, 4.0]).0, 75.0);
        assert_eq!(run(G::Countif, Some(b"<>1"), &[1.0, 2.0]).0, 50.0);
        assert_eq!(run(G::Countif, None, &[0.0, 1.0]).0, 50.0);
    }

    #[test]
    fn incremental_sum_carries_first_across_rows() {
        let mut g = Grouping::new(TimeGrouping::IncrementalSum, None, 1, 10, 1, 1.0);
        let mut f = 0;
        g.add(10.0);
        g.add(15.0);
        assert_eq!(g.flush(&mut f), 5.0);
        g.add(18.0);
        assert_eq!(
            g.flush(&mut f),
            3.0,
            "the last value becomes the next first"
        );
    }

    #[test]
    fn median_and_percentile() {
        use TimeGrouping as G;
        assert_eq!(run(G::Median, None, &[3.0, 1.0, 2.0, 10.0]).0, 2.5);
        assert_eq!(run(G::Percentile50, None, &[1.0, 2.0, 3.0, 4.0]).0, 1.5);
        assert_eq!(run(G::Percentile, None, &[7.0, 7.0]).0, 7.0);
        // C's trimmed mean of [1, 2, 3] at 5%: two slots (1 + 2) plus an interpolated slot 3*0.7 + 2*0.3, over 3.
        assert!((run(G::TrimmedMean, None, &[1.0, 2.0, 3.0]).0 - 1.9).abs() < 1e-12);
    }

    #[test]
    fn ses_levels_run_across_rows() {
        let mut g = Grouping::new(TimeGrouping::Ses, None, 1, 3, 1, 1.0);
        let mut f = 0;
        g.add(4.0);
        let first = g.flush(&mut f);
        assert_eq!(first, 4.0);
        g.add(8.0);
        assert_eq!(g.flush(&mut f), 0.5 * 8.0 + 0.5 * 4.0);
    }
}
