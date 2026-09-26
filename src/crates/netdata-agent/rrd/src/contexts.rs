//! Contexts, ported from `src/database/contexts/` (rrdcontext): per host, contexts keyed by context id, their
//! instances keyed by chart id and the instances' metrics keyed by dimension id, each with a lifecycle (collected,
//! archived, deleted) and a retention that a worker recomputes. Queries and the context APIs read this tree.
//!
//! Charts and dimensions call the hooks (`rrdcontext_*` in C) when they are created, change or store; the hooks mark
//! what changed and queue the context for post-processing, which `Contexts::process_queued()` runs on the worker.
//!
//! Not here yet, each with the subsystem that brings it (decisions D19): the hub queue and the versions sent to
//! Netdata Cloud (claiming, ACLK), loading from SQL and UUID reuse (dbengine), garbage collection of deleted objects
//! (chart deletion) and the extreme cardinality protection (dbengine rotations).

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::chart::{Algorithm, Chart, ChartType, Dim, dim_flags, flags as chart_flags};
use crate::labels::Labels;

/// `RRD_FLAGS`.
pub mod flags {
    pub const DELETED: u32 = 1 << 0;
    pub const COLLECTED: u32 = 1 << 1;
    pub const UPDATED: u32 = 1 << 2;
    pub const ARCHIVED: u32 = 1 << 3;
    pub const OWN_LABELS: u32 = 1 << 4;
    pub const DEMAND_LABELS: u32 = 1 << 5;
    pub const LIVE_RETENTION: u32 = 1 << 6;
    pub const QUEUED_FOR_HUB: u32 = 1 << 7;
    pub const QUEUED_FOR_PP: u32 = 1 << 8;
    pub const HIDDEN: u32 = 1 << 9;
    pub const REASON_TRIGGERED: u32 = 1 << 10;
    pub const REASON_LOAD_SQL: u32 = 1 << 11;
    pub const REASON_NEW_OBJECT: u32 = 1 << 12;
    pub const REASON_UPDATED_OBJECT: u32 = 1 << 13;
    pub const REASON_CHANGED_LINKING: u32 = 1 << 14;
    pub const REASON_CHANGED_METADATA: u32 = 1 << 15;
    pub const REASON_ZERO_RETENTION: u32 = 1 << 16;
    pub const REASON_CHANGED_FIRST_TIME_T: u32 = 1 << 17;
    pub const REASON_CHANGED_LAST_TIME_T: u32 = 1 << 18;
    pub const REASON_STOPPED_BEING_COLLECTED: u32 = 1 << 19;
    pub const REASON_STARTED_BEING_COLLECTED: u32 = 1 << 20;
    pub const REASON_DISCONNECTED_CHILD: u32 = 1 << 21;
    pub const REASON_UNUSED: u32 = 1 << 22;
    pub const REASON_DB_ROTATION: u32 = 1 << 23;
    pub const NO_TIER0_RETENTION: u32 = 1 << 28;
    pub const MERGED_COLLECTED_RI_TO_RC: u32 = 1 << 29;
    /// An action: update the retention from the database.
    pub const REASON_UPDATE_RETENTION: u32 = 1 << 30;

    pub const ALL_UPDATE_REASONS: u32 = REASON_TRIGGERED
        | REASON_LOAD_SQL
        | REASON_NEW_OBJECT
        | REASON_UPDATED_OBJECT
        | REASON_CHANGED_LINKING
        | REASON_CHANGED_METADATA
        | REASON_ZERO_RETENTION
        | REASON_CHANGED_FIRST_TIME_T
        | REASON_CHANGED_LAST_TIME_T
        | REASON_STOPPED_BEING_COLLECTED
        | REASON_STARTED_BEING_COLLECTED
        | REASON_DISCONNECTED_CHILD
        | REASON_DB_ROTATION
        | REASON_UNUSED;
    pub const ALLOWED_EXTERNALLY_ON_NEW_OBJECTS: u32 = ARCHIVED | HIDDEN | ALL_UPDATE_REASONS;
    pub const REQUIRED_FOR_DELETIONS: u32 = DELETED | LIVE_RETENTION;
    pub const PREVENTING_DELETIONS: u32 = QUEUED_FOR_HUB | COLLECTED | QUEUED_FOR_PP;
}

/// `rrdcontext_reasons[]`: the update reasons in their listing order, with the delay before a hub dispatch.
pub const REASONS: [(u32, &str, u64); 15] = [
    (flags::REASON_TRIGGERED, "triggered transition", 65_000_000),
    (flags::REASON_NEW_OBJECT, "object created", 65_000_000),
    (flags::REASON_UPDATED_OBJECT, "object updated", 65_000_000),
    (flags::REASON_LOAD_SQL, "loaded from sql", 65_000_000),
    (
        flags::REASON_CHANGED_METADATA,
        "changed metadata",
        65_000_000,
    ),
    (flags::REASON_ZERO_RETENTION, "has no retention", 65_000_000),
    (
        flags::REASON_CHANGED_FIRST_TIME_T,
        "updated first_time_t",
        65_000_000,
    ),
    (
        flags::REASON_CHANGED_LAST_TIME_T,
        "updated last_time_t",
        65_000_000,
    ),
    (
        flags::REASON_STOPPED_BEING_COLLECTED,
        "stopped collected",
        65_000_000,
    ),
    (
        flags::REASON_STARTED_BEING_COLLECTED,
        "started collected",
        5_000_000,
    ),
    (flags::REASON_UNUSED, "unused", 5_000_000),
    (
        flags::REASON_CHANGED_LINKING,
        "changed rrd link",
        65_000_000,
    ),
    (
        flags::REASON_DISCONNECTED_CHILD,
        "child disconnected",
        65_000_000,
    ),
    (flags::REASON_DB_ROTATION, "db rotation", 65_000_000),
    (
        flags::REASON_UPDATE_RETENTION,
        "updated retention",
        65_000_000,
    ),
];

/// `RRDCONTEXT_MINIMUM_ALLOWED_PRIORITY`.
const MINIMUM_ALLOWED_PRIORITY: u32 = 10;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn now_usec() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

fn now_sec() -> u64 {
    now_usec() / 1_000_000
}

/// The atomic flags of a context, instance or metric, with C's transitions.
#[derive(Debug, Default)]
pub struct Flags(AtomicU32);

impl Flags {
    pub fn get(&self) -> u32 {
        self.0.load(Ordering::SeqCst)
    }

    /// `rrd_flag_check()`: any of the bits.
    pub fn check(&self, f: u32) -> bool {
        self.get() & f != 0
    }

    /// `rrd_flag_set()`: never for COLLECTED, ARCHIVED or DELETED.
    fn set(&self, f: u32) {
        self.0.fetch_or(f, Ordering::SeqCst);
    }

    fn clear(&self, f: u32) {
        self.0.fetch_and(!f, Ordering::SeqCst);
    }

