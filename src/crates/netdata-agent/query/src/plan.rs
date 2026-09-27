//! Query planning, ported from `src/web/api/queries/query-plan.c`: which tiers serve a metric's window and where each
//! takes over (decisions D74). A metric's tiers are seen through [`TierView`]s, so the planner needs no engine; the
//! executor (`execute.rs`) opens the plans and switches between them.

use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;

/// `QUERY_PLAN_MIN_POINTS`: the resolution the tier choice aims for at least.
pub const QUERY_PLAN_MIN_POINTS: usize = 10;
/// `POINTS_TO_EXPAND_QUERY`: the points a plan reads past its ends.
pub const POINTS_TO_EXPAND_QUERY: i64 = 5;
/// `QUERY_PLANS_MAX`.
pub const QUERY_PLANS_MAX: usize = RRD_STORAGE_TIERS;
/// `QUERY_PLAN_POINTS_WEIGHT_SCALE`.
const WEIGHT_SCALE: u64 = 1_000_000;

/// A tier of a metric as the planner sees it (`qm->tiers[t]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TierView {
    /// `smh`: the tier holds the metric.
    pub has_handle: bool,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub update_every_s: i64,
}

/// `QUERY_PLAN_ENTRY`: a tier serving `[after, before]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlanEntry {
    pub tier: usize,
    pub after: i64,
    pub before: i64,
}

/// A metric's tiers, and how many the agent runs (`nd_profile.storage_tiers`).
#[derive(Debug, Clone, Copy)]
pub struct Tiers<'a> {
    pub views: &'a [TierView],
    pub storage_tiers: usize,
}

impl Tiers<'_> {
    /// `query_metric_is_valid_tier()`.
    pub fn is_valid(&self, tier: usize) -> bool {
        self.views.get(tier).is_some_and(|t| {
            t.has_handle && t.first_time_s != 0 && t.last_time_s != 0 && t.update_every_s != 0
        })
    }

    /// `query_plan_tier_is_valid()`: a valid tier the agent runs.
    pub fn plan_tier_is_valid(&self, tier: usize) -> bool {
        tier < self.storage_tiers && tier < RRD_STORAGE_TIERS && self.is_valid(tier)
    }

    /// `query_plan_entry_is_valid()`: set, ordered, inside the wanted window, on a valid tier.
    pub fn entry_is_valid(&self, e: &PlanEntry, after_wanted: i64, before_wanted: i64) -> bool {
        e.after != 0
            && e.before != 0
            && e.after <= e.before
            && e.after >= after_wanted
            && e.before <= before_wanted
            && self.plan_tier_is_valid(e.tier)
    }

    /// `query_metric_first_working_tier()`: the lowest valid tier, else 0.
    pub fn first_working_tier(&self) -> usize {
        (0..self.storage_tiers)
            .find(|&t| self.is_valid(t))
            .unwrap_or(0)
    }

    /// `query_metric_tier_overlaps_timeframe()`.
    fn overlaps(&self, tier: usize, after_wanted: i64, before_wanted: i64) -> bool {
        self.is_valid(tier)
            && self.views[tier].first_time_s <= before_wanted
            && self.views[tier].last_time_s >= after_wanted
    }

    /// `query_metric_best_tier_for_timeframe()`: the coarsest tier that still gives half the points wanted (at least
    /// ten), else the densest one, among the tiers overlapping the window; `weights` get each tier's points density
    /// (`-LONG_MAX` for a tier that does not overlap), untouched when there is no choice to make.
    pub fn best_tier(
        &self,
        after_wanted: i64,
        before_wanted: i64,
        points_wanted: usize,
        weights: &mut [i64],
    ) -> usize {
        if self.storage_tiers < 2 {
            return 0;
        }
        if before_wanted <= after_wanted || points_wanted == 0 {
            return self.first_working_tier();
        }
        let duration = before_wanted - after_wanted;
        let points_wanted = if points_wanted < QUERY_PLAN_MIN_POINTS {
            if duration > QUERY_PLAN_MIN_POINTS as i64 {
                QUERY_PLAN_MIN_POINTS
            } else {
                duration as usize
            }
        } else {
            points_wanted
        };
        let minimum = minimum_acceptable_weight(points_wanted);
        let mut best: Option<(usize, i64, bool)> = None;
        for (tier, slot) in weights.iter_mut().enumerate().take(self.storage_tiers) {
            if !self.overlaps(tier, after_wanted, before_wanted) {
                *slot = -i64::MAX;
                continue;
            }
            let weight =
                density_weight(self.views[tier].update_every_s, after_wanted, before_wanted);
            *slot = weight;
            if weight == -i64::MAX {
                continue;
            }
            let acceptable = weight >= minimum;
            if best.is_none_or(|(b, bw, ba)| is_better(tier, weight, acceptable, b, bw, ba)) {
                best = Some((tier, weight, acceptable));
            }
        }
        best.map_or_else(|| self.first_working_tier(), |(tier, _, _)| tier)
    }
}

