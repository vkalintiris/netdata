//! The metric registry (`src/database/engine/mrg.c`, `mrg-internals.h`, `mrg-load.c`): every metric the engine knows
//! per tier, with the oldest time on disk, the newest time on disk (clean) and collected (hot), and the latest update
//! every. One registry serves every tier, as C's sections do. Brief `knowledge/brief-dbengine-s2.md` §1.3, §1.5 and
//! §4.3 in the status repository.
//!
//! A metric lives while it is acquired or has retention: when the last holder releases one without retention
//! (`metric_release()`), it leaves the registry. The registry's own reference is its map entry; an acquired
//! [`Handle`] is another. C's writer counters exist only in its internal-checks builds, so they never keep a metric.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log, nd_log_limit};

use crate::dbengine::format::descriptor::ValidatedPage;
use crate::dbengine::format::journal_v2::{UeSource, expand};

/// `UUIDMAP_PARTITIONS`: the registry's lock partitions, by the UUID's last byte.
const PARTITIONS: usize = 32;

/// `set_metric_field_with_condition()`: the value is stored when `condition(current, wanted)` holds and it differs
/// from the current one; whether it was.
fn set_if<T: Copy + PartialEq>(
    field: &impl AtomicField<T>,
    wanted: T,
    condition: impl Fn(T, T) -> bool,
) -> bool {
    let mut current = field.load();
    loop {
        if !condition(current, wanted) || current == wanted {
            return false;
        }
        match field.compare_exchange(current, wanted) {
            Ok(()) => return true,
            Err(now) => current = now,
        }
    }
}

/// The atomics `set_if()` works on.
trait AtomicField<T> {
    fn load(&self) -> T;
    fn compare_exchange(&self, current: T, wanted: T) -> Result<(), T>;
}

impl AtomicField<i64> for AtomicI64 {
    fn load(&self) -> i64 {
        AtomicI64::load(self, Ordering::Relaxed)
    }

    fn compare_exchange(&self, current: i64, wanted: i64) -> Result<(), i64> {
        self.compare_exchange_weak(current, wanted, Ordering::Relaxed, Ordering::Relaxed)
            .map(drop)
    }
}

impl AtomicField<u32> for AtomicU32 {
    fn load(&self) -> u32 {
        AtomicU32::load(self, Ordering::Relaxed)
    }

    fn compare_exchange(&self, current: u32, wanted: u32) -> Result<(), u32> {
        self.compare_exchange_weak(current, wanted, Ordering::Relaxed, Ordering::Relaxed)
            .map(drop)
    }
}

/// `struct metric`.
#[derive(Debug)]
pub struct Metric {
    uuid: [u8; 16],
    tier: usize,
    /// The oldest point on disk.
    first_time_s: AtomicI64,
    /// The newest point on disk.
    latest_time_s_clean: AtomicI64,
    /// The newest point collected, not yet on disk.
    latest_time_s_hot: AtomicI64,
    latest_update_every_s: AtomicU32,
}

/// A metric's retention as `mrg_metric_get_retention()` reads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Retention {
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub update_every_s: u32,
}

impl Metric {
    fn new(
        uuid: [u8; 16],
        tier: usize,
        first_time_s: i64,
        last_time_s: i64,
        update_every_s: u32,
    ) -> Metric {
        Metric {
            uuid,
            tier,
            first_time_s: AtomicI64::new(first_time_s.max(0)),
            latest_time_s_clean: AtomicI64::new(last_time_s.max(0)),
            latest_time_s_hot: AtomicI64::new(0),
            latest_update_every_s: AtomicU32::new(update_every_s),
        }
    }

    pub fn uuid(&self) -> &[u8; 16] {
        &self.uuid
    }

    pub fn tier(&self) -> usize {
        self.tier
    }

    /// `mrg_metric_set_first_time_s()`: `LONG_MAX` means unset; a negative time is refused.
    pub fn set_first_time_s(&self, first_time_s: i64) -> bool {
        let first_time_s = if first_time_s == i64::MAX {
            0
        } else {
            first_time_s
        };
        if first_time_s < 0 {
            return false;
        }
        self.first_time_s.store(first_time_s, Ordering::Relaxed);
        true
    }

    /// `mrg_metric_set_first_time_s_if_bigger()`.
    pub fn set_first_time_s_if_bigger(&self, first_time_s: i64) -> bool {
        set_if(&self.first_time_s, first_time_s, |current, wanted| {
            wanted != 0 && wanted != i64::MAX && wanted > current
        })
    }

