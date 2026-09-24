//! Reading each admitted metric into result rows, one storage tier: the plan and the LATEST fast path
//! (`src/web/api/queries/query-plan.c`), the execute loop (`query-execute.c`) and the per-metric driver
//! (`rrd2rrdr()`, `query.c`). Spec §4.3, §5.

use std::time::Instant;

use netdata_agent_storage::ram::RamQuery;
use netdata_agent_storage::storage_number::SN_FLAG_RESET;
use netdata_agent_storage::storage_point::StoragePoint;

use crate::grouping::Grouping;
use crate::rrdr::{Rrdr, result_flags, value_flags};
use crate::tables::{TimeGrouping, options};
use crate::target::{QueryMetric, QueryTarget, metric_status, status};
use crate::window::Window;

/// `POINTS_TO_EXPAND_QUERY`: the points a plan reads past its end.
const POINTS_TO_EXPAND_QUERY: i64 = 5;

/// `QUERY_POINT_MODE`: how a point that spans a row end is projected onto it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PointMode {
    Linear,
    Total,
    Hold,
}

/// `TIER_QUERY_FETCH`: which statistic of a storage point is its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fetch {
    Average,
    Min,
    Max,
    Sum,
}

/// The fetch and point mode of the grouping registry entry: LINEAR/AVERAGE except `min`, `max` and `sum` (TOTAL).
fn fetch_and_mode(method: TimeGrouping) -> (Fetch, PointMode) {
    match method {
        TimeGrouping::Min => (Fetch::Min, PointMode::Linear),
        TimeGrouping::Max => (Fetch::Max, PointMode::Linear),
        TimeGrouping::Sum => (Fetch::Sum, PointMode::Total),
        _ => (Fetch::Average, PointMode::Linear),
    }
}

/// `QUERY_POINT`.
#[derive(Debug, Clone, Copy)]
struct QueryPoint {
    sp: StoragePoint,
    value: f64,
    added: bool,
    tier: usize,
}

/// `QUERY_POINT_EMPTY`.
const EMPTY_POINT: QueryPoint = QueryPoint {
    sp: StoragePoint::UNSET,
    value: f64::NAN,
    added: false,
    tier: 0,
};

/// `query_point_total_projection()`: the share of a point's total that falls in `(row_start, row_end]`.
fn total_projection(point: &QueryPoint, row_start: i64, row_end: i64) -> f64 {
    let duration = point.sp.end_time_s - point.sp.start_time_s;
    if duration <= 0 {
        return point.value;
    }
    let overlap_start = point.sp.start_time_s.max(row_start);
    let overlap_end = point.sp.end_time_s.min(row_end);
    if overlap_end <= overlap_start {
        return f64::NAN;
    }
    if overlap_start == point.sp.start_time_s && overlap_end == point.sp.end_time_s {
        return point.value;
    }
    point.value * (overlap_end - overlap_start) as f64 / duration as f64
}

/// What `rrd2rrdr_query_ops_prep()` decided for one metric.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Prepared {
    /// Serve the collector's last stored value (`query_latest_fast_path()`).
    Latest { value: f64, time_s: i64 },
    /// Read tier 0 over `[after, before]` (`qm->plan.array[0]`).
    Plan { after: i64, before: i64 },
}

/// `QUERY_ENGINE_OPS`: one metric's execution state.
struct Ops {
    fetch: Fetch,
    point_mode: PointMode,
    view_update_every: i64,
    query_granularity: i64,
    plan_switch_time_offset: i64,
    current_plan_expire_time: i64,
    result_plan_expire_time: i64,
    result_plan_expire_time_overflow: bool,
    group_point: StoragePoint,
    query_point: StoragePoint,
    group_value_flags: u32,
    group_points_non_zero: usize,
    db_points_read: usize,
}

impl Ops {
    fn new(qt: &QueryTarget, window: &Window) -> Self {
        let (fetch, mut point_mode) = fetch_and_mode(qt.request.time_group);
        if window.options & options::ANOMALY_BIT != 0 {
            point_mode = PointMode::Hold;
        }
        let view_update_every = window.view_update_every();
        Ops {
            fetch,
            point_mode,
            view_update_every,
            query_granularity: window.query_granularity,
            plan_switch_time_offset: if point_mode == PointMode::Total {
                view_update_every
            } else {
                0
            },
            current_plan_expire_time: 0,
            result_plan_expire_time: 0,
            result_plan_expire_time_overflow: false,
            group_point: StoragePoint::UNSET,
            query_point: StoragePoint::UNSET,
            group_value_flags: value_flags::NOTHING,
            group_points_non_zero: 0,
            db_points_read: 0,
        }
    }

