//! Reading each admitted metric into result rows: the LATEST fast path and the plans of `plan.rs` (a valid selected
//! tier, else the best tier with coarser tiers before its data and finer ones after it;
//! `src/web/api/queries/query-plan.c`), the execute loop that switches between them (`query-execute.c`) and the
//! per-metric driver (`rrd2rrdr()`, `query.c`). Spec §4.3, §5.

use netdata_agent_log::{Priority, Source, nd_log};
use std::time::Instant;

use netdata_agent_rrd::pulse::{Queries, QuerySource};
use netdata_agent_rrd::storage::TierHandle;
use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
use netdata_agent_storage::query::StorageQuery;
use netdata_agent_storage::storage_number::SN_FLAG_RESET;
use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_web::progress::Tracker;

use crate::finalize::{cardinality_limit, percentage_of_total};
use crate::groupby::{AddMode, add_metric, finalize, initialize};
use crate::grouping::{Grouping, Windows};
use crate::plan::{self, PlanEntry, PlanState, TierView, Tiers};
use crate::rrdr::{Rrdr, result_flags, value_flags};
use crate::tables::{TimeGrouping, options};
use crate::target::{QueryMetric, QueryTarget, metric_status, status};
use crate::window::Window;

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
#[derive(Debug)]
enum Prepared {
    /// Serve the collector's last stored value (`query_latest_fast_path()`).
    Latest { value: f64, time_s: i64 },
    /// Read the plans (`qm->plan`), each over its window read past its ends (`ops->plans[p].expanded_after/before`).
    Plans {
        entries: Vec<PlanEntry>,
        expanded: Vec<(i64, i64)>,
    },
}

/// `QUERY_ENGINE_OPS`: one metric's execution state, with its plans' queries.
struct Ops<'h> {
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
    db_total_points_read: usize,
    db_points_read_per_tier: [usize; RRD_STORAGE_TIERS],
    /// `ops->tier`: the active plan's tier.
    tier: usize,
    current_plan: usize,
    entries: Vec<PlanEntry>,
    views: [TierView; RRD_STORAGE_TIERS],
    storage_tiers: usize,
    states: Vec<PlanState>,
    /// `ops->plans[p].handle`: a plan's query until the plan is finalized.
    queries: Vec<Option<StorageQuery<'h>>>,
    /// `qt->window.before`: where the last plan runs to.
    window_before: i64,
}

impl<'h> Ops<'h> {
    /// Every plan is open with its query (`query_planer_initialize_plans()`); none is active yet.
    fn new(
        qt: &QueryTarget,
        window: &Window,
        views: [TierView; RRD_STORAGE_TIERS],
        entries: Vec<PlanEntry>,
        queries: Vec<Option<StorageQuery<'h>>>,
    ) -> Self {
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
            db_total_points_read: 0,
            db_points_read_per_tier: [0; RRD_STORAGE_TIERS],
            tier: 0,
            current_plan: 0,
            entries,
            views,
            storage_tiers: qt.request.profile.storage_tiers as usize,
            states: vec![PlanState::Open; queries.len()],
            queries,
            window_before: window.before,
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

    /// `query_plan_should_switch_plan()`: a point ending at `now` crosses the active plan's expiry.
    fn should_switch(&self, now: i64) -> bool {
        now >= self.current_plan_expire_time
    }

    /// `query_result_plan_should_switch_plan()`: the row ending at `now` is past the expiry and the offset of the
    /// grouping; never when the sum overflowed.
    fn result_should_switch(&self, now: i64) -> bool {
        now >= self.result_plan_expire_time && !self.result_plan_expire_time_overflow
    }

    /// `query_planer_set_active_plan()`.
    fn set_active(&mut self, p: usize) {
        self.tier = self.entries[p].tier;
        self.current_plan = p;
        self.set_expire_time(plan::expire_time(&self.entries, p));
    }

    /// `query_planer_finalize_plan()`: an open plan's query is released.
    fn finalize(&mut self, p: usize) {
        if self.states[p] == PlanState::Open {
            self.queries[p] = None;
            self.states[p] = PlanState::Finalized;
        }
    }

    /// `query_planer_finalize_remaining_plans()`.
    fn finalize_remaining(&mut self) {
        for p in 0..self.entries.len() {
            self.finalize(p);
        }
    }

    /// `query_planer_next_plan()`: activates the first later plan that neither `now` nor the last point's end has
    /// reached, releasing the plan it leaves (skipped plans stay open until the end); otherwise the active plan runs
    /// to the window's end.
    fn next_plan(&mut self, now: i64, last_point_end: i64) -> bool {
        let tiers = Tiers {
            views: &self.views,
            storage_tiers: self.storage_tiers,
        };
        match plan::next_plan(
            &tiers,
            &self.entries,
            &self.states,
            self.current_plan,
            now,
            last_point_end,
        ) {
            Some(p) => {
                self.finalize(self.current_plan);
                self.set_active(p);
                true
            }
            None => {
                self.set_expire_time(self.window_before);
                false
            }
        }
    }

    /// `ops->seqh`: the active plan's query, open because only open plans are activated and the plan left is
    /// released after the switch.
    fn query(&mut self) -> &mut StorageQuery<'h> {
        self.queries[self.current_plan]
            .as_mut()
            .expect("the active plan is open")
    }