    /// `rrd_flag_add_remove_atomic()`: removes `remove`; when `check` was not set, adds it with `add`. The old flags.
    fn add_remove(&self, check: u32, add: u32, remove: u32) -> u32 {
        self.0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                let mut new = old & !remove;
                if old & check == 0 {
                    new |= check | add;
                }
                Some(new)
            })
            .unwrap_or_else(|old| old)
    }

    /// `rrd_flags_replace_atomic()`.
    fn replace(&self, desired: u32) -> u32 {
        self.0.swap(desired, Ordering::SeqCst)
    }

    fn set_collected(&self) -> u32 {
        self.add_remove(
            flags::COLLECTED,
            flags::REASON_STARTED_BEING_COLLECTED | flags::UPDATED,
            flags::ARCHIVED
                | flags::DELETED
                | flags::REASON_STOPPED_BEING_COLLECTED
                | flags::REASON_ZERO_RETENTION
                | flags::REASON_DISCONNECTED_CHILD,
        )
    }

    fn set_archived(&self) -> u32 {
        self.add_remove(
            flags::ARCHIVED,
            flags::REASON_STOPPED_BEING_COLLECTED | flags::UPDATED,
            flags::COLLECTED
                | flags::DELETED
                | flags::REASON_STARTED_BEING_COLLECTED
                | flags::REASON_ZERO_RETENTION,
        )
    }

    fn set_deleted(&self, reason: u32) -> u32 {
        self.add_remove(
            flags::DELETED,
            flags::REASON_ZERO_RETENTION | flags::UPDATED | reason,
            flags::ARCHIVED | flags::COLLECTED,
        )
    }

    /// `rrd_flag_set_deleted_overwrite()`.
    fn set_deleted_overwrite(&self, replacement: u32) -> u32 {
        self.replace(
            (replacement & !(flags::COLLECTED | flags::ARCHIVED | flags::DELETED)) | flags::DELETED,
        )
    }

    pub fn is_collected(&self) -> bool {
        self.check(flags::COLLECTED)
    }

    pub fn is_archived(&self) -> bool {
        self.check(flags::ARCHIVED)
    }

    pub fn is_deleted(&self) -> bool {
        self.check(flags::DELETED)
    }

    fn is_updated(&self) -> bool {
        self.check(flags::UPDATED)
    }

    fn set_updated(&self, reason: u32) {
        self.set(flags::UPDATED | reason);
    }

    fn unset_updated(&self) {
        self.clear(flags::UPDATED | flags::ALL_UPDATE_REASONS);
    }
}

/// A dictionary of this tree: creation order plus an id index.
#[derive(Debug)]
struct Index<T> {
    ordered: Vec<Arc<T>>,
    by_id: HashMap<String, usize>,
}

impl<T> Default for Index<T> {
    fn default() -> Self {
        Index {
            ordered: Vec::new(),
            by_id: HashMap::new(),
        }
    }
}

impl<T> Index<T> {
    fn get(&self, id: &str) -> Option<Arc<T>> {
        self.by_id.get(id).map(|&i| Arc::clone(&self.ordered[i]))
    }

    fn insert(&mut self, id: &str, item: Arc<T>) {
        self.by_id.insert(id.to_string(), self.ordered.len());
        self.ordered.push(item);
    }
}

// ---- the post-processing queue (rrdcontext-queues.c) ----

/// `host->rrdctx.pp_queue`: contexts in the order they were queued; a queued context keeps its place.
#[derive(Debug, Default)]
struct PpQueue {
    inner: Mutex<PpEntries>,
}

#[derive(Debug, Default)]
struct PpEntries {
    by_idx: BTreeMap<u64, Arc<Context>>,
    next_idx: u64,
}

/// `rc->pp`: guarded by the queue lock.
#[derive(Debug, Default, Clone, Copy)]
pub struct PpState {
    idx: u64,
    pub queued_flags: u32,
    pub executions: usize,
    pub queued_ut: u64,
    pub dequeued_ut: u64,
}

impl PpQueue {
    /// `rrdcontext_add_to_pp_queue()`.
    fn add(&self, rc: &Arc<Context>) {
        let mut q = lock(&self.inner);
        let mut pp = lock(&rc.pp);
        let found = pp.idx != 0
            && q.by_idx
                .get(&pp.idx)
                .is_some_and(|queued| Arc::ptr_eq(queued, rc));
        rc.flags.set(flags::QUEUED_FOR_PP);
        if found {
            pp.queued_flags |= rc.flags.get();
        } else {
            q.next_idx += 1;
            pp.idx = q.next_idx;
            q.by_idx.insert(pp.idx, Arc::clone(rc));
            pp.queued_flags = rc.flags.get();
            pp.queued_ut = now_usec();
        }
    }

    /// `rrdcontext_del_from_pp_queue()` with the queue locked.
    fn del_locked(q: &mut PpEntries, rc: &Context) {
        let mut pp = lock(&rc.pp);
        if q.by_idx
            .get(&pp.idx)
            .is_some_and(|queued| std::ptr::eq(Arc::as_ptr(queued), rc))
        {
            q.by_idx.remove(&pp.idx);
            rc.flags.clear(flags::QUEUED_FOR_PP);
            pp.dequeued_ut = now_usec();
        }
        pp.idx = 0;
    }

    fn len(&self) -> usize {
        lock(&self.inner).by_idx.len()
    }
}

// ---- contexts ----

/// What was last sent to Netdata Cloud for a context (`VERSIONED_CONTEXT_DATA hub`); nothing until claiming exists.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Hub {
    pub version: u64,
    pub id: Option<String>,
    pub title: Option<Vec<u8>>,
    pub chart_type: Option<String>,
    pub units: Option<String>,
    pub family: Option<Vec<u8>>,
    pub priority: u64,
    pub first_time_s: u64,
    pub last_time_s: u64,
    pub deleted: bool,
}

/// The mutable part of a context.
#[derive(Debug, Clone)]
pub struct ContextState {
    pub version: u64,
    /// Bytes: merging two titles can split a multi-byte character, as in C.
    pub title: Vec<u8>,
    pub units: String,
    pub family: Vec<u8>,
    pub priority: u32,
    pub chart_type: ChartType,
    pub first_time_s: i64,
    pub last_time_s: i64,
    pub hub: Hub,
}

/// `RRDCONTEXT`.
#[derive(Debug)]
pub struct Context {
    id: String,
    pub flags: Flags,
    state: Mutex<ContextState>,
    instances: Mutex<Index<Instance>>,
    pp: Mutex<PpState>,
    queue: Weak<PpQueue>,
    /// The host's cached retention (`host->retention`).
    host_retention: Weak<Mutex<HostRetention>>,
    /// The RAM engine's metric index.
    ram_index: Weak<RamIndex>,
}

impl Context {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> ContextState {
        lock(&self.state).clone()
    }

    pub fn pp(&self) -> PpState {
        *lock(&self.pp)
    }

    pub fn instances(&self) -> Vec<Arc<Instance>> {
        lock(&self.instances).ordered.clone()
    }

    pub fn instance(&self, id: &str) -> Option<Arc<Instance>> {
        lock(&self.instances).get(id)
    }

    fn queue_for_post_processing(self: &Arc<Self>) {
        if let Some(queue) = self.queue.upgrade() {
            queue.add(self);
        }
    }