    /// `query_planer_set_expire_time()`.
    fn set_expire_time(&mut self, expire_time: i64) {
        self.current_plan_expire_time = expire_time;
        match expire_time.checked_add(self.plan_switch_time_offset) {
            Some(t) => {
                self.result_plan_expire_time = t;
                self.result_plan_expire_time_overflow = false;
            }
            None => {
                self.result_plan_expire_time = i64::MAX;
                self.result_plan_expire_time_overflow = true;
            }
        }
    }

    /// `query_planer_next_plan()` with a single plan: nothing to switch to, the plan runs to the window's end.
    fn next_plan(&mut self, window: &Window) -> bool {
        self.set_expire_time(window.before);
        false
    }

    /// `query_project_point()`.
    fn project(&self, point: &mut QueryPoint, previous: &QueryPoint, now: i64) {
        match self.point_mode {
            PointMode::Linear => {
                if point.sp.end_time_s - point.sp.start_time_s > 1
                    && previous.sp.end_time_s == point.sp.start_time_s
                    && point.value.is_finite()
                    && previous.value.is_finite()
                {
                    point.value = previous.value
                        + (point.value - previous.value)
                            * (1.0
                                - (point.sp.end_time_s - now) as f64
                                    / (point.sp.end_time_s - point.sp.start_time_s) as f64);
                    point.sp.end_time_s = now;
                }
            }
            PointMode::Total => {
                point.value = total_projection(point, now - self.view_update_every, now);
            }
            PointMode::Hold => {}
        }
    }

    /// `query_add_point_to_group()`: `source_end` decides whether the point's flags and statistics belong to the
    /// row ending at `now_end`.
    fn add(&mut self, grouping: &mut Grouping, point: &QueryPoint, now_end: i64, source_end: i64) {
        if !point.value.is_finite() {
            return;
        }
        // FP_ZERO: -0.0 is zero, subnormals are not.
        if point.value != 0.0 {
            self.group_points_non_zero += 1;
        }
        let in_row = source_end > now_end - self.view_update_every && source_end <= now_end;
        if point.sp.flags & SN_FLAG_RESET != 0 && in_row {
            self.group_value_flags |= value_flags::RESET;
        }
        grouping.add(point.value);
        if point.tier != 0 || in_row {
            self.group_point.merge_to(&point.sp);
        }
        if !point.added {
            self.query_point.merge_to(&point.sp);
        }
    }
}

/// `query_metric_is_valid_tier()`.
fn tier_is_valid(qm: &QueryMetric) -> bool {
    let t = &qm.tier0;
    t.dim.ring().is_some() && t.first_time_s != 0 && t.last_time_s != 0 && t.update_every_s != 0
}

/// `rrd2rrdr_query_ops_prep()`: the LATEST fast path, else the plan (`query_plan()`); `None` fails the metric.
fn prepare(qt: &mut QueryTarget, d: usize, window: &Window) -> Option<Prepared> {
    let qm = &qt.query[d];
    let (db_last, db_ue) = (qm.tier0.last_time_s, qm.tier0.update_every_s);
    if qt.request.time_group == TimeGrouping::Latest
        && window.points == 1
        && qt.request.resampling_time <= 0
        && window.options & (options::SELECTED_TIER | options::ANOMALY_BIT) == 0
        && db_last <= window.before
        && db_last.saturating_add(db_ue) >= window.after
    {
        // NaN without a live dimension, or when the last sample was a gap: the storage query serves those.
        let value = qt.dimensions[qm.dimension]
            .rm
            .dim()
            .map_or(f64::NAN, |dim| dim.collection().last_stored_value);
        if value.is_finite() {
            return Some(Prepared::Latest {
                value,
                time_s: db_last,
            });
        }
    }
    // Below two tiers the best tier is 0, and a selected tier resolves to it too: tier 0 must be valid either way.
    if !tier_is_valid(qm) {
        return None;
    }
    let (first, last) = (qm.tier0.first_time_s, qm.tier0.last_time_s);
    if first > window.before || last < window.after {
        return None;
    }
    let (after, before) = (first.max(window.after), last.min(window.before));
    qt.query[d].plan = Some((after, before));
    qt.db.tier0_queries += 1;
    Some(Prepared::Plan { after, before })
}