/// `query_plan_points_density_weight()`: the points the tier gives over the window, scaled by a million.
fn density_weight(update_every_s: i64, after_wanted: i64, before_wanted: i64) -> i64 {
    if update_every_s <= 0 || before_wanted <= after_wanted {
        return -i64::MAX;
    }
    let duration = (before_wanted - after_wanted) as u64;
    if duration > i64::MAX as u64 / WEIGHT_SCALE {
        return i64::MAX;
    }
    ((duration * WEIGHT_SCALE) / update_every_s as u64) as i64
}

/// `query_plan_minimum_acceptable_points_weight()`: half the points wanted, rounded up, scaled by a million.
fn minimum_acceptable_weight(points_wanted: usize) -> i64 {
    if points_wanted == 0 {
        return 0;
    }
    if points_wanted as u64 > i64::MAX as u64 / WEIGHT_SCALE {
        return i64::MAX;
    }
    let acceptable = (points_wanted as u64 * WEIGHT_SCALE).div_ceil(2);
    if acceptable > i64::MAX as u64 {
        i64::MAX
    } else {
        acceptable as i64
    }
}

/// `query_plan_points_density_is_better()`: an acceptable tier beats one that is not; among acceptable tiers the
/// fewer points win (ties to the higher tier), among the others the more points (ties to the lower tier).
fn is_better(
    tier: usize,
    weight: i64,
    acceptable: bool,
    best: usize,
    best_weight: i64,
    best_acceptable: bool,
) -> bool {
    if acceptable {
        return !best_acceptable || weight < best_weight || (weight == best_weight && tier > best);
    }
    !best_acceptable && (weight > best_weight || (weight == best_weight && tier < best))
}

/// What `query_plan_build_entries()` leaves in `qm->plan`, and whether planning succeeded (a failed plan is still
/// printed with the metric).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Built {
    pub entries: Vec<PlanEntry>,
    pub ok: bool,
}

/// `query_plan_build_entries()`: a selected tier the metric has serves the window alone; otherwise the best tier,
/// with coarser tiers before its data and finer tiers after it, sorted by start. `selected` is the requested tier
/// when `RRDR_OPTION_SELECTED_TIER` is set and the tier runs.
pub fn build_entries(
    tiers: &Tiers<'_>,
    selected: Option<usize>,
    after_wanted: i64,
    before_wanted: i64,
    points_wanted: usize,
    weights: &mut [i64],
) -> Built {
    let fail = |entries: Vec<PlanEntry>| Built { entries, ok: false };
    let (tier, switch_tiers) = match selected.filter(|&t| tiers.is_valid(t)) {
        Some(t) => (t, false),
        None => {
            let t = tiers.best_tier(after_wanted, before_wanted, points_wanted, weights);
            if !tiers.is_valid(t) {
                return fail(Vec::new());
            }
            (t, true)
        }
    };
    let v = tiers.views[tier];
    if v.first_time_s > before_wanted || v.last_time_s < after_wanted {
        return fail(Vec::new());
    }
    let mut entries = vec![PlanEntry {
        tier,
        after: v.first_time_s.max(after_wanted),
        before: v.last_time_s.min(before_wanted),
    }];
    if switch_tiers {
        let (mut first_s, mut last_s) = (entries[0].after, entries[0].before);
        // coarser tiers can start the query
        if first_s > after_wanted {
            for tr in tier + 1..tiers.storage_tiers {
                if entries.len() >= QUERY_PLANS_MAX {
                    break;
                }
                if !tiers.is_valid(tr) {
                    continue;
                }
                let t = tiers.views[tr];
                if t.first_time_s < first_s
                    && t.first_time_s <= before_wanted
                    && t.last_time_s >= after_wanted
                {
                    let e = PlanEntry {
                        tier: tr,
                        after: t.first_time_s.max(after_wanted),
                        before: first_s,
                    };
                    if !tiers.entry_is_valid(&e, after_wanted, before_wanted) {
                        return fail(entries);
                    }
                    entries.push(e);
                    first_s = e.after;
                    if e.after <= after_wanted {
                        break;
                    }
                }
            }
        }
        // finer tiers can finish it
        if last_s < before_wanted {
            for tr in (0..tier).rev() {
                if entries.len() >= QUERY_PLANS_MAX {
                    break;
                }
                if !tiers.is_valid(tr) {
                    continue;
                }
                let t = tiers.views[tr];
                if t.last_time_s > last_s
                    && t.first_time_s <= before_wanted
                    && t.last_time_s >= after_wanted
                {
                    let e = PlanEntry {
                        tier: tr,
                        after: last_s,
                        before: t.last_time_s.min(before_wanted),
                    };
                    if !tiers.entry_is_valid(&e, after_wanted, before_wanted) {
                        return fail(entries);
                    }
                    entries.push(e);
                    last_s = e.before;
                    if e.before >= before_wanted {
                        break;
                    }
                }
            }
        }
    }
    sort_as_the_reference(&mut entries);
    if !entries
        .iter()
        .all(|e| tiers.entry_is_valid(e, after_wanted, before_wanted))
    {
        return fail(entries);
    }
    Built { entries, ok: true }
}

