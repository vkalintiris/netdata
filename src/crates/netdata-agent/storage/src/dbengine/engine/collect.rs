//! A metric's collection into its tier (`rrdeng_store_metric_*()` in `rrdengineapi.c`): the hot page its collector
//! fills, sized so that the pages of a chart's metrics end together and those of different charts do not; closed when
//! full, on a gap the page cannot hold, on a step change, a new update every or finalize, then left dirty for the
//! flusher. Brief `knowledge/brief-dbengine-s3-commit2-map.md` in the status repository; decisions D65 and D66.

use std::sync::Arc;

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log_limit};
use twox_hash::XxHash3_64;

use super::cache::CachedPage;
use super::mrg::Handle;
use super::query::Dbengine;
use crate::dbengine::RRD_STORAGE_TIERS;
use crate::dbengine::format::descriptor::{point_size, uuid_text};
use crate::dbengine::format::page::{PageBuilder, gorilla};
use crate::storage_number::SN_EMPTY_SLOT;

const USEC_PER_SEC: u64 = 1_000_000;

/// `tier_page_size[]` on 64-bit hosts: the most bytes of points a tier's page holds.
pub const TIER_PAGE_SIZE: [usize; RRD_STORAGE_TIERS] = [4096, 2048, 384, 384, 384];

/// `nd_log_limit_static_global_var(erl, 1, 0)` of the conflict record.
static CONFLICT: ErrorLimit = ErrorLimit::new(1, 0);

/// What staggers a chart's pages in a tier: C hashes the address of the chart's `pg_alignment`, different on every
/// run; this port hashes what names the chart (D65.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Alignment(u64);

impl Alignment {
    /// XXH3 of the host's GUID, the chart's id and the tier, each name ended by a zero byte (D66 2.Q5).
    pub fn new(host_guid: &str, chart_id: &str, tier: usize) -> Alignment {
        let mut key = Vec::with_capacity(host_guid.len() + chart_id.len() + 3);
        key.extend_from_slice(host_guid.as_bytes());
        key.push(0);
        key.extend_from_slice(chart_id.as_bytes());
        key.push(0);
        key.push(tier as u8);
        Alignment(XxHash3_64::oneshot(&key))
    }

    #[cfg(test)]
    pub(crate) fn from_hash(hash: u64) -> Alignment {
        Alignment(hash)
    }

    /// `indexing_partition()`: the slot the chart's pages end at.
    fn target(self, max_slots: usize) -> usize {
        (self.0 % max_slots as u64) as usize
    }
}

/// `aligned_allocation_entries()`: the slots that make a page starting at `t_s` end at the target slot, a whole page
/// when it starts there.
fn aligned_allocation_entries(max_slots: usize, target: usize, t_s: i64) -> usize {
    let pos = (t_s as u64 % max_slots as u64) as usize;
    if pos > target {
        target + max_slots - pos
    } else if pos < target {
        target - pos
    } else {
        max_slots
    }
}

/// `rrdeng_alloc_new_page_data()`'s slots out of `max_slots`: aligned, but at least a third of a page and at least 3.
fn page_slots(max_slots: usize, alignment: Alignment, t_s: i64) -> usize {
    aligned_allocation_entries(max_slots, alignment.target(max_slots), t_s)
        .max(max_slots / 3)
        .max(3)
}

/// `struct rrdeng_collect_handle`: one collector of one metric in its tier.
#[derive(Debug)]
pub struct CollectHandle {
    engine: Arc<Dbengine>,
    metric: Handle,
    alignment: Alignment,
    /// The hot page and its data (`pgc_page`, `page_data`).
    page: Option<Arc<CachedPage>>,
    /// `RRDENG_PAGE_RETENTION_RECORDED`, `RRDENG_PAGE_CREATED_IN_FUTURE`: the page flags that change what happens.
    retention_recorded: bool,
    created_in_future: bool,
    entries_max: usize,
    position: usize,
    /// The last point's time; kept when the page closes.
    page_end_ut: u64,
    update_every_ut: u64,
    finished: bool,
}

impl CollectHandle {
    /// `rrdeng_store_metric_init()`: the metric's registry entry held, its update every set, its next point expected
    /// one update every after its retention.
    pub fn init(
        engine: &Arc<Dbengine>,
        metric: &Handle,
        update_every_s: u32,
        alignment: Alignment,
    ) -> CollectHandle {
        debug_assert!(update_every_s > 0, "collected every 0 seconds");
        let metric = metric.dup();
        engine.tiers[metric.tier()].collector_started();
        metric.set_update_every(update_every_s);
        // the retention, not the latest time: C's read stores the first time when it is unset
        let last_s = metric.retention().last_time_s;
        CollectHandle {
            engine: Arc::clone(engine),
            metric,
            alignment,
            page: None,
            retention_recorded: false,
            created_in_future: false,
            entries_max: 0,
            position: 0,
            page_end_ut: last_s.max(0) as u64 * USEC_PER_SEC,
            update_every_ut: u64::from(update_every_s) * USEC_PER_SEC,
            finished: false,
        }
    }