/// `rrd2rrdr_query_execute_latest_fast_path()`.
fn execute_latest(
    r: &mut Rrdr,
    col: usize,
    qm: &QueryMetric,
    window: &Window,
    value: f64,
    time_s: i64,
) -> StoragePoint {
    let value = if window.options & options::ABSOLUTE != 0 {
        value.abs()
    } else {
        value
    };
    let idx = r.index(0, col);
    if value != 0.0 {
        r.od[col] |= metric_status::NONZERO;
    }
    r.o[idx] = value_flags::NOTHING;
    r.v[idx] = value;
    r.ar[idx] = 0.0;
    if r.queries_count != 0 {
        if value < r.view.min {
            r.view.min = value;
        }
        if value > r.view.max {
            r.view.max = value;
        }
    } else {
        r.view.min = value;
        r.view.max = value;
    }
    r.queries_count += 1;
    r.result_points_generated += 1;
    StoragePoint {
        min: value,
        max: value,
        sum: value,
        start_time_s: time_s - qm.tier0.update_every_s,
        end_time_s: time_s,
        count: 1,
        anomaly_count: 0,
        flags: 0,
    }
}

/// `rrd2rrdr_query_execute()` over one plan: fills column `col` and returns the metric's merged points; the reads
/// are left in `ops.db_points_read`.
fn execute_plan(
    r: &mut Rrdr,
    col: usize,
    grouping: &mut Grouping,
    qm: &QueryMetric,
    window: &Window,
    ops: &mut Ops,
    handle: &mut RamQuery<'_>,
) -> StoragePoint {
    let opts = window.options;
    let use_anomaly_bit_as_value = opts & options::ANOMALY_BIT != 0;
    let points_wanted = r.rows;
    let vue = ops.view_update_every;
    let mut points_added = 0;
    let (mut min, mut max) = (r.view.min, r.view.max);
    let (mut last2, mut last1, mut new) = (EMPTY_POINT, EMPTY_POINT, EMPTY_POINT);
    let mut now_start = window.after - ops.query_granularity;
    let mut now_end = window.after + (vue - ops.query_granularity);
    let mut read_since_plan_switch = 0usize;
    let mut finished_counter = 0;

    while points_added < points_wanted && finished_counter <= 10 {
        if now_end >= ops.result_plan_expire_time && !ops.result_plan_expire_time_overflow {
            ops.next_plan(window);
            read_since_plan_switch = 0;
        }

        // Interpolation can consume a tier-0 point before the row that owns its metadata.
        if new.added
            && new.tier == 0
            && new.sp.end_time_s > now_start
            && new.sp.end_time_s <= now_end
            && new.value.is_finite()
        {
            if new.sp.flags & SN_FLAG_RESET != 0 {
                ops.group_value_flags |= value_flags::RESET;
            }
            ops.group_point.merge_to(&new.sp);
        }

        // Read every point that ends before now_end.
        let mut count_same_end_time = 0;
        while count_same_end_time < 100 {
            if count_same_end_time == 0 {
                last2 = last1;
                last1 = new;
            }
            if handle.is_finished() {
                finished_counter += 1;
                if count_same_end_time != 0 {
                    last2 = last1;
                    last1 = new;
                }
                new = EMPTY_POINT;
                new.sp.start_time_s = last1.sp.end_time_s;
                new.sp.end_time_s = now_end;
                break;
            }
            finished_counter = 0;
            read_since_plan_switch += 1;
            let mut sp = handle.next_metric();
            ops.db_points_read += 1;
            if opts & options::ABSOLUTE != 0 {
                sp.make_positive();
            }
            // The point crosses the plan's end: with one plan the switch only moves the expiry.
            if sp.end_time_s >= ops.current_plan_expire_time {
                ops.next_plan(window);
            }
            new.sp = sp;
            new.tier = 0;
            new.added = false;
            new.value = if !sp.is_unset() && !sp.is_gap() {
                if use_anomaly_bit_as_value {
                    sp.anomaly_rate()
                } else {
                    match ops.fetch {
                        Fetch::Average => sp.sum / f64::from(sp.count),
                        Fetch::Min => sp.min,
                        Fetch::Max => sp.max,
                        Fetch::Sum => {
                            let mut value = sp.sum;
                            let duration = sp.end_time_s - sp.start_time_s;
                            if qm.values_stored_as_rates && duration > 0 {
                                value *= duration as f64 / f64::from(sp.count);
                            }
                            if sp.start_time_s < now_start && sp.end_time_s < now_end {
                                value = total_projection(
                                    &QueryPoint { value, ..new },
                                    now_start,
                                    now_end,
                                );
                            }
                            value
                        }
                    }
                }
            } else {
                f64::NAN
            };

            // A zero-duration point from the engine is widened to one update interval.
            if read_since_plan_switch > 1 && new.sp.start_time_s == new.sp.end_time_s {
                new.sp.start_time_s = new.sp.end_time_s - qm.tier0.update_every_s;
            }
            // The engine did not advance.
            if read_since_plan_switch > 1 && new.sp.end_time_s <= last1.sp.end_time_s {
                count_same_end_time += 1;
                continue;
            }
            count_same_end_time = 0;

            if new.sp.end_time_s < now_end {
                if new.sp.end_time_s > now_start {
                    ops.add(grouping, &new, now_end, new.sp.end_time_s);
                    new.added = true;
                }
                // A point wholly before the row is skipped.
                continue;
            }
            // The point ends in the future: it is projected below.
            break;
        }
        if count_same_end_time != 0 && new.sp.end_time_s <= last1.sp.end_time_s {
            new.sp.end_time_s = now_end;
        }

        // Emit every row the three points in memory (last2, last1, new) cover.
        let stop_time = new.sp.end_time_s;
        let mut new_point_total_remaining = f64::NAN;
        loop {
            let mut current;
            let source_end;
            if now_end > new.sp.start_time_s {
                current = new;
                source_end = new.sp.end_time_s;
                new.added = true;
                ops.project(&mut current, &last1, now_end);
                if ops.point_mode == PointMode::Total && current.value.is_finite() {
                    if new_point_total_remaining.is_finite() {
                        new_point_total_remaining -= current.value;
                    } else {
                        let duration = new.sp.end_time_s - new.sp.start_time_s;
                        new_point_total_remaining = if duration > 0 {
                            new.value * (new.sp.end_time_s - now_end) as f64 / duration as f64
                        } else {
                            0.0
                        };
                    }
                }
            } else if now_end <= last1.sp.end_time_s {
                current = last1;
                source_end = last1.sp.end_time_s;
                last1.added = true;
                ops.project(&mut current, &last2, now_end);
            } else {
                current = EMPTY_POINT;
                source_end = 0;
            }
            ops.add(grouping, &current, now_end, source_end);

            debug_assert_eq!(r.t[points_added], now_end, "row time");
            let idx = r.index(points_added, col);
            if ops.group_points_non_zero != 0 {
                r.od[col] |= metric_status::NONZERO;
            }
            let mut flags = ops.group_value_flags;
            let value = grouping.flush(&mut flags);
            r.o[idx] = flags;
            r.v[idx] = value;
            r.ar[idx] = ops.group_point.anomaly_rate();
            if points_added != 0 || r.queries_count != 0 {
                if value < min {
                    min = value;
                }
                if value > max {
                    max = value;
                }
            } else {
                min = value;
                max = value;
            }
            points_added += 1;
            ops.group_value_flags = value_flags::NOTHING;
            ops.group_points_non_zero = 0;
            ops.group_point = StoragePoint::UNSET;

            if points_added < points_wanted {
                now_end += vue;
            }
            if !(now_end <= stop_time && points_added < points_wanted) {
                break;
            }
        }
        if points_added >= points_wanted {
            break;
        }

        // Carry what the last point holds past this row into the next one.
        let next_row_start = now_end - vue;
        if new.sp.end_time_s > next_row_start
            && new.value.is_finite()
            && ((new.added
                && (ops.point_mode == PointMode::Total
                    || (new.tier == 0 && new.sp.start_time_s < next_row_start)))
                || (!new.added && ops.point_mode == PointMode::Total))
        {
            let settle =
                ops.point_mode == PointMode::Total || new.sp.end_time_s >= qm.tier0.last_time_s;
            let mut carried = new.value;
            if ops.point_mode == PointMode::Total && new.sp.end_time_s < now_end {
                carried = if new_point_total_remaining.is_finite() {
                    new_point_total_remaining
                } else {
                    total_projection(&new, next_row_start, now_end)
                };
            }
            if carried.is_finite() {
                if settle {
                    if carried != 0.0 {
                        ops.group_points_non_zero += 1;
                    }
                    if new.sp.flags & SN_FLAG_RESET != 0 {
                        ops.group_value_flags |= value_flags::RESET;
                    }
                    grouping.add(carried);
                    if !new.added {
                        ops.query_point.merge_to(&new.sp);
                    }
                }
                // Tier-0 evidence is carried onto its own row at the top of the row loop.
                if new.tier != 0 {
                    ops.group_point.merge_to(&new.sp);
                }
            }
            if settle {
                new.added = true;
            }
        }

        // The row loop advanced now_end past this row and the main loop advances it again.
        now_end -= vue;
        now_start = now_end;
        now_end += vue;
    }

    while points_added < points_wanted {
        let idx = r.index(points_added, col);
        r.o[idx] = value_flags::EMPTY;
        r.v[idx] = 0.0;
        r.ar[idx] = 0.0;
        points_added += 1;
    }

    r.queries_count += 1;
    r.view.min = min;
    r.view.max = max;
    r.result_points_generated += points_added;
    r.db_points_read += ops.db_points_read;
    ops.query_point
}