    /// `rrdcontext_trigger_updates()`.
    fn trigger_updates(self: &Arc<Self>) {
        if self.flags.is_updated() || !self.flags.check(flags::LIVE_RETENTION) {
            self.queue_for_post_processing();
        }
    }

    /// `rrdcontext_merge_with()`: an archived context takes a collected newcomer's metadata; a collected one keeps
    /// its own against an archived newcomer; otherwise titles and families merge.
    fn merge_with(&self, state: &mut ContextState, new: &Incoming) {
        let (was_archived, new_archived) = (self.flags.is_archived(), new.archived);
        if !was_archived && new_archived {
            return;
        }
        let use_new = was_archived && !new_archived;
        for (field, value) in [
            (&mut state.title, new.title),
            (&mut state.family, new.family),
        ] {
            if field.as_slice() != value {
                *field = if use_new {
                    value.to_vec()
                } else {
                    string_2way_merge(field, value)
                };
                self.flags.set_updated(flags::REASON_CHANGED_METADATA);
            }
        }
        if state.units != new.units {
            state.units = new.units.to_string();
            self.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
        if state.chart_type != new.chart_type {
            state.chart_type = new.chart_type;
            self.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
        if state.priority != new.priority {
            state.priority = new.priority;
            self.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
    }
}

/// The metadata `rrdcontext_merge_with()` merges into a context.
struct Incoming<'a> {
    archived: bool,
    title: &'a [u8],
    family: &'a [u8],
    units: &'a str,
    chart_type: ChartType,
    priority: u32,
}

/// `string_2way_merge()`: the common prefix, `[x]`, the common suffix of what remains; on bytes.
pub fn string_2way_merge(a: &[u8], b: &[u8]) -> Vec<u8> {
    const X: &[u8] = b"[x]";
    if a == b || a == X {
        return a.to_vec();
    }
    if b == X {
        return b.to_vec();
    }
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let mut out = a[..prefix].to_vec();
    if prefix < a.len() || prefix < b.len() {
        let (ra, rb) = (&a[prefix..], &b[prefix..]);
        let suffix = ra
            .iter()
            .rev()
            .zip(rb.iter().rev())
            .take_while(|(x, y)| x == y)
            .count();
        out.extend_from_slice(X);
        out.extend_from_slice(&ra[ra.len() - suffix..]);
    }
    out
}

/// The RAM engine's metric index (`rrddim_Judy_array` in `src/database/ram/rrddim_mem.c`): the rings of live ram and
/// alloc dimensions by UUID. C's index is process-wide; dimension UUIDs are random, so a host-wide one finds the same.
#[derive(Debug, Default)]
pub struct RamIndex {
    rings: Mutex<HashMap<[u8; 16], Weak<Dim>>>,
}

impl RamIndex {
    /// A dimension with a ring registers when it is created.
    pub(crate) fn register(&self, dim: &Dim) {
        lock(&self.rings).insert(*dim.uuid(), dim.weak());
    }

    /// `rrddim_metric_retention_by_id()`: the oldest and newest time of the live ring with this UUID.
    fn retention_by_id(&self, uuid: &[u8; 16]) -> Option<(i64, i64)> {
        let dim = lock(&self.rings).get(uuid).and_then(Weak::upgrade)?;
        Some((dim.first_entry_s(), dim.last_entry_s()))
    }
}

// ---- instances ----

/// The mutable part of an instance.
#[derive(Debug, Clone)]
pub struct InstanceState {
    pub uuid: [u8; 16],
    pub name: String,
    pub title: String,
    pub units: String,
    pub family: String,
    pub chart_type: ChartType,
    /// A 24-bit field in C.
    pub priority: u32,
    pub update_every_s: i32,
    pub first_time_s: i64,
    pub last_time_s: i64,
    /// `ri->rrdlabels` when not linked to a chart (`OWN_LABELS`).
    own_labels: Labels,
}

/// `RRDINSTANCE`.
#[derive(Debug)]
pub struct Instance {
    id: String,
    context: Weak<Context>,
    pub flags: Flags,
    state: Mutex<InstanceState>,
    /// `ri->rrdset`: the chart while it exists.
    chart: Mutex<Option<Weak<Chart>>>,
    metrics: Mutex<Index<Metric>>,
    /// `ri->internal.collected_metrics_count`: detects BEGIN/END without SET.
    collected_metrics_count: AtomicU32,
}

/// `(unsigned)st->priority`, and what the 24-bit `ri->priority` keeps of it.
fn chart_priority(chart: &Chart) -> (u32, u32) {
    let p = chart.meta().priority as i32 as u32;
    (p, p & 0x00FF_FFFF)
}

impl Instance {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> InstanceState {
        lock(&self.state).clone()
    }

    pub fn context(&self) -> Option<Arc<Context>> {
        self.context.upgrade()
    }

    pub fn chart(&self) -> Option<Arc<Chart>> {
        lock(&self.chart).as_ref().and_then(Weak::upgrade)
    }

    pub fn metrics(&self) -> Vec<Arc<Metric>> {
        lock(&self.metrics).ordered.clone()
    }

    pub fn metric(&self, id: &str) -> Option<Arc<Metric>> {
        lock(&self.metrics).get(id)
    }

    /// `rrdinstance_labels()`: the chart's labels while linked, else its own.
    pub fn labels(&self) -> Labels {
        match self.chart() {
            Some(chart) => chart.meta().labels,
            None => lock(&self.state).own_labels.clone(),
        }
    }

    /// `rrdinstance_trigger_updates()`.
    fn trigger_updates(&self) {
        if let Some(chart) = self.chart() {
            let (p, stored) = chart_priority(&chart);
            let update_every = chart.update_every();
            let mut state = lock(&self.state);
            if p != state.priority {
                state.priority = stored;
                self.flags.set_updated(flags::REASON_CHANGED_METADATA);
            }
            if update_every != state.update_every_s {
                state.update_every_s = update_every;
                self.flags.set_updated(flags::REASON_CHANGED_METADATA);
            }
        } else if self.flags.is_collected() {
            self.flags.set_archived();
            self.flags.set_updated(flags::REASON_CHANGED_LINKING);
        }
        if self.flags.is_updated() || !self.flags.check(flags::LIVE_RETENTION) {
            if let Some(rc) = self.context() {
                rc.flags.set_updated(flags::REASON_TRIGGERED);
                rc.queue_for_post_processing();
            }
        }
    }

    /// `rrdinstance_updated_rrdset_flags_no_action()`: the chart's hidden flag.
    fn sync_hidden(&self, chart: &Chart) {
        let chart_hidden = chart.meta().flags & chart_flags::HIDDEN != 0;
        let hidden = self.flags.check(flags::HIDDEN);
        if chart_hidden && !hidden {
            self.flags
                .set_updated(flags::HIDDEN | flags::REASON_CHANGED_METADATA);
        } else if !chart_hidden && hidden {
            self.flags.clear(flags::HIDDEN);
            self.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
    }
}

// ---- metrics ----

/// The mutable part of a metric.
#[derive(Debug, Clone)]
pub struct MetricState {
    pub uuid: [u8; 16],
    pub name: String,
    pub algorithm: Algorithm,
    pub first_time_s: i64,
    pub last_time_s: i64,
}

/// `RRDMETRIC`.
#[derive(Debug)]
pub struct Metric {
    id: String,
    instance: Weak<Instance>,
    pub flags: Flags,
    state: Mutex<MetricState>,
    /// `rm->rrddim`: the dimension while it exists.
    dim: Mutex<Option<Weak<Dim>>>,
}

impl Metric {
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn state(&self) -> MetricState {
        lock(&self.state).clone()
    }

    pub fn instance(&self) -> Option<Arc<Instance>> {
        self.instance.upgrade()
    }

    pub fn dim(&self) -> Option<Arc<Dim>> {
        lock(&self.dim).as_ref().and_then(Weak::upgrade)
    }

    /// The dimension whose storage holds this metric: the linked one, else the RAM engine's by UUID
    /// (`metric_get_by_id()`), as the query target admits it.
    pub fn storage_dim(&self) -> Option<Arc<Dim>> {
        if let Some(dim) = self.dim() {
            return Some(dim);
        }
        let uuid = lock(&self.state).uuid;
        let index = self.instance()?.context()?.ram_index.upgrade()?;
        let rings = lock(&index.rings);
        rings.get(&uuid).and_then(Weak::upgrade)
    }

    fn is_linked_to(&self, dim: &Dim) -> bool {
        self.dim()
            .is_some_and(|d| std::ptr::eq(Arc::as_ptr(&d), dim))
    }

    /// `rrdmetric_trigger_updates()`.
    fn trigger_updates(&self) {
        if self.flags.is_collected()
            && (self.dim().is_none() || self.flags.check(flags::REASON_DISCONNECTED_CHILD))
        {
            self.flags.set_archived();
        }
        if self.flags.is_updated() || !self.flags.check(flags::LIVE_RETENTION) {
            if let Some(ri) = self.instance() {
                ri.flags.set_updated(flags::REASON_TRIGGERED);
                if let Some(rc) = ri.context() {
                    rc.queue_for_post_processing();
                }
            }
        }
    }

    /// `rrdmetric_update_retention()`: a live dimension's rings, else the storage engine by UUID
    /// (`get_metric_retention_by_id()`, tier 0 only until dbengine).
    fn update_retention(&self) {
        let (mut first, mut last) = match self.dim() {
            Some(dim) => {
                self.flags.clear(flags::NO_TIER0_RETENTION);
                (dim.first_entry_s(), dim.last_entry_s())
            }
            None => {
                let uuid = lock(&self.state).uuid;
                let found = self
                    .instance()
                    .and_then(|ri| ri.context())
                    .and_then(|rc| rc.ram_index.upgrade())
                    .and_then(|index| index.retention_by_id(&uuid));
                let (first, last) = found.unwrap_or((0, 0));
                if first != 0 || last != 0 {
                    self.flags.clear(flags::NO_TIER0_RETENTION);
                } else {
                    self.flags.set(flags::NO_TIER0_RETENTION);
                }
                (if first > 0 { first } else { 0 }, last.max(0))
            }
        };
        if first > last {
            std::mem::swap(&mut first, &mut last);
        }
        let mut state = lock(&self.state);
        if first != state.first_time_s {
            state.first_time_s = first;
            self.flags.set_updated(flags::REASON_CHANGED_FIRST_TIME_T);
        }
        if last != state.last_time_s {
            state.last_time_s = last;
            self.flags.set_updated(flags::REASON_CHANGED_LAST_TIME_T);
        }
        if state.first_time_s == 0 && state.last_time_s == 0 {
            self.flags.set_deleted(flags::REASON_ZERO_RETENTION);
        }
        drop(state);
        self.flags.set(flags::LIVE_RETENTION);
    }

    /// `rrdmetric_should_be_deleted()`.
    fn should_be_deleted(&self) -> bool {
        if !self.flags.check(flags::REQUIRED_FOR_DELETIONS)
            || self.flags.check(flags::PREVENTING_DELETIONS)
            || self.dim().is_some()
        {
            return false;
        }
        self.update_retention();
        let state = lock(&self.state);
        state.first_time_s == 0 && state.last_time_s == 0
    }

    /// `rrdmetric_process_updates()`.
    fn process_updates(&self, force: bool, reason: u32) {
        if reason != 0 {
            self.flags.set_updated(reason);
        }
        if !force
            && !self.flags.is_updated()
            && self.flags.check(flags::LIVE_RETENTION)
            && !self.flags.check(flags::REASON_UPDATE_RETENTION)
        {
            return;
        }
        if reason & flags::REASON_DISCONNECTED_CHILD != 0 {
            self.flags.set_archived();
            self.flags.set(flags::REASON_DISCONNECTED_CHILD);
        }
        if self.flags.is_deleted() && reason & flags::REASON_UPDATE_RETENTION != 0 {
            self.flags.set_archived();
        }
        self.update_retention();
        self.flags.unset_updated();
    }
}

// ---- the per-chart and per-dimension links (st->rrdcontexts, rd->rrdcontexts) ----

/// `st->rrdcontexts`.
#[derive(Debug, Default)]
pub struct ChartLink {
    context: Mutex<Option<(Arc<Context>, Arc<Instance>)>>,
    /// `st->rrdcontexts.collected`.
    collected: AtomicBool,
}

impl ChartLink {
    pub fn instance(&self) -> Option<Arc<Instance>> {
        lock(&self.context).as_ref().map(|(_, ri)| Arc::clone(ri))
    }
}

/// `rd->rrdcontexts`.
#[derive(Debug, Default)]
pub struct DimLink {
    metric: Mutex<Option<Arc<Metric>>>,
    /// `rd->rrdcontexts.collected`.
    collected: AtomicBool,
}

impl DimLink {
    pub fn metric(&self) -> Option<Arc<Metric>> {
        lock(&self.metric).clone()
    }

    /// `rrdmetric_not_collected_rrddim()`.
    fn not_collected(&self) {
        self.collected.store(false, Ordering::Relaxed);
    }
}

/// `rrddim_get_rrdmetric()`: the dimension's metric, which must point back at it.
fn dim_metric(dim: &Dim) -> Option<Arc<Metric>> {
    let rm = dim.contexts().metric()?;
    debug_assert!(
        rm.is_linked_to(dim),
        "RRDMETRIC: '{}' is not linked to its RRDDIM",
        rm.id
    );
    Some(rm)
}

/// `rrdset_get_rrdinstance()`.
fn chart_instance(chart: &Chart) -> Option<Arc<Instance>> {
    chart.contexts().instance()
}

// ---- the hooks (rrdcontext.c public API) ----

/// `rrdmetric_from_rrddim()` (`rrdcontext_updated_rrddim()`): upserts the dimension's metric in its chart's instance.
pub fn updated_rrddim(chart: &Chart, dim: &Dim) {
    let Some(ri) = chart_instance(chart) else {
        return;
    };
    let meta = dim.meta();
    let mut metrics = lock(&ri.metrics);
    let rm = match metrics.get(dim.id()) {
        None => {
            let rm = Arc::new(Metric {
                id: dim.id().to_string(),
                instance: Arc::downgrade(&ri),
                flags: Flags::default(),
                state: Mutex::new(MetricState {
                    uuid: *dim.uuid(),
                    name: meta.name,
                    algorithm: meta.algorithm,
                    first_time_s: 0,
                    last_time_s: 0,
                }),
                dim: Mutex::new(Some(dim.weak())),
            });
            rm.flags.set_updated(flags::REASON_NEW_OBJECT);
            metrics.insert(dim.id(), Arc::clone(&rm));
            drop(metrics);
            rm.trigger_updates();
            rm
        }
        Some(rm) => {
            drop(metrics);
            // rrdmetric_conflict_callback()
            let mut state = lock(&rm.state);
            if state.uuid != *dim.uuid() {
                state.uuid = *dim.uuid();
                rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
            }
            {
                let mut link = lock(&rm.dim);
                let old = link.as_ref().and_then(Weak::upgrade);
                let is_dim = |old: &Arc<Dim>| std::ptr::eq(Arc::as_ptr(old), dim);
                if old.as_ref().is_some_and(|old| !is_dim(old)) {
                    rm.flags.set_updated(flags::REASON_CHANGED_LINKING);
                }
                if old.as_ref().is_none_or(|old| !is_dim(old)) {
                    *link = Some(dim.weak());
                }
            }
            if state.name != meta.name {
                state.name = meta.name;
                rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
            }
            if state.algorithm != meta.algorithm {
                state.algorithm = meta.algorithm;
                rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
            }
            // The newcomer has no retention: an unset first or last "widens" to zero, which still counts as a change.
            if state.first_time_s == 0 {
                rm.flags.set_updated(flags::REASON_CHANGED_FIRST_TIME_T);
            }
            if state.last_time_s == 0 {
                rm.flags.set_updated(flags::REASON_CHANGED_LAST_TIME_T);
            }
            drop(state);
            if rm.flags.is_collected() && rm.flags.is_archived() {
                rm.flags.set_collected();
            }
            let updated = rm.flags.is_updated();
            if updated {
                rm.flags.set(flags::REASON_UPDATED_OBJECT);
                rm.trigger_updates();
            }
            rm
        }
    };
    *lock(&dim.contexts().metric) = Some(rm);
    dim.contexts().not_collected();
}

/// `rrdmetric_updated_rrddim_flags()`: multiplier, divisor or obsolete changed.
pub fn updated_rrddim_flags(dim: &Dim) {
    dim.contexts().not_collected();
    let Some(rm) = dim_metric(dim) else {
        return;
    };
    if dim.meta().flags & dim_flags::OBSOLETE != 0 && rm.flags.is_collected() {
        rm.flags.set_archived();
    }
    rm.trigger_updates();
}

/// `rrdmetric_updated_rrddim_algorithm()`.
pub fn updated_rrddim_algorithm(dim: &Dim) {
    dim.contexts().not_collected();
    let Some(rm) = dim_metric(dim) else {
        return;
    };
    let algorithm = dim.meta().algorithm;
    {
        let mut state = lock(&rm.state);
        if state.algorithm != algorithm {
            state.algorithm = algorithm;
            rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
    }
    rm.trigger_updates();
}

/// `rrdmetric_collected_rrddim()`: the first store since the link was (re)established.
pub fn collected_rrddim(dim: &Dim) {
    if dim.contexts().collected.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(rm) = dim_metric(dim) else {
        return;
    };
    if !rm.flags.is_collected() {
        rm.flags.set_collected();
    }
    if let Some(ri) = rm.instance() {
        ri.collected_metrics_count.fetch_add(1, Ordering::Relaxed);
    }
    rm.trigger_updates();
}

/// `rrdinstance_from_rrdset()` (`rrdcontext_updated_rrdset()`): upserts the chart's context and instance and links
/// the chart to them; a chart that moved to another context leaves its old instance and metrics deleted.
pub fn updated_rrdset(chart: &Chart) {
    let contexts = chart.host_contexts();
    let meta = chart.meta();
    let (priority, stored_priority) = chart_priority(chart);
    let rc = contexts.upsert_context(&meta.context, |state_new| {
        state_new.title = meta.title.clone().into_bytes();
        state_new.units = meta.units.clone();
        state_new.family = meta.family.clone().into_bytes();
        state_new.priority = priority;
        state_new.chart_type = meta.chart_type;
    });
    let ri = {
        let mut instances = lock(&rc.instances);
        match instances.get(chart.id()) {
            None => {
                let hidden = meta.flags & chart_flags::HIDDEN != 0;
                let ri = Arc::new(Instance {
                    id: chart.id().to_string(),
                    context: Arc::downgrade(&rc),
                    flags: Flags(AtomicU32::new(if hidden { flags::HIDDEN } else { 0 })),
                    state: Mutex::new(InstanceState {
                        uuid: *chart.uuid(),
                        name: meta.name.clone().unwrap_or_else(|| chart.id().to_string()),
                        title: meta.title.clone(),
                        units: meta.units.clone(),
                        family: meta.family.clone(),
                        chart_type: meta.chart_type,
                        priority: stored_priority,
                        update_every_s: meta.update_every,
                        first_time_s: 0,
                        last_time_s: 0,
                        own_labels: Labels::default(),
                    }),
                    chart: Mutex::new(Some(chart.weak())),
                    metrics: Mutex::new(Index::default()),
                    collected_metrics_count: AtomicU32::new(0),
                });
                ri.flags.set_updated(flags::REASON_NEW_OBJECT);
                instances.insert(chart.id(), Arc::clone(&ri));
                drop(instances);
                ri.trigger_updates();
                ri
            }
            Some(ri) => {
                drop(instances);
                if instance_conflict(&ri, chart, &meta, stored_priority) {
                    ri.trigger_updates();
                }
                ri
            }
        }
    };
    let old = lock(&chart.contexts().context).replace((Arc::clone(&rc), Arc::clone(&ri)));
    if let Some((rc_old, ri_old)) = old {
        let context_changed = !Arc::ptr_eq(&rc_old, &rc);
        let instance_changed = !Arc::ptr_eq(&ri_old, &ri);
        if context_changed && instance_changed {
            for dim in chart.dims() {
                if let Some(rm_old) = dim.contexts().metric() {
                    rm_old.flags.set_deleted_overwrite(
                        flags::UPDATED
                            | flags::LIVE_RETENTION
                            | flags::REASON_UNUSED
                            | flags::REASON_ZERO_RETENTION,
                    );
                    *lock(&rm_old.dim) = None;
                    let mut state = lock(&rm_old.state);
                    state.first_time_s = 0;
                    state.last_time_s = 0;
                    drop(state);
                    *lock(&dim.contexts().metric) = None;
                    updated_rrddim(chart, &dim);
                }
            }
            if !ri_old.flags.check(flags::OWN_LABELS) {
                lock(&ri_old.state).own_labels = Labels::default();
            }
            ri_old.flags.set_deleted_overwrite(
                flags::UPDATED
                    | flags::OWN_LABELS
                    | flags::LIVE_RETENTION
                    | flags::REASON_UNUSED
                    | flags::REASON_ZERO_RETENTION,
            );
            *lock(&ri_old.chart) = None;
            let mut state = lock(&ri_old.state);
            state.first_time_s = 0;
            state.last_time_s = 0;
            drop(state);
            ri_old.trigger_updates();
        } else {
            assert!(
                !context_changed && !instance_changed,
                "RRDCONTEXT: cannot switch rrdcontext without switching rrdinstance too"
            );
        }
    }
}

/// `rrdinstance_conflict_callback()`: whether the instance was updated.
fn instance_conflict(
    ri: &Instance,
    chart: &Chart,
    meta: &crate::chart::ChartMeta,
    stored_priority: u32,
) -> bool {
    let mut guard = lock(&ri.state);
    let state = &mut *guard;
    let changed = |ri: &Instance| ri.flags.set_updated(flags::REASON_CHANGED_METADATA);
    if state.uuid != *chart.uuid() {
        state.uuid = *chart.uuid();
        changed(ri);
    }
    let name = meta.name.clone().unwrap_or_else(|| chart.id().to_string());
    for (field, new) in [
        (&mut state.name, &name),
        (&mut state.title, &meta.title),
        (&mut state.units, &meta.units),
        (&mut state.family, &meta.family),
    ] {
        if field != new {
            *field = new.clone();
            changed(ri);
        }
    }
    if state.chart_type != meta.chart_type {
        state.chart_type = meta.chart_type;
        changed(ri);
    }
    if state.priority != stored_priority {
        state.priority = stored_priority;
        changed(ri);
    }
    if state.update_every_s != meta.update_every {
        state.update_every_s = meta.update_every;
        changed(ri);
    }
    {
        let mut link = lock(&ri.chart);
        let linked = link.as_ref().and_then(Weak::upgrade);
        if linked.is_none_or(|c| !std::ptr::eq(Arc::as_ptr(&c), chart)) {
            *link = Some(chart.weak());
            ri.flags.set_updated(flags::REASON_CHANGED_LINKING);
        }
    }
    // Linked to a chart: its labels are the chart's.
    state.own_labels = Labels::default();
    if meta.flags & chart_flags::HIDDEN != 0 {
        ri.flags.set(flags::HIDDEN);
    } else {
        ri.flags.clear(flags::HIDDEN);
    }
    drop(guard);
    if ri.flags.is_collected() && ri.flags.is_archived() {
        ri.flags.set_collected();
    }
    if ri.flags.is_updated() {
        ri.flags.set(flags::REASON_UPDATED_OBJECT);
    }
    ri.flags.is_updated()
}

/// `rrdinstance_rrdset_not_collected()`: the next store and collection count again.
pub fn rrdset_not_collected(chart: &Chart) {
    chart.contexts().collected.store(false, Ordering::Relaxed);
    for dim in chart.dims() {
        dim.contexts().not_collected();
    }
}

/// `rrdinstance_updated_rrdset_name()`.
pub fn updated_rrdset_name(chart: &Chart) {
    rrdset_not_collected(chart);
    let Some(ri) = chart_instance(chart) else {
        return;
    };
    let name = chart.meta().name.unwrap_or_else(|| chart.id().to_string());
    let mut state = lock(&ri.state);
    if state.name != name {
        state.name = name;
        drop(state);
        ri.flags.set_updated(flags::REASON_CHANGED_METADATA);
        ri.trigger_updates();
    }
}

/// `rrdinstance_updated_rrdset_flags()`: obsolete or hidden changed.
pub fn updated_rrdset_flags(chart: &Chart) {
    rrdset_not_collected(chart);
    let Some(ri) = chart_instance(chart) else {
        return;
    };
    if chart.meta().flags & chart_flags::OBSOLETE != 0 {
        ri.flags.set_archived();
    }
    ri.sync_hidden(chart);
    ri.trigger_updates();
}

/// `rrdinstance_rrdset_has_updated_retention()`: replication filled in the past.
pub fn updated_retention_rrdset(chart: &Chart) {
    let Some(ri) = chart_instance(chart) else {
        return;
    };
    ri.flags.set_updated(flags::REASON_UPDATE_RETENTION);
    ri.trigger_updates();
}

/// `rrdinstance_collected_rrdset()`: the first completed collection since the link was (re)established; the
/// instance becomes collected only if a metric stored in between.
pub fn collected_rrdset(chart: &Chart) {
    if chart.contexts().collected.swap(true, Ordering::Relaxed) {
        return;
    }
    let ri = match chart_instance(chart) {
        Some(ri) => ri,
        None => {
            updated_rrdset(chart);
            match chart_instance(chart) {
                Some(ri) => ri,
                None => return,
            }
        }
    };
    ri.sync_hidden(chart);
    if ri.collected_metrics_count.load(Ordering::Relaxed) != 0 && !ri.flags.is_collected() {
        ri.flags.set_collected();
    }
    ri.collected_metrics_count.store(0, Ordering::Relaxed);
    ri.trigger_updates();
}

// ---- the per-host tree and the worker (rrdcontext-worker.c) ----

/// `host->retention`: first and last time over all contexts.
#[derive(Debug, Default)]
struct HostRetention {
    first_time_s: i64,
    last_time_s: i64,
    /// Each new first time while a child's receiver runs, for the stream path messages C sends from
    /// `rrdhost_update_cached_retention()` (`stream_path_retention_updated()`); `None` while nobody takes them.
    first_time_changes: Option<Vec<i64>>,
}

impl HostRetention {
    /// The end of `rrdhost_update_cached_retention()`: a changed first time owes the child a stream path.
    fn note_first_time(&mut self, old_first_time_s: i64) {
        if self.first_time_s != old_first_time_s {
            if let Some(changes) = &mut self.first_time_changes {
                changes.push(self.first_time_s);
            }
        }
    }
}

/// `host->rrdctx`: the host's contexts and their post-processing queue.
#[derive(Debug, Default)]
pub struct Contexts {
    index: Mutex<Index<Context>>,
    queue: Arc<PpQueue>,
    retention: Arc<Mutex<HostRetention>>,
    /// `RRDHOST_FLAG_RRDCONTEXT_GET_RETENTION`: a child disconnected.
    get_retention: AtomicBool,
    /// The RAM engine's metric index the unlinked metrics of this host look into.
    ram_index: Arc<RamIndex>,
    /// `dictionary_version(host->rrdctx.contexts)`: one per insert, delete and conflict that updated a context.
    version: AtomicU32,
}

impl Contexts {
    pub(crate) fn ram_index(&self) -> &RamIndex {
        &self.ram_index
    }

    pub fn all(&self) -> Vec<Arc<Context>> {
        lock(&self.index).ordered.clone()
    }

    pub fn get(&self, id: &str) -> Option<Arc<Context>> {
        lock(&self.index).get(id)
    }

    /// `host->retention` (first, last).
    pub fn retention(&self) -> (i64, i64) {
        let r = lock(&self.retention);
        (r.first_time_s, r.last_time_s)
    }

    /// Starts (a child's receiver runs) or stops recording the first-time changes a stream path is sent for.
    pub fn record_first_time_changes(&self, record: bool) {
        lock(&self.retention).first_time_changes = record.then(Vec::new);
    }

    /// The first times recorded since the last call, oldest first.
    pub fn take_first_time_changes(&self) -> Vec<i64> {
        lock(&self.retention)
            .first_time_changes
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Contexts waiting for post-processing.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// The contexts dictionary's insert, conflict and react callbacks for a collected chart's metadata.
    fn upsert_context(&self, id: &str, fill: impl FnOnce(&mut ContextState)) -> Arc<Context> {
        let mut new = ContextState {
            version: 0,
            title: Vec::new(),
            units: String::new(),
            family: Vec::new(),
            priority: 0,
            chart_type: ChartType::Line,
            first_time_s: 0,
            last_time_s: 0,
            hub: Hub::default(),
        };
        fill(&mut new);
        let mut index = lock(&self.index);
        match index.get(id) {
            None => {
                new.version = now_sec();
                let rc = Arc::new(Context {
                    id: id.to_string(),
                    flags: Flags::default(),
                    state: Mutex::new(new),
                    instances: Mutex::new(Index::default()),
                    pp: Mutex::new(PpState::default()),
                    queue: Arc::downgrade(&self.queue),
                    host_retention: Arc::downgrade(&self.retention),
                    ram_index: Arc::downgrade(&self.ram_index),
                });
                rc.flags.set_updated(flags::REASON_NEW_OBJECT);
                index.insert(id, Arc::clone(&rc));
                self.version.fetch_add(1, Ordering::Relaxed);
                drop(index);
                rc.trigger_updates();
                rc
            }
            Some(rc) => {
                drop(index);
                // rrdcontext_conflict_callback(): the newcomer comes from a collected chart, so it is not archived.
                let updated = {
                    let mut state = lock(&rc.state);
                    rc.merge_with(
                        &mut state,
                        &Incoming {
                            archived: false,
                            title: &new.title,
                            family: &new.family,
                            units: &new.units,
                            chart_type: new.chart_type,
                            priority: new.priority,
                        },
                    );
                    if rc.flags.is_collected() && rc.flags.is_archived() {
                        rc.flags.set_collected();
                    }
                    if rc.flags.is_updated() {
                        rc.flags.set(flags::REASON_UPDATED_OBJECT);
                    }
                    rc.flags.is_updated()
                };
                if updated {
                    self.version.fetch_add(1, Ordering::Relaxed);
                    rc.trigger_updates();
                }
                rc
            }
        }
    }

    /// `dictionary_version(host->rrdctx.contexts)`.
    pub fn version(&self) -> u32 {
        self.version.load(Ordering::Relaxed)
    }

    /// `rrdcontext_host_child_disconnected()`: the worker recomputes every retention on its next cycle.
    pub fn child_disconnected(&self) {
        self.get_retention.store(true, Ordering::Release);
    }

    /// One host's share of a worker cycle (`rrdcontext_main()`): the retention after a disconnect, then the queue.
    pub fn worker_cycle(&self) {
        if self.get_retention.load(Ordering::Acquire) {
            self.recalculate_host_retention(flags::REASON_DISCONNECTED_CHILD);
            self.get_retention.store(false, Ordering::Release);
        }
        self.process_queued();
    }

    /// `rrdcontext_post_process_queued_contexts()`: every queued context in queue order, including those queued
    /// while the pass runs.
    pub fn process_queued(&self) {
        let mut after = 0;
        loop {
            let rc = {
                let mut q = lock(&self.queue.inner);
                let Some((&idx, rc)) = q.by_idx.range(after + 1..).next() else {
                    break;
                };
                after = idx;
                let rc = Arc::clone(rc);
                PpQueue::del_locked(&mut q, &rc);
                rc
            };
            post_process_updates(&rc, false, 0);
        }
    }

    /// `rrdcontext_recalculate_host_retention()`: every context, forced; the host's retention is replaced.
    pub fn recalculate_host_retention(&self, reason: u32) {
        let (mut first, mut last) = (0, 0);
        for rc in self.all() {
            recalculate_context_retention(&rc, reason);
            let state = lock(&rc.state);
            if first == 0 || (state.first_time_s != 0 && state.first_time_s < first) {
                first = state.first_time_s;
            }
            if last == 0 || state.last_time_s > last {
                last = state.last_time_s;
            }
        }
        // rrdhost_update_cached_retention(global)
        let mut r = lock(&self.retention);
        let old_first = r.first_time_s;
        (r.first_time_s, r.last_time_s) = (first, last);
        r.note_first_time(old_first);
    }
}

/// `rrdcontext_recalculate_context_retention()`: a forced post-processing (repeated in C while the extreme
/// cardinality protection removes instances, which is not here yet).
pub fn recalculate_context_retention(rc: &Context, reason: u32) {
    post_process_updates(rc, true, reason);
}

/// `rrdhost_update_cached_retention()` (not global): widens the host's retention.
fn widen_host_retention(rc: &Context, first: i64, last: i64) {
    let Some(retention) = rc.host_retention.upgrade() else {
        return;
    };
    let mut r = lock(&retention);
    let old_first = r.first_time_s;
    if r.first_time_s == 0 || (first != 0 && first < r.first_time_s) {
        r.first_time_s = first;
    }
    if r.last_time_s == 0 || last > r.last_time_s {
        r.last_time_s = last;
    }
    r.note_first_time(old_first);
}

/// `check_if_cloud_version_changed_unsafe()`: whether what Netdata Cloud has seen differs.
fn cloud_version_changed(rc: &Context, state: &ContextState) -> bool {
    let f = rc.flags.get();
    let hub = &state.hub;
    let last = if f & flags::COLLECTED != 0 {
        0
    } else {
        state.last_time_s
    };
    let changed = hub.id.as_deref() != Some(rc.id.as_str())
        || hub.title.as_deref() != Some(state.title.as_slice())
        || hub.units.as_deref() != Some(state.units.as_str())
        || hub.family.as_deref() != Some(state.family.as_slice())
        || hub.chart_type.as_deref() != Some(state.chart_type.name())
        || u64::from(state.priority) != hub.priority
        || state.first_time_s as u64 != hub.first_time_s
        || last as u64 != hub.last_time_s
        || (f & flags::DELETED != 0) != hub.deleted;
    if changed || f & flags::COLLECTED == 0 {
        widen_host_retention(rc, state.first_time_s, state.last_time_s);
    }
    changed
}

/// `rrdinstance_post_process_updates()`.
fn instance_post_process_updates(ri: &Instance, force: bool, reason: u32) {
    if reason != 0 {
        ri.flags.set_updated(reason);
    }
    if !force && !ri.flags.is_updated() && ri.flags.check(flags::LIVE_RETENTION) {
        return;
    }
    let (mut min_first, mut max_last) = (i64::MAX, 0);
    let (mut active, mut no_tier0) = (0usize, 0usize);
    let (mut live_retention, mut currently_collected) = (true, false);
    for rm in ri.metrics() {
        let mut reason_to_pass = reason;
        if ri.flags.check(flags::REASON_UPDATE_RETENTION) {
            reason_to_pass |= flags::REASON_UPDATE_RETENTION;
        }
        rm.process_updates(force, reason_to_pass);
        if !rm.flags.check(flags::LIVE_RETENTION) {
            live_retention = false;
        }
        if rm.flags.check(flags::NO_TIER0_RETENTION) {
            no_tier0 += 1;
        }
        if rm.should_be_deleted() {
            continue;
        }
        let state = rm.state();
        if !currently_collected && rm.flags.is_collected() && state.first_time_s != 0 {
            currently_collected = true;
        }
        active += 1;
        if state.first_time_s != 0 && state.first_time_s < min_first {
            min_first = state.first_time_s;
        }
        if state.last_time_s != 0 && state.last_time_s > max_last {
            max_last = state.last_time_s;
        }
    }
    if no_tier0 != 0 && no_tier0 == active {
        ri.flags.set(flags::NO_TIER0_RETENTION);
    } else {
        ri.flags.clear(flags::NO_TIER0_RETENTION);
    }
    if live_retention {
        ri.flags.set(flags::LIVE_RETENTION);
    } else {
        ri.flags.clear(flags::LIVE_RETENTION);
    }
    let mut state = lock(&ri.state);
    let set_times = |state: &mut InstanceState, first: i64, last: i64| {
        if state.first_time_s != first {
            state.first_time_s = first;
            ri.flags.set_updated(flags::REASON_CHANGED_FIRST_TIME_T);
        }
        if state.last_time_s != last {
            state.last_time_s = last;
            ri.flags.set_updated(flags::REASON_CHANGED_LAST_TIME_T);
        }
    };
    if active == 0 {
        set_times(&mut state, 0, 0);
        ri.flags.set_deleted(flags::REASON_ZERO_RETENTION);
    } else {
        if min_first == i64::MAX {
            min_first = 0;
        }
        if min_first == 0 || max_last == 0 {
            set_times(&mut state, 0, 0);
            if live_retention {
                ri.flags.set_deleted(flags::REASON_ZERO_RETENTION);
            }
        } else {
            ri.flags.clear(flags::REASON_ZERO_RETENTION);
            set_times(&mut state, min_first, max_last);
            if currently_collected {
                ri.flags.set_collected();
            } else {
                ri.flags.set_archived();
            }
        }
    }
    drop(state);
    ri.flags.unset_updated();
}

/// `rrdinstance_should_be_deleted()`.
fn instance_should_be_deleted(ri: &Instance) -> bool {
    if !ri.flags.check(flags::REQUIRED_FOR_DELETIONS)
        || ri.flags.check(flags::PREVENTING_DELETIONS)
        || ri.chart().is_some()
        || !lock(&ri.metrics).ordered.is_empty()
    {
        return false;
    }
    let state = lock(&ri.state);
    state.first_time_s == 0 && state.last_time_s == 0
}

/// `rrdcontext_post_process_updates()`.
fn post_process_updates(rc: &Context, force: bool, reason: u32) {
    if reason != 0 {
        rc.flags.set_updated(reason);
    }
    let (mut min_priority_collected, mut min_priority_not_collected) = (u32::MAX, u32::MAX);
    let (mut min_first, mut max_last) = (i64::MAX, 0);
    let mut active = 0usize;
    let (mut live_retention, mut currently_collected, mut hidden) = (true, false, true);
    let instances = rc.instances();
    for ri in &instances {
        let mut reason_to_pass = reason;
        if rc.flags.check(flags::REASON_UPDATE_RETENTION) {
            reason_to_pass |= flags::REASON_UPDATE_RETENTION;
        }
        instance_post_process_updates(ri, force, reason_to_pass);
        if hidden && !ri.flags.check(flags::HIDDEN) {
            hidden = false;
        }
        if live_retention && !ri.flags.check(flags::LIVE_RETENTION) {
            live_retention = false;
        }
        if instance_should_be_deleted(ri) {
            continue;
        }
        let ri_state = ri.state();
        if ri.flags.is_collected() && !ri.flags.check(flags::MERGED_COLLECTED_RI_TO_RC) {
            // rrdcontext_update_from_collected_rrdinstance()
            let mut state = lock(&rc.state);
            rc.merge_with(
                &mut state,
                &Incoming {
                    archived: ri.flags.is_archived(),
                    title: ri_state.title.as_bytes(),
                    family: ri_state.family.as_bytes(),
                    units: &ri_state.units,
                    chart_type: ri_state.chart_type,
                    priority: ri_state.priority,
                },
            );
            drop(state);
            ri.flags.set(flags::MERGED_COLLECTED_RI_TO_RC);
        }
        if !currently_collected && ri.flags.is_collected() && ri_state.first_time_s != 0 {
            currently_collected = true;
        }
        active += 1;
        if ri_state.priority >= MINIMUM_ALLOWED_PRIORITY {
            let min = if ri.flags.is_collected() {
                &mut min_priority_collected
            } else {
                &mut min_priority_not_collected
            };
            *min = (*min).min(ri_state.priority);
        }
        if ri_state.first_time_s != 0 && ri_state.first_time_s < min_first {
            min_first = ri_state.first_time_s;
        }
        if ri_state.last_time_s != 0 && ri_state.last_time_s > max_last {
            max_last = ri_state.last_time_s;
        }
    }
    let min_priority = if instances.is_empty() {
        u32::MAX
    } else if min_priority_collected != u32::MAX {
        min_priority_collected
    } else {
        min_priority_not_collected
    };
    if hidden {
        rc.flags.set(flags::HIDDEN);
    } else {
        rc.flags.clear(flags::HIDDEN);
    }
    if live_retention {
        rc.flags.set(flags::LIVE_RETENTION);
    } else {
        rc.flags.clear(flags::LIVE_RETENTION);
    }
    let mut state = lock(&rc.state);
    lock(&rc.pp).executions += 1;
    let set_times = |state: &mut ContextState, first: i64, last: i64| {
        if state.first_time_s != first {
            state.first_time_s = first;
            rc.flags.set_updated(flags::REASON_CHANGED_FIRST_TIME_T);
        }
        if state.last_time_s != last {
            state.last_time_s = last;
            rc.flags.set_updated(flags::REASON_CHANGED_LAST_TIME_T);
        }
    };
    if active == 0 {
        set_times(&mut state, 0, 0);
        rc.flags.set_deleted(flags::REASON_ZERO_RETENTION);
    } else {
        if min_first == i64::MAX {
            min_first = 0;
        }
        if min_first == 0 && max_last == 0 {
            set_times(&mut state, 0, 0);
            rc.flags.set_deleted(flags::REASON_ZERO_RETENTION);
        } else {
            rc.flags.clear(flags::REASON_ZERO_RETENTION);
            set_times(&mut state, min_first, max_last);
            if currently_collected {
                rc.flags.set_collected();
            } else {
                rc.flags.set_archived();
            }
        }
        if min_priority != u32::MAX && state.priority != min_priority {
            state.priority = min_priority;
            rc.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
    }
    // The hub queue comes with claiming; until then only the version moves (rrdcontext_get_next_version()).
    if rc.flags.is_updated() && cloud_version_changed(rc, &state) {
        state.version = state.version.max(state.hub.version).max(now_sec()) + 1;
    }
    rc.flags.unset_updated();
}

#[cfg(test)]
mod tests;