    pub fn metric(&self) -> &Handle {
        &self.metric
    }

    /// `rrdeng_store_metric_next()`: a point one update every after the last is appended; a later one closes the
    /// page when the step is off or the gap does not fit, else the gap is filled with empty points; an earlier or
    /// repeated one is dropped.
    #[allow(clippy::too_many_arguments)]
    pub fn store_next(
        &mut self,
        t_ut: u64,
        n: f64,
        min: f64,
        max: f64,
        count: u16,
        anomaly_count: u16,
        flags: u32,
    ) {
        let ue = self.update_every_ut;
        let delta = t_ut.wrapping_sub(self.page_end_ut);
        if delta != ue {
            if t_ut <= self.page_end_ut {
                return;
            }
            if self.page.is_some() {
                // too small, unaligned (an update every of 0, where C divides by it, counts as one), or a gap the
                // page cannot hold
                if delta < ue
                    || ue == 0
                    || !delta.is_multiple_of(ue)
                    || delta / ue >= (self.entries_max - self.position) as u64
                {
                    self.flush_current_page();
                } else {
                    let stop_ut = t_ut - ue;
                    let mut this_ut = self.page_end_ut + ue;
                    while this_ut <= stop_ut {
                        self.append_point(this_ut, f64::NAN, f64::NAN, f64::NAN, 1, 0, SN_EMPTY_SLOT);
                        this_ut = self.page_end_ut + ue;
                    }
                }
            }
        }
        self.append_point(t_ut, n, min, max, count, anomaly_count, flags);
    }

    /// `rrdeng_store_metric_append_point()`: the point goes into the page before its end moves, so queries that read
    /// the end find it.
    #[allow(clippy::too_many_arguments)]
    fn append_point(
        &mut self,
        t_ut: u64,
        n: f64,
        min: f64,
        max: f64,
        count: u16,
        anomaly_count: u16,
        flags: u32,
    ) {
        let t_s = (t_ut / USEC_PER_SEC) as i64;
        match &self.page {
            None => {
                let tier = self.metric.tier();
                let page_type = self.engine.tiers[tier].config.page_type;
                let max_slots = TIER_PAGE_SIZE[tier] / point_size(page_type).max(1);
                let slots = page_slots(max_slots, self.alignment, t_s);
                // C's fatal on an unknown page type: the configuration never gives one
                let Some(mut builder) = PageBuilder::new(page_type, slots) else {
                    debug_assert!(false, "page type {page_type}");
                    return;
                };
                builder.append(n, min, max, count, anomaly_count, flags);
                let (empty, capacity) = (builder.is_empty(), builder.capacity());
                self.create_new_page(t_ut, builder, capacity);
                self.first_retention(empty);
            }
            Some(page) => {
                let page = Arc::clone(page);
                let (grew, empty) = {
                    let mut builder = page.builder().expect("the collector's page");
                    let grew = builder.append(n, min, max, count, anomaly_count, flags);
                    (grew == Some(true), builder.is_empty())
                };
                page.hot_set_end_time_s(t_s);
                if grew {
                    self.engine.main.grew(&page, gorilla::BUFFER_SIZE);
                }
                self.page_end_ut = t_ut;
                self.first_retention(empty);
                self.position += 1;
                if self.position >= self.entries_max {
                    self.flush_current_page();
                }
            }
        }
        self.metric.set_hot_latest_time_s(t_s);
    }