/// `qsort()` with `compare_query_plan_entries_on_start_time()`, which never answers "equal": the reference's glibc
/// (2.41) leaves entries of equal start in reverse order of building, which this reproduces (D74.2).
fn sort_as_the_reference(entries: &mut [PlanEntry]) {
    let mut indexed: Vec<(usize, PlanEntry)> = entries.iter().copied().enumerate().collect();
    indexed.sort_by(|(ia, a), (ib, b)| a.after.cmp(&b.after).then(ib.cmp(ia)));
    for (slot, (_, e)) in entries.iter_mut().zip(indexed) {
        *slot = e;
    }
}

/// `query_planer_expand_duration_in_points()`: how far a plan reads into its neighbour's, in its own points.
pub fn expand_duration_in_points(this_update_every: i64, next_update_every: i64) -> i64 {
    let delta = (this_update_every - next_update_every).abs();
    if delta < this_update_every * POINTS_TO_EXPAND_QUERY {
        POINTS_TO_EXPAND_QUERY
    } else {
        (delta + this_update_every - 1) / this_update_every
    }
}

/// `query_planer_initialize_plans()`'s windows: each plan read past its ends by the points its neighbours need, the
/// first one from its start when it is tier 0.
pub fn expanded_windows(tiers: &Tiers<'_>, entries: &[PlanEntry]) -> Vec<(i64, i64)> {
    let ue = |e: &PlanEntry| tiers.views[e.tier].update_every_s;
    entries
        .iter()
        .enumerate()
        .map(|(p, e)| {
            let pad_start = if p > 0 {
                expand_duration_in_points(ue(e), ue(&entries[p - 1]))
            } else if e.tier == 0 {
                0
            } else {
                POINTS_TO_EXPAND_QUERY
            };
            let pad_end = match entries.get(p + 1) {
                Some(next) => expand_duration_in_points(ue(e), ue(next)),
                None => POINTS_TO_EXPAND_QUERY,
            };
            (e.after - ue(e) * pad_start, e.before + ue(e) * pad_end)
        })
        .collect()
}

/// A plan's query as `ops->plans[p]` tracks it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanState {
    #[default]
    Uninitialized,
    Open,
    Finalized,
}

/// `query_planer_plan_can_be_activated()`.
pub fn can_activate(
    tiers: &Tiers<'_>,
    entries: &[PlanEntry],
    states: &[PlanState],
    plan: usize,
) -> bool {
    let (Some(e), Some(&state)) = (entries.get(plan), states.get(plan)) else {
        return false;
    };
    state == PlanState::Open
        && e.after != 0
        && e.before != 0
        && e.after <= e.before
        && tiers.plan_tier_is_valid(e.tier)
}

/// `query_planer_next_plan()`'s choice: the first plan after `current` whose end neither `now` nor the last point's
/// end has reached; `None` past the last plan, or when that plan cannot be activated.
pub fn next_plan(
    tiers: &Tiers<'_>,
    entries: &[PlanEntry],
    states: &[PlanState],
    current: usize,
    now: i64,
    last_point_end: i64,
) -> Option<usize> {
    let mut p = current;
    loop {
        p += 1;
        let before = entries.get(p)?.before;
        if now < before && last_point_end < before {
            break;
        }
    }
    can_activate(tiers, entries, states, p).then_some(p)
}

/// `query_planer_set_active_plan()`'s expiry: where the next plan starts when it starts inside this one, else this
/// one's end.
pub fn expire_time(entries: &[PlanEntry], plan: usize) -> i64 {
    match entries.get(plan + 1) {
        Some(next) if next.after < entries[plan].before => next.after,
        _ => entries[plan].before,
    }
}

/// `query_target_min_update_every_for_tier()`: the smallest update every of the metrics on a tier, else the agent's.
pub fn min_update_every_for_tier(
    update_everies: impl Iterator<Item = i64>,
    tier: usize,
    storage_tiers: usize,
    profile_update_every: i64,
) -> i64 {
    if tier >= storage_tiers {
        return profile_update_every;
    }
    update_everies
        .filter(|&ue| ue != 0)
        .min()
        .unwrap_or(profile_update_every)
}

#[cfg(test)]
mod tests;