    /// `mrg_metric_get_first_time_s_smart()`: an unset first time is the newest clean time, else the hot one, and is
    /// stored as the first time when still unset.
    pub fn first_time_s(&self) -> i64 {
        let first = self.first_time_s.load(Ordering::Relaxed);
        if first > 0 {
            return first;
        }
        let mut first = self.latest_time_s_clean.load(Ordering::Relaxed);
        if first <= 0 {
            first = self.latest_time_s_hot.load(Ordering::Relaxed);
        }
        if first <= 0 {
            return 0;
        }
        if set_if(&self.first_time_s, first, |current, _| current <= 0) {
            first
        } else {
            self.first_time_s.load(Ordering::Relaxed)
        }
    }

    /// `mrg_metric_expand_retention()`: the first time moves earlier, the last later (with its update every); an
    /// update every without a last time is taken only when none is set.
    pub fn expand_retention(&self, first_time_s: i64, last_time_s: i64, update_every_s: u32) {
        if first_time_s > 0 && first_time_s != i64::MAX {
            set_if(&self.first_time_s, first_time_s, expand::first);
        }
        if last_time_s > 0 {
            if set_if(&self.latest_time_s_clean, last_time_s, expand::last) && update_every_s > 0 {
                set_if(&self.latest_update_every_s, update_every_s, |_, _| true);
            }
        } else if update_every_s > 0 {
            set_if(&self.latest_update_every_s, update_every_s, |current, _| {
                expand::update_every_unset(current)
            });
        }
    }

    /// `mrg_metric_get_retention()`.
    pub fn retention(&self) -> Retention {
        let clean = self.latest_time_s_clean.load(Ordering::Relaxed);
        let hot = self.latest_time_s_hot.load(Ordering::Relaxed);
        Retention {
            first_time_s: self.first_time_s(),
            last_time_s: clean.max(hot),
            update_every_s: self.latest_update_every_s.load(Ordering::Relaxed),
        }
    }

    /// `acquired_metric_has_retention()`.
    pub fn has_retention(&self) -> bool {
        let r = self.retention();
        r.first_time_s != 0 && r.last_time_s != 0 && r.first_time_s <= r.last_time_s
    }

    /// `mrg_metric_set_clean_latest_time_s()`: a later first time follows an earlier clean time.
    pub fn set_clean_latest_time_s(&self, latest_time_s: i64) -> bool {
        if latest_time_s > 0 && set_if(&self.latest_time_s_clean, latest_time_s, |_, _| true) {
            set_if(&self.first_time_s, latest_time_s, |current, wanted| {
                current <= 0 || wanted < current
            });
            return true;
        }
        false
    }

    /// `mrg_metric_set_hot_latest_time_s()`.
    pub fn set_hot_latest_time_s(&self, latest_time_s: i64) -> bool {
        if latest_time_s > 0 {
            self.latest_time_s_hot
                .store(latest_time_s, Ordering::Relaxed);
            return true;
        }
        false
    }

    /// `mrg_metric_get_latest_time_s()`: the newer of the clean and hot times.
    pub fn latest_time_s(&self) -> i64 {
        self.latest_time_s_clean
            .load(Ordering::Relaxed)
            .max(self.latest_time_s_hot.load(Ordering::Relaxed))
    }

    /// `mrg_metric_get_latest_clean_time_s()`.
    pub fn latest_clean_time_s(&self) -> i64 {
        self.latest_time_s_clean.load(Ordering::Relaxed)
    }

    /// `mrg_metric_set_update_every()`.
    pub fn set_update_every(&self, update_every_s: u32) -> bool {
        update_every_s > 0 && set_if(&self.latest_update_every_s, update_every_s, |_, _| true)
    }

    /// `mrg_metric_set_update_every_s_if_zero()`.
    pub fn set_update_every_s_if_zero(&self, update_every_s: u32) -> bool {
        update_every_s > 0
            && set_if(&self.latest_update_every_s, update_every_s, |current, _| {
                current == 0
            })
    }

    /// `mrg_metric_get_update_every_s()`.
    pub fn update_every_s(&self) -> u32 {
        self.latest_update_every_s.load(Ordering::Relaxed)
    }

    /// `mrg_metric_clear_retention()`.
    pub fn clear_retention(&self) {
        self.first_time_s.store(0, Ordering::Relaxed);
        self.latest_time_s_clean.store(0, Ordering::Relaxed);
        self.latest_time_s_hot.store(0, Ordering::Relaxed);
    }
}

type Partition = HashMap<([u8; 16], usize), Arc<Metric>>;

#[derive(Debug, Default)]
struct Inner {
    partitions: [Mutex<Partition>; PARTITIONS],
    /// `acquired_metrics`: what the pre-population keeps acquired until "mrg cleanup".
    prepopulated: Mutex<Vec<Arc<Metric>>>,
}