    /// `rrdeng_store_metric_create_new_page()`: a page already cached at the start (the metric collected twice) is
    /// logged, and the new page tries one update every earlier, its first point kept.
    fn create_new_page(&mut self, mut t_ut: u64, builder: PageBuilder, capacity: usize) {
        let tier = self.metric.tier();
        let ue_s = (self.update_every_ut / USEC_PER_SEC) as u32;
        let mut page = CachedPage::collected((t_ut / USEC_PER_SEC) as i64, ue_s, builder);
        let added = loop {
            match self.engine.main.add(tier, self.metric.uuid(), page) {
                Ok(added) => break added,
                Err(conflict) => {
                    let existing = conflict.existing;
                    nd_log_limit!(
                        &CONFLICT,
                        Source::Daemon,
                        Priority::Warning,
                        "DBENGINE: metric '{}' new page from {} to {}, update every {}, has a conflict in main cache \
                         with existing {}{} page from {} to {}, update every {} - is it collected more than once?",
                        uuid_text(self.metric.uuid()),
                        conflict.page.start_time_s,
                        conflict.page.end_time_s(),
                        ue_s,
                        if existing.is_hot() { "hot" } else { "not-hot" },
                        if existing.is_gap() { " gap" } else { "" },
                        existing.start_time_s,
                        existing.end_time_s(),
                        existing.update_every_s()
                    );
                    drop(existing);
                    // an update every of 0 (unreachable) steps back a second instead of retrying forever
                    t_ut = t_ut.wrapping_sub(self.update_every_ut.max(USEC_PER_SEC));
                    page = *conflict.page;
                    page.restart_at((t_ut / USEC_PER_SEC) as i64);
                }
            }
        };
        self.entries_max = capacity;
        self.page_end_ut = t_ut;
        // the first point is already in the page
        self.position = 1;
        self.retention_recorded = false;
        // `max_acceptable_collected_time()`
        self.created_in_future = added.start_time_s > self.engine.now_s() + 1;
        self.page = Some(added);
        self.check_and_fix_mrg_update_every();
    }

    /// `rrdeng_store_metric_first_retention()`: the first page with data moves the metric's first time back, once.
    fn first_retention(&mut self, empty: bool) {
        if self.retention_recorded || self.created_in_future || empty {
            return;
        }
        if let Some(page) = &self.page {
            self.metric.expand_retention(page.start_time_s, 0, 0);
            self.retention_recorded = true;
        }
    }

    /// `rrdeng_store_metric_flush_current_page()`: a page without data leaves the cache; one with data sets the
    /// metric's clean time, counts its samples and turns dirty. The next point's expected time stays.
    pub fn flush_current_page(&mut self) {
        let Some(page) = self.page.take() else {
            return;
        };
        let tier = self.metric.tier();
        if page.is_empty() {
            self.engine.main.to_clean_evict_or_release(tier, page);
        } else {
            let (start_s, end_s) = (page.start_time_s, page.end_time_s());
            self.metric.set_clean_latest_time_s(end_s);
            let ue = i64::from(self.metric.update_every_s());
            if end_s != 0 && start_s != 0 && end_s > start_s && ue != 0 {
                self.engine.tiers[tier].add_samples(((end_s - start_s) / ue) as u64);
            }
            self.engine.main.hot_to_dirty(tier, &page);
        }
        // C's `mrg_metric_set_hot_latest_time_s(0)` here changes nothing
        self.retention_recorded = false;
        self.created_in_future = false;
        self.position = 0;
        self.entries_max = 0;
        self.check_and_fix_mrg_update_every();
    }

    /// `rrdeng_store_metric_change_collection_frequency()`: a new update every closes the page.
    pub fn change_collection_frequency(&mut self, update_every_s: u32) {
        debug_assert!(update_every_s > 0, "collected every 0 seconds");
        self.check_and_fix_mrg_update_every();
        let update_every_ut = u64::from(update_every_s) * USEC_PER_SEC;
        if update_every_ut == self.update_every_ut {
            return;
        }
        self.flush_current_page();
        self.metric.set_update_every(update_every_s);
        self.update_every_ut = update_every_ut;
    }

    /// `rrdeng_store_metric_finalize()`: the page closed and the collector gone; whether the metric has no retention
    /// (so its dimension may be deleted).
    pub fn finalize(mut self) -> bool {
        self.finish()
    }

    fn finish(&mut self) -> bool {
        self.finished = true;
        self.flush_current_page();
        self.engine.tiers[self.metric.tier()].collector_finished();
        let r = self.metric.retention();
        r.first_time_s == 0 && r.last_time_s == 0
    }

    /// `check_and_fix_mrg_update_every()`: the registry follows the handle, or a handle without one follows the
    /// registry.
    fn check_and_fix_mrg_update_every(&mut self) {
        let handle_ue_s = (self.update_every_ut / USEC_PER_SEC) as u32;
        let mrg_ue_s = self.metric.update_every_s();
        if handle_ue_s == mrg_ue_s {
            return;
        }
        if self.update_every_ut == 0 {
            self.update_every_ut = u64::from(mrg_ue_s) * USEC_PER_SEC;
        } else {
            self.metric.set_update_every(handle_ue_s);
        }
    }
}

/// A handle dropped without `finalize()` does its work, so no hot page or collector count is left behind (D66 2.Q6).
impl Drop for CollectHandle {
    fn drop(&mut self) {
        if !self.finished {
            self.finish();
        }
    }
}

#[cfg(test)]
mod tests;