/// Stops a query between metrics: the client went away (`interrupt_callback`) or `timeout` ran out.
pub struct Control<'a> {
    /// When the request arrived (`qt->timings.received_ut`).
    pub received: Instant,
    pub interrupted: &'a dyn Fn() -> bool,
}

impl Control<'_> {
    fn cancel(&self, timeout_ms: i32) -> bool {
        (self.interrupted)()
            || (timeout_ms != 0
                && self.received.elapsed().as_secs_f64() * 1000.0 > f64::from(timeout_ms))
    }
}

/// `rrd2rrdr()` for a v1 query: one column per admitted metric, in `qt.query` order. Clears NONZERO from
/// `window.options` when no executed metric is nonzero.
pub fn run_v1(qt: &mut QueryTarget, window: &mut Window, control: &Control) -> Rrdr {
    let mut r = Rrdr::new(window, qt.query.len());
    for (d, qm) in qt.query.iter().enumerate() {
        let rm = &qt.dimensions[qm.dimension].rm;
        r.di[d] = rm.id().to_string();
        r.dn[d] = rm.state().name;
    }
    r.view.flags |= if window.relative {
        result_flags::RELATIVE
    } else {
        result_flags::ABSOLUTE
    };
    let mut grouping = Grouping::new(
        qt.request.time_group,
        qt.request.time_group_options.as_deref(),
        window.group,
        window.points,
        window.resampling_group,
        window.resampling_divisor,
    );
    let (mut used, mut nonzero) = (0, 0);
    for d in 0..qt.query.len() {
        let prepared = prepare(qt, d, window);
        r.od[d] = qt.query[d].status;
        grouping.reset();
        let qd = qt.query[d].dimension;
        let qi = qt.dimensions[qd].instance;
        let qc = qt.instances[qi].context;
        let qn = qt.contexts[qc].node;
        let Some(prepared) = prepared else {
            qt.instances[qi].metrics.failed += 1;
            qt.contexts[qc].metrics.failed += 1;
            qt.nodes[qn].metrics.failed += 1;
            qt.dimensions[qd].status |= status::FAILED;
            qt.query[d].status |= metric_status::FAILED;
            continue;
        };
        let qm = &qt.query[d];
        let query_points = match prepared {
            Prepared::Latest { value, time_s } => {
                execute_latest(&mut r, d, qm, window, value, time_s)
            }
            Prepared::Plan { after, before } => {
                let mut ops = Ops::new(qt, window);
                ops.set_expire_time(before);
                let dim = qm.tier0.dim.clone();
                let Some(ring) = dim.ring() else {
                    unreachable!("prepare() checked the ring")
                };
                let mut handle = ring.query(
                    after,
                    before + qm.tier0.update_every_s * POINTS_TO_EXPAND_QUERY,
                );
                let query_points =
                    execute_plan(&mut r, d, &mut grouping, qm, window, &mut ops, &mut handle);
                qt.db.tier0_points += ops.db_points_read;
                query_points
            }
        };
        r.od[d] |= metric_status::QUERIED;
        qt.instances[qi].metrics.queried += 1;
        qt.contexts[qc].metrics.queried += 1;
        qt.nodes[qn].metrics.queried += 1;
        qt.dimensions[qd].status |= status::QUERIED;
        let qm = &mut qt.query[d];
        qm.query_points = query_points;
        qm.status |= metric_status::QUERIED;
        if qm.status & metric_status::NONZERO != 0 {
            nonzero += 1;
        }
        used += 1;
        if control.cancel(qt.request.timeout_ms) {
            r.view.flags |= result_flags::CANCEL;
            break;
        }
    }
    if used != 0 && window.options & options::NONZERO != 0 && nonzero == 0 {
        window.options &= !options::NONZERO;
    }
    qt.executed = Some(Instant::now());
    r
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use netdata_agent_rrd::chart::Algorithm;
    use netdata_agent_rrd::host::Host;
    use netdata_agent_storage::storage_number::{SN_FLAG_NOT_ANOMALOUS, pack, unpack};

    use super::*;
    use crate::testing::{T0, host, v1_target};

    fn run(h: &Arc<Host>, query: &str) -> (QueryTarget, Window, Rrdr) {
        let (mut qt, mut window) = v1_target(h, query);
        let control = Control {
            received: Instant::now(),
            interrupted: &|| false,
        };
        let r = run_v1(&mut qt, &mut window, &control);
        (qt, window, r)
    }

    fn rows(r: &Rrdr) -> Vec<(f64, u32)> {
        (0..r.rows).map(|i| (r.v[i], r.o[i])).collect()
    }

    const EMPTY_ROW: (f64, u32) = (0.0, value_flags::EMPTY);

    #[test]
    fn natural_points_follow_the_worked_example() {
        let h = host();
        let (qt, window, r) = run(&h, &format!("after={T0}&before={}", T0 + 6));
        assert_eq!(
            (window.after, window.before, window.points),
            (T0, T0 + 6, 7)
        );
        let thirty = unpack(pack(30.0, 0));
        assert_eq!(
            rows(&r),
            vec![
                EMPTY_ROW,
                (10.0, 0),
                EMPTY_ROW,
                (20.0, 0),
                (thirty, 0),
                EMPTY_ROW,
                (40.0, 0)
            ]
        );
        assert_eq!(r.t, (T0..=T0 + 6).collect::<Vec<_>>());
        assert_eq!((r.view.min, r.view.max), (0.0, 40.0));
        assert_eq!(r.view.flags, result_flags::ABSOLUTE);
        let selected = metric_status::SELECTED;
        assert_eq!(
            r.od[0],
            selected | metric_status::NONZERO | metric_status::QUERIED
        );
        assert_eq!((qt.db.tier0_queries, qt.db.tier0_points), (1, 7));
        assert_eq!(
            (r.queries_count, r.result_points_generated, r.db_points_read),
            (1, 7, 7)
        );
        assert_eq!(
            qt.query[0].query_points,
            StoragePoint {
                min: 10.0,
                max: 40.0,
                sum: 10.0 + 20.0 + thirty + 40.0,
                start_time_s: T0,
                end_time_s: T0 + 6,
                count: 4,
                anomaly_count: 0,
                // The first merged point is copied whole; later merges only add RESET.
                flags: SN_FLAG_NOT_ANOMALOUS,
            }
        );
        assert_eq!(
            qt.query[0].status,
            selected | metric_status::QUERIED,
            "v1 execution never copies NONZERO back to the metric"
        );
        assert_eq!(qt.dimensions[0].status & status::QUERIED, status::QUERIED);
        assert_eq!(
            (qt.instances[0].metrics.queried, qt.nodes[0].metrics.queried),
            (1, 1)
        );
    }

    #[test]
    fn virtual_points_group_the_samples() {
        let h = host();
        let thirty = unpack(pack(30.0, 0));
        let (qt, window, r) = run(&h, &format!("after={T0}&before={}&points=3", T0 + 6));
        assert_eq!((window.after, window.points, window.group), (T0 + 1, 3, 2));
        assert_eq!(
            rows(&r),
            vec![(10.0, 0), ((20.0 + thirty) / 2.0, 0), (40.0, 0)]
        );
        assert_eq!(qt.db.tier0_points, 6);

        let (qt, window, r) = run(&h, &format!("after={T0}&before={}&points=2", T0 + 6));
        assert_eq!(
            (window.after, window.before, window.group),
            (T0 + 2, T0 + 7, 3)
        );
        assert_eq!(rows(&r), vec![((20.0 + thirty) / 2.0, 0), (40.0, 0)]);
        assert_eq!(qt.db.tier0_points, 6);
    }

    #[test]
    fn latest_serves_the_last_stored_value() {
        let h = host();
        let dim = h.charts().find("t.a").unwrap().dim("d").unwrap();
        dim.update_collection(|c| c.last_stored_value = -41.5);
        let q = format!("after={T0}&before={}&points=1&group=latest", T0 + 6);
        let (qt, _, r) = run(&h, &q);
        assert_eq!(rows(&r), vec![(-41.5, 0)]);
        assert_eq!(
            r.od[0],
            metric_status::SELECTED | metric_status::NONZERO | metric_status::QUERIED
        );
        assert_eq!((r.view.min, r.view.max), (-41.5, -41.5));
        assert_eq!(
            (qt.db.tier0_queries, qt.db.tier0_points, r.db_points_read),
            (0, 0, 0)
        );
        assert_eq!(
            qt.query[0].query_points,
            StoragePoint {
                min: -41.5,
                max: -41.5,
                sum: -41.5,
                start_time_s: T0 + 5,
                end_time_s: T0 + 6,
                count: 1,
                anomaly_count: 0,
                flags: 0,
            }
        );
        let (_, _, r) = run(&h, &format!("{q}&options=abs"));
        assert_eq!(rows(&r), vec![(41.5, 0)]);

        // Without a finite cached value, and with anomaly-bit, the storage path answers.
        let (qt, _, r) = run(&h, &format!("{q}&options=anomaly-bit"));
        assert_eq!(qt.db.tier0_queries, 1);
        assert_eq!(rows(&r), vec![(0.0, 0)], "no stored sample is anomalous");
        dim.update_collection(|c| c.last_stored_value = f64::NAN);
        let (qt, _, r) = run(&h, &q);
        assert_eq!(qt.db.tier0_queries, 1);
        assert_eq!(rows(&r), vec![(40.0, 0)]);
    }

    #[test]
    fn a_metric_ending_before_the_window_fails() {
        let h = host();
        // `e` ends two seconds before `d`: admission tolerates two update intervals, the plan does not.
        let chart = h.charts().find("t.a").unwrap();
        let (e, _) = chart.dim_add("e", None, 1, 1, Algorithm::Absolute);
        for t in T0 + 1..=T0 + 4 {
            e.store_metric(t as u64 * 1_000_000, 1.0, SN_FLAG_NOT_ANOMALOUS);
        }
        h.contexts().process_queued();
        let (qt, window, r) = run(&h, &format!("after={}&before={}", T0 + 6, T0 + 6));
        assert_eq!((window.after, window.points), (T0 + 6, 1));
        assert_eq!(r.dn, ["d", "e"]);
        assert_eq!(rows(&r), vec![(40.0, 0)]);
        assert_eq!(
            (r.od[1], qt.query[1].status),
            (
                metric_status::SELECTED,
                metric_status::SELECTED | metric_status::FAILED
            )
        );
        assert_eq!(qt.dimensions[1].status & status::FAILED, status::FAILED);
        assert_eq!(
            (
                qt.instances[0].metrics.failed,
                qt.contexts[0].metrics.failed
            ),
            (1, 1)
        );
        assert_eq!(
            (
                qt.instances[0].metrics.queried,
                qt.db.tier0_queries,
                r.queries_count
            ),
            (1, 1, 1)
        );
    }

    #[test]
    fn nonzero_is_dropped_when_no_metric_is_nonzero() {
        let h = host();
        let (_, window, _) = run(&h, &format!("after={T0}&before={}&options=nonzero", T0 + 6));
        assert_eq!(window.options & options::NONZERO, 0);
    }

    #[test]
    fn a_negative_timeout_cancels_after_the_first_metric() {
        let h = host();
        let (_, _, r) = run(&h, &format!("after={T0}&before={}&timeout=-1", T0 + 6));
        assert_eq!(r.view.flags & result_flags::CANCEL, result_flags::CANCEL);
    }
}