/// The registry (`MRG`); clones share it.
#[derive(Debug, Clone, Default)]
pub struct Mrg(Arc<Inner>);

/// An acquired metric; dropping it is `mrg_metric_release()`.
#[derive(Debug)]
pub struct Handle {
    mrg: Mrg,
    metric: Option<Arc<Metric>>,
}

impl std::ops::Deref for Handle {
    type Target = Metric;

    fn deref(&self) -> &Metric {
        self.metric
            .as_ref()
            .expect("a handle holds its metric until released")
    }
}

impl Handle {
    /// `mrg_metric_dup()`.
    pub fn dup(&self) -> Handle {
        Handle {
            mrg: self.mrg.clone(),
            metric: self.metric.clone(),
        }
    }

    /// `mrg_metric_release()`: whether the metric left the registry.
    pub fn release(mut self) -> bool {
        match self.metric.take() {
            Some(metric) => self.mrg.release(metric),
            None => false,
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        if let Some(metric) = self.metric.take() {
            self.mrg.release(metric);
        }
    }
}

/// `nd_log_limit_static_global_var(erl, 1, 0)` of each of the population's repairs.
static WRONG_LAST: ErrorLimit = ErrorLimit::new(1, 0);
static WRONG_FIRST: ErrorLimit = ErrorLimit::new(1, 0);
static ZERO_TIMES: ErrorLimit = ErrorLimit::new(1, 0);

/// `mrg_metric_clean_samples_from_snapshot()`: the samples between the first and the clean last time.
fn clean_samples(first_time_s: i64, latest_time_s_clean: i64, update_every_s: u32) -> u64 {
    if update_every_s == 0
        || first_time_s <= 0
        || latest_time_s_clean <= 0
        || first_time_s >= latest_time_s_clean
    {
        return 0;
    }
    (latest_time_s_clean - first_time_s) as u64 / u64::from(update_every_s)
}

impl Mrg {
    pub fn new() -> Mrg {
        Mrg::default()
    }

    fn partition(&self, uuid: &[u8; 16]) -> MutexGuard<'_, Partition> {
        self.0.partitions[usize::from(uuid[15]) & (PARTITIONS - 1)]
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    fn handle(&self, metric: Arc<Metric>) -> Handle {
        Handle {
            mrg: self.clone(),
            metric: Some(metric),
        }
    }

    /// `metric_release()`: the last holder of a metric without retention removes it. The holder's reference goes
    /// under the partition lock, so of two holders releasing at once the second sees itself last.
    fn release(&self, metric: Arc<Metric>) -> bool {
        let key = (metric.uuid, metric.tier);
        let mut partition = self.partition(&metric.uuid);
        let Some(held) = partition
            .get(&key)
            .filter(|held| Arc::ptr_eq(held, &metric))
        else {
            return false;
        };
        let held = Arc::clone(held);
        drop(metric);
        // the map's reference and `held`
        if Arc::strong_count(&held) == 2 && !held.has_retention() {
            partition.remove(&key);
            return true;
        }
        false
    }

    /// `mrg_metric_add_and_acquire()`: the metric of `uuid` on `tier`, added with these times unless it exists; and
    /// whether it was added.
    pub fn add_and_acquire(
        &self,
        uuid: &[u8; 16],
        tier: usize,
        first_time_s: i64,
        last_time_s: i64,
        update_every_s: u32,
    ) -> (Handle, bool) {
        let mut partition = self.partition(uuid);
        if let Some(metric) = partition.get(&(*uuid, tier)) {
            let metric = Arc::clone(metric);
            drop(partition);
            return (self.handle(metric), false);
        }
        let metric = Arc::new(Metric::new(
            *uuid,
            tier,
            first_time_s,
            last_time_s,
            update_every_s,
        ));
        partition.insert((*uuid, tier), Arc::clone(&metric));
        drop(partition);
        (self.handle(metric), true)
    }

    /// `mrg_metric_get_and_acquire_by_uuid()`.
    pub fn get_and_acquire(&self, uuid: &[u8; 16], tier: usize) -> Option<Handle> {
        let metric = self.partition(uuid).get(&(*uuid, tier)).cloned()?;
        Some(self.handle(metric))
    }

    /// `rrdeng_metric_retention_by_uuid()`: the metric's times on `tier`, `None` when the tier does not hold it. The
    /// lookup releases what it acquired, so a metric without retention that nobody else holds goes, as in C.
    pub fn retention_by_uuid(&self, uuid: &[u8; 16], tier: usize) -> Option<Retention> {
        self.get_and_acquire(uuid, tier).map(|h| h.retention())
    }