    /// A point read from the active plan.
    fn count_read(&mut self) {
        self.db_points_read_per_tier[self.tier] += 1;
        self.db_total_points_read += 1;
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

/// `rrd2rrdr_query_ops_prep()`: the LATEST fast path, else the plans (`query_plan()`), which the metric keeps with the
/// tiers' weights, even when planning fails, and `queries` counts; `None` fails the metric.
fn prepare(qt: &mut QueryTarget, d: usize, window: &Window) -> Option<Prepared> {
    let qm = &qt.query[d];
    let (db_last, db_ue) = (qm.tiers[0].last_time_s, qm.tiers[0].update_every_s);
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
    let views = qm.tier_views();
    let tiers = Tiers {
        views: &views,
        storage_tiers: qt.request.profile.storage_tiers as usize,
    };
    // a selected tier the agent runs; build_entries() checks the metric has it
    let selected = (window.options & options::SELECTED_TIER != 0
        && qt.request.tier < qt.request.profile.storage_tiers)
        .then_some(qt.request.tier as usize);
    let mut weights = [0; RRD_STORAGE_TIERS];
    let built = plan::build_entries(
        &tiers,
        selected,
        window.after,
        window.before,
        window.points as usize,
        &mut weights,
    );
    let qm = &mut qt.query[d];
    for (tier, weight) in qm.tiers.iter_mut().zip(weights) {
        tier.weight = weight;
    }
    qm.plan.clone_from(&built.entries);
    if !built.ok {
        return None;
    }
    let expanded = plan::expanded_windows(&tiers, &built.entries);
    for e in &built.entries {
        qt.db.tiers[e.tier].queries += 1;
    }
    // query_plan() activates plan 0 once every plan is open
    let open = [PlanState::Open; plan::QUERY_PLANS_MAX];
    if !plan::can_activate(&tiers, &built.entries, &open[..built.entries.len()], 0) {
        return None;
    }
    Some(Prepared::Plans {
        entries: built.entries,
        expanded,
    })
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
        start_time_s: time_s - qm.tiers[0].update_every_s,
        end_time_s: time_s,
        count: 1,
        anomaly_count: 0,
        flags: 0,
    }
}

/// `rrd2rrdr_query_execute()` over the plans from the active one: fills column `col` and returns the metric's merged
/// points; the reads per tier are left in `ops.db_points_read_per_tier`.
fn execute_plan(
    r: &mut Rrdr,
    col: usize,
    grouping: &mut Grouping,
    qm: &QueryMetric,
    window: &Window,
    ops: &mut Ops<'_>,
) -> StoragePoint {
    let opts = window.options;
    let use_anomaly_bit_as_value = opts & options::ANOMALY_BIT != 0;
    let points_wanted = r.n;
    let vue = ops.view_update_every;
    let mut points_added = 0;
    let (mut min, mut max) = (r.view.min, r.view.max);
    let (mut last2, mut last1, mut new) = (EMPTY_POINT, EMPTY_POINT, EMPTY_POINT);
    // At a plan switch, the new plan's first point, read ahead to join the plans where it starts.
    let mut next1 = StoragePoint::UNSET;
    let mut next1_tier = 0;
    let mut now_start = window.after - ops.query_granularity;
    let mut now_end = window.after + (vue - ops.query_granularity);
    let mut read_since_plan_switch = 0usize;
    let mut finished_counter = 0;

    while points_added < points_wanted && finished_counter <= 10 {
        if ops.result_should_switch(now_end) {
            ops.next_plan(now_end - ops.plan_switch_time_offset, new.sp.end_time_s);
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
            let mut sp;
            let mut sp_tier;
            if next1.is_unset() {
                if ops.query().is_finished() {
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
                sp_tier = ops.tier;
                sp = ops.query().next_metric();
                ops.count_read();
                if opts & options::ABSOLUTE != 0 {
                    sp.make_positive();
                }
            } else {
                // The point read ahead is served without asking whether its query finished.
                finished_counter = 0;
                sp = next1;
                sp_tier = next1_tier;
                next1 = StoragePoint::UNSET;
                read_since_plan_switch = 1;
            }
            let mut prepared_sum = sp.sum;

            // The point crosses the plan's end and the next plan took over: its first point decides which serves.
            if ops.should_switch(sp.end_time_s)
                && ops.next_plan(now_end - ops.plan_switch_time_offset, new.sp.end_time_s)
            {
                let mut sp2 = ops.query().next_metric();
                let sp2_tier = ops.tier;
                ops.count_read();
                if opts & options::ABSOLUTE != 0 {
                    sp2.make_positive();
                }
                let finer_total_overlap = ops.point_mode == PointMode::Total
                    && sp2_tier < sp_tier
                    && sp2.start_time_s < sp.end_time_s;
                if sp.start_time_s > sp2.start_time_s
                    || (finer_total_overlap && sp2.start_time_s <= sp.start_time_s)
                {
                    // The old plan's point is wholly after the new one's, or a finer total covers it: dropped.
                    sp = sp2;
                    sp_tier = sp2_tier;
                    prepared_sum = sp2.sum;
                } else {
                    // The old plan's point serves up to where the new one starts, which ends its interpolation; a
                    // finer total keeps only the old point's share before it.
                    next1 = sp2;
                    next1_tier = sp2_tier;
                    if finer_total_overlap {
                        let duration = sp.end_time_s - sp.start_time_s;
                        let retained = sp2.start_time_s - sp.start_time_s;
                        if !qm.values_stored_as_rates && duration > 0 {
                            prepared_sum *= retained as f64 / duration as f64;
                        }
                        sp.end_time_s = sp2.start_time_s;
                    }
                }
            }
            new.sp = sp;
            new.tier = sp_tier;
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
                            let mut value = prepared_sum;
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

            // A zero-duration point from the engine is widened to one update interval of the active plan's tier.
            if read_since_plan_switch > 1 && new.sp.start_time_s == new.sp.end_time_s {
                new.sp.start_time_s = new.sp.end_time_s - qm.tiers[ops.tier].update_every_s;
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

        // Emit every row the three points in memory (last2, last1, new) cover, up to where a point read ahead starts.
        let mut stop_time = new.sp.end_time_s;
        if !next1.is_unset() && next1.start_time_s >= now_end {
            stop_time = next1.start_time_s;
        }
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
                || (!new.added && ops.point_mode == PointMode::Total && next1.is_unset()))
        {
            let settle =
                ops.point_mode == PointMode::Total || new.sp.end_time_s >= qm.tiers[0].last_time_s;
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
    ops.finalize_remaining();

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
    r.db_points_read += ops.db_total_points_read;
    ops.query_point
}

/// What a run needs from its caller: what stops it between metrics (the client went away, `timeout` ran out) and
/// the configured time-grouping limits.
pub struct Control<'a> {
    /// When the request arrived (`qt->timings.received_ut`).
    pub received: Instant,
    /// `web_client_interrupt_callback()`: it leaves the errno of its socket peek behind, as C's `recv()` does.
    pub interrupted: &'a dyn Fn(&mut i32) -> bool,
    /// The configured SES/DES window limits.
    pub windows: Windows,
    /// The pulse counters and the query's source (`qt->request.query_source`), when the query counts.
    pub pulse: Option<(&'a Queries, QuerySource)>,
    /// The request's progress row (`qt->request.transaction`), when the query has one.
    pub progress: Option<Tracker<'a>>,
}

impl Control<'_> {
    /// `query_progress_set_finish_line()`: the metrics the query will run.
    fn finish_line(&self, all: usize) {
        if let Some(progress) = &self.progress {
            progress.finish_line(all);
        }
    }

    /// `query_progress_done_step()`: a metric run without cancelling the query.
    fn step(&self) {
        if let Some(progress) = &self.progress {
            progress.step();
        }
    }

    /// `pulse_queries_rrdr_query_completed()` of a queried metric: one query, and the points `r` read and generated
    /// since `last`, which it then holds.
    fn metric_queried(&self, r: &Rrdr, last: &mut (u64, u64)) {
        let now = (r.db_points_read as u64, r.result_points_generated as u64);
        if let Some((queries, source)) = self.pulse {
            queries.rrdr_query_completed(1, now.0 - last.0, now.1 - last.1, source);
        }
        *last = now;
    }

    /// The two checks `rrd2rrdr()` makes after each queried metric; both can log in the same iteration. `errno` is
    /// what C's errno holds between iterations: the interrupt callback's peek leaves EAGAIN while the client is
    /// connected (ECONNRESET after a reset), and every record written clears it.
    fn cancel(&self, timeout_ms: i32, errno: &mut i32) -> bool {
        let mut cancel = false;
        if (self.interrupted)(errno) {
            nd_log!(Source::Access, Priority::Notice, errno = *errno; "QUERY INTERRUPTED");
            *errno = 0;
            cancel = true;
        }
        let elapsed_ms = self.received.elapsed().as_micros() as f64 / 1000.0;
        if timeout_ms != 0 && elapsed_ms > f64::from(timeout_ms) {
            nd_log!(Source::Access, Priority::Warning, errno = *errno;
                "QUERY CANCELED RUNTIME EXCEEDED {elapsed_ms:.2} ms (LIMIT {timeout_ms} ms)");
            *errno = 0;
            cancel = true;
        }
        cancel
    }
}

/// The indexes of a metric's dimension, instance, context and node.
fn links(qt: &QueryTarget, d: usize) -> (usize, usize, usize, usize) {
    let qd = qt.query[d].dimension;
    let qi = qt.dimensions[qd].instance;
    let qc = qt.instances[qi].context;
    (qd, qi, qc, qt.contexts[qc].node)
}

/// `r->time_grouping.create()` for the window.
fn new_grouping(qt: &QueryTarget, window: &Window, windows: Windows) -> Grouping {
    Grouping::new(
        qt.request.time_group,
        qt.request.time_group_options.as_deref(),
        window.group,
        window.points,
        window.resampling_group,
        window.resampling_divisor,
        windows,
    )
}

/// One metric of the `rrd2rrdr()` loop up to its execution: column `col` of `r` takes the metric's status, the
/// grouping is reset, and the metric is executed there (its merged points kept, its reads counted per tier), every
/// plan's query open from the start; a failed plan is counted and gives `None`.
fn query_metric(
    qt: &mut QueryTarget,
    d: usize,
    window: &Window,
    grouping: &mut Grouping,
    r: &mut Rrdr,
    col: usize,
) -> Option<StoragePoint> {
    let prepared = prepare(qt, d, window);
    r.od[col] = qt.query[d].status;
    grouping.reset();
    let (qd, qi, qc, qn) = links(qt, d);
    let Some(prepared) = prepared else {
        qt.instances[qi].metrics.failed += 1;
        qt.contexts[qc].metrics.failed += 1;
        qt.nodes[qn].metrics.failed += 1;
        qt.dimensions[qd].status |= status::FAILED;
        qt.query[d].status |= metric_status::FAILED;
        return None;
    };
    let qm = &qt.query[d];
    let query_points = match prepared {
        Prepared::Latest { value, time_s } => execute_latest(r, col, qm, window, value, time_s),
        Prepared::Plans { entries, expanded } => {
            // metric_dup() per plan: the queries borrow these, which leaves `qt` free for the counters
            let handles: Vec<TierHandle> = entries
                .iter()
                .map(|e| {
                    qm.tiers[e.tier]
                        .handle
                        .clone()
                        .expect("a valid plan's tier holds the metric")
                })
                .collect();
            let queries = handles
                .iter()
                .zip(&expanded)
                .map(|(h, &(after, before))| Some(h.query(after, before, qt.request.priority)))
                .collect();
            let mut ops = Ops::new(qt, window, qm.tier_views(), entries, queries);
            ops.set_active(0);
            let query_points = execute_plan(r, col, grouping, qm, window, &mut ops);
            let storage_tiers = qt.request.profile.storage_tiers as usize;
            for (stats, points) in qt
                .db
                .tiers
                .iter_mut()
                .zip(ops.db_points_read_per_tier)
                .take(storage_tiers)
            {
                stats.points += points;
            }
            query_points
        }
    };
    qt.query[d].query_points = query_points;
    r.od[col] |= metric_status::QUERIED;
    Some(query_points)
}

/// `rrd2rrdr()`'s node timing: when the loop moves to another node, the previous one gets the time since it
/// started; the last node never does.
struct NodeTimer {
    last: Instant,
    node: Option<usize>,
    node_started: Instant,
}

impl NodeTimer {
    fn new() -> Self {
        let now = Instant::now();
        NodeTimer {
            last: now,
            node: None,
            node_started: now,
        }
    }

    /// At the top of every metric, executed or not.
    fn enter(&mut self, qt: &mut QueryTarget, d: usize) {
        let (_, _, _, qn) = links(qt, d);
        if self.node != Some(qn) {
            if let Some(previous) = self.node {
                qt.nodes[previous].duration_ut = self
                    .last
                    .saturating_duration_since(self.node_started)
                    .as_micros() as u64;
            }
            self.node = Some(qn);
            self.node_started = self.last;
        }
    }

    /// After a metric executed: its duration is the time since the one executed before it (or the run's start).
    fn executed(&mut self, qt: &mut QueryTarget, d: usize) {
        let now = Instant::now();
        qt.query[d].duration_ut = now.saturating_duration_since(self.last).as_micros() as u64;
        self.last = now;
    }
}

/// The counters and statuses of a queried metric.
fn count_queried(qt: &mut QueryTarget, d: usize) {
    let (qd, qi, qc, qn) = links(qt, d);
    qt.instances[qi].metrics.queried += 1;
    qt.contexts[qc].metrics.queried += 1;
    qt.nodes[qn].metrics.queried += 1;
    qt.dimensions[qd].status |= status::QUERIED;
    qt.query[d].status |= metric_status::QUERIED;
}

fn time_flags(window: &Window) -> u32 {
    if window.relative {
        result_flags::RELATIVE
    } else {
        result_flags::ABSOLUTE
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
    r.view.flags |= time_flags(window);
    let mut grouping = new_grouping(qt, window, control.windows);
    control.finish_line(qt.query.len());
    let (mut used, mut nonzero) = (0, 0);
    let mut timer = NodeTimer::new();
    // C's errno as the loop leaves it
    let mut last_points = (0, 0);
    let mut errno = 0;
    for d in 0..qt.query.len() {
        timer.enter(qt, d);
        if query_metric(qt, d, window, &mut grouping, &mut r, d).is_none() {
            continue;
        }
        timer.executed(qt, d);
        count_queried(qt, d);
        control.metric_queried(&r, &mut last_points);
        if qt.query[d].status & metric_status::NONZERO != 0 {
            nonzero += 1;
        }
        used += 1;
        if control.cancel(qt.request.timeout_ms, &mut errno) {
            r.view.flags |= result_flags::CANCEL;
            break;
        }
        control.step();
    }
    percentage_of_total(&mut r, window.options);
    let r = cardinality_limit(r, qt.request.cardinality_limit);
    if used != 0 && window.options & options::NONZERO != 0 && nonzero == 0 {
        window.options &= !options::NONZERO;
    }
    qt.executed = Some(Instant::now());
    r
}

/// `rrd2rrdr()` for a v2 query: each metric executes into a one-column result and joins its group of the first
/// pass; finalize runs the later passes. `None` when there is nothing to group (C answers 500). A cancel is kept
/// only by a one-pass query, as in C.
pub fn run_v2(qt: &mut QueryTarget, window: &mut Window, control: &Control) -> Option<Rrdr> {
    let mut grouped = initialize(qt, window)?;
    let flags = time_flags(window);
    if let Some(last) = grouped.passes.last_mut() {
        last.view.flags |= flags;
    }
    let mut grouping = new_grouping(qt, window, control.windows);
    control.finish_line(qt.query.len());
    let (mut used, mut nonzero) = (0, 0);
    let mut timer = NodeTimer::new();
    // C's errno as the loop leaves it
    let mut last_points = (0, 0);
    let mut errno = 0;
    for d in 0..qt.query.len() {
        timer.enter(qt, d);
        let Some(query_points) = query_metric(qt, d, window, &mut grouping, &mut grouped.r_tmp, 0)
        else {
            continue;
        };
        timer.executed(qt, d);
        let r_tmp = &grouped.r_tmp;
        // The execution sets NONZERO on the column; v2 copies it back to the metric.
        qt.query[d].status = r_tmp.od[0];
        let r = &mut grouped.passes[0];
        r.view.min = r_tmp.view.min;
        r.view.max = r_tmp.view.max;
        r.view.after = r_tmp.view.after;
        r.view.before = r_tmp.view.before;
        r.rows = r_tmp.rows;
        let aggregation = qt.request.group_by[0].aggregation;
        add_metric(
            r,
            qt.query[d].grouped_as.first_slot,
            r_tmp,
            0,
            aggregation,
            &query_points,
            AddMode::default(),
        );
        count_queried(qt, d);
        control.metric_queried(&grouped.r_tmp, &mut last_points);
        // Aggregated across metrics from here: positive.
        let (_, qi, qc, qn) = links(qt, d);
        qt.query[d].query_points.make_positive();
        let points = qt.query[d].query_points;
        qt.instances[qi].query_points.merge_to(&points);
        qt.contexts[qc].query_points.merge_to(&points);
        qt.nodes[qn].query_points.merge_to(&points);
        qt.query_points.merge_to(&points);
        if qt.query[d].status & metric_status::NONZERO != 0 {
            nonzero += 1;
        }
        used += 1;
        if control.cancel(qt.request.timeout_ms, &mut errno) {
            grouped.passes[0].view.flags |= result_flags::CANCEL;
            break;
        }
        control.step();
    }
    let r = finalize(qt, window, grouped);
    let r = cardinality_limit(r, qt.request.cardinality_limit);
    if used != 0 && window.options & options::NONZERO != 0 && nonzero == 0 {
        window.options &= !options::NONZERO;
    }
    qt.executed = Some(Instant::now());
    Some(r)
}

#[cfg(test)]
mod tests;