    /// The number of metrics held (`mrg_get_statistics().entries`).
    pub fn entries(&self) -> usize {
        self.0
            .partitions
            .iter()
            .map(|p| p.lock().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }

    /// `mrg_metric_prepopulate()`: a metric of the metadata database, kept acquired with no retention until
    /// [`Mrg::prepopulate_cleanup`], unless it is already known.
    pub fn prepopulate(&self, uuid: &[u8; 16], tier: usize) {
        let (mut handle, added) = self.add_and_acquire(uuid, tier, 0, 0, 0);
        if added {
            if let Some(metric) = handle.metric.take() {
                self.0
                    .prepopulated
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(metric);
            }
        }
    }

    /// `mrg_metric_prepopulate_cleanup()`, at the exit step "mrg cleanup" of the startup: the pre-populated metrics
    /// are released, and those that gained no retention leave the registry, with C's record.
    pub fn prepopulate_cleanup(&self) {
        let prepopulated = std::mem::take(
            &mut *self
                .0
                .prepopulated
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        let counter = prepopulated.len();
        let deleted = prepopulated
            .into_iter()
            .filter_map(|metric| self.release(metric).then_some(()))
            .count();
        if counter > 0 || deleted > 0 {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "MRG DUMP: Prepopulated {counter} metrics, released {}, deleted {deleted}",
                counter - deleted
            );
        }
    }

    /// `mrg_update_metric_retention_and_granularity_by_uuid()`: a metric of a journal v2 file, its times repaired
    /// against `now_s` with C's rate-limited records, added or expanded; the samples it adds to its journal's count.
    pub fn update_retention_by_uuid(
        &self,
        uuid: &[u8; 16],
        tier: usize,
        mut first_time_s: i64,
        mut last_time_s: i64,
        update_every_s: u32,
        now_s: i64,
    ) -> u64 {
        if last_time_s > now_s {
            nd_log_limit!(
                &WRONG_LAST,
                Source::Daemon,
                Priority::Warning,
                "DBENGINE JV2: wrong last time on-disk ({first_time_s} - {last_time_s}, now {now_s}), fixing last \
                 time to now"
            );
            last_time_s = now_s;
        }
        if first_time_s > last_time_s {
            nd_log_limit!(
                &WRONG_FIRST,
                Source::Daemon,
                Priority::Warning,
                "DBENGINE JV2: wrong first time on-disk ({first_time_s} - {last_time_s}, now {now_s}), fixing first \
                 time to last time"
            );
            first_time_s = last_time_s;
        }
        if first_time_s == 0 || last_time_s == 0 {
            nd_log_limit!(
                &ZERO_TIMES,
                Source::Daemon,
                Priority::Warning,
                "DBENGINE JV2: zero on-disk timestamps ({first_time_s} - {last_time_s}, now {now_s}), using them \
                 as-is"
            );
        }
        let (metric, added) = match self.get_and_acquire(uuid, tier) {
            Some(metric) => (metric, false),
            None => self.add_and_acquire(uuid, tier, first_time_s, last_time_s, update_every_s),
        };
        if added {
            return if update_every_s > 0 {
                (last_time_s - first_time_s) as u64 / u64::from(update_every_s)
            } else {
                0
            };
        }
        let samples = |m: &Metric| {
            if update_every_s == 0 {
                return 0;
            }
            clean_samples(
                m.first_time_s.load(Ordering::Relaxed),
                m.latest_time_s_clean.load(Ordering::Relaxed),
                m.latest_update_every_s.load(Ordering::Relaxed),
            )
        };
        let old = samples(&metric);
        metric.expand_retention(first_time_s, last_time_s, update_every_s);
        samples(&metric).saturating_sub(old)
    }

    /// A tier's view for the v1 replay (`journalfile_restore_extent_metadata()`).
    pub fn tier(&self, tier: usize) -> TierReplay<'_> {
        TierReplay { mrg: self, tier }
    }
}

/// What the v1 replay of one tier reads and updates in the registry.
pub struct TierReplay<'a> {
    mrg: &'a Mrg,
    tier: usize,
}

impl UeSource for TierReplay<'_> {
    fn update_every(&self, uuid: &[u8; 16]) -> u32 {
        self.mrg
            .get_and_acquire(uuid, self.tier)
            .map_or(0, |m| m.update_every_s())
    }

    fn replayed(&mut self, uuid: &[u8; 16], vd: &ValidatedPage) {
        let (metric, added) = self.mrg.add_and_acquire(
            uuid,
            self.tier,
            vd.start_time_s,
            vd.end_time_s,
            vd.update_every_s,
        );
        if !added {
            metric.expand_retention(vd.start_time_s, vd.end_time_s, vd.update_every_s);
        }
    }
}

#[cfg(test)]
mod tests;
