//! Charts and dimensions, ported from `src/database/rrdset-index-id.c`, `rrdset-index-name.c`, `rrdset-collection.c`
//! (`rrdset_set_update_every_s()`, `rrdset_finalize_collection()`) and `rrddim.c`: the per-host chart index with C's
//! insert/conflict rules and chart naming, and the dimensions with their storage on each tier: a ram ring, or a
//! dbengine registry entry with its collection handle (decisions D65, D68).
//!
//! One collector (a stream thread) changes a chart while queries read it: the mutable state sits behind locks that
//! are held only for short, bounded steps.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};

use netdata_agent_storage::dbengine::engine::collect::{Alignment, CollectHandle};
use netdata_agent_storage::dbengine::engine::mrg::Handle;
use netdata_agent_storage::query::{Priority, StorageQuery};
use netdata_agent_storage::ram::{ALLOC_MIN_ENTRIES, RamMetric, Seed};
use netdata_agent_text::sanitize::{rrd_string_sanitize, rrdset_strncpyz_name};

use crate::clock::now_realtime_s;
use crate::contexts::{self, ChartLink, Contexts, DimLink};
use crate::host::{meta_flags, pending_flags};
use crate::index::Index;
use crate::labels::{FLAG_DONT_DELETE, Labels, SRC_AUTO};
use crate::mode::{DbMode, align_entries_to_pagesize};
use crate::pulse;
use crate::storage::{Backfill, StorageLayout};
use crate::stream_control::BackfillRunning;
use crate::tiers::{self, Rollup, TierRecord};

/// `RRD_ID_LENGTH_MAX`.
pub const ID_LENGTH_MAX: usize = 1200;
/// `CONFIG_MAX_VALUE`: the bound of a sanitized chart name.
const NAME_LENGTH_MAX: usize = 2048;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn text(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

/// `rrd_string_strdupz()`.
fn rrd_string(v: &str) -> String {
    text(&rrd_string_sanitize(v.as_bytes()))
}

/// `snprintfz(dst, n, ...)`: at most `n - 1` bytes (the texts here are ASCII after the protocol's words).
fn bounded(mut s: String, n: usize) -> String {
    if s.len() >= n {
        let mut end = n - 1;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// `RRDSET_TYPE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartType {
    Line,
    Area,
    Stacked,
    Heatmap,
}

impl ChartType {
    /// `rrdset_type_id()`: unknown names are lines.
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"area" => ChartType::Area,
            b"stacked" => ChartType::Stacked,
            b"heatmap" => ChartType::Heatmap,
            _ => ChartType::Line,
        }
    }

    /// `RRDSET_TYPE`, the value SQL stores.
    pub fn id(self) -> i32 {
        match self {
            ChartType::Line => 0,
            ChartType::Area => 1,
            ChartType::Stacked => 2,
            ChartType::Heatmap => 3,
        }
    }

    /// `RRDSET_TYPE` as stored in SQL (`chart.chart_type`): 0 line, 1 area, 2 stacked, 3 heatmap; others are lines,
    /// as `rrdset_type_name()` names them.
    pub fn from_id(id: i32) -> Self {
        match id {
            1 => ChartType::Area,
            2 => ChartType::Stacked,
            3 => ChartType::Heatmap,
            _ => ChartType::Line,
        }
    }

    /// `rrdset_type_name()`.
    pub fn name(self) -> &'static str {
        match self {
            ChartType::Line => "line",
            ChartType::Area => "area",
            ChartType::Stacked => "stacked",
            ChartType::Heatmap => "heatmap",
        }
    }
}

/// `RRD_ALGORITHM`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Absolute,
    Incremental,
    PcentOverRowTotal,
    PcentOverDiffTotal,
}

impl Algorithm {
    /// `rrd_algorithm_id()`: unknown names are absolute.
    pub fn from_name(name: &[u8]) -> Self {
        match name {
            b"incremental" => Algorithm::Incremental,
            b"percentage-of-absolute-row" => Algorithm::PcentOverRowTotal,
            b"percentage-of-incremental-row" => Algorithm::PcentOverDiffTotal,
            _ => Algorithm::Absolute,
        }
    }

    /// `RRD_ALGORITHM`, the value SQL stores (not the declaration order of this enum).
    pub fn id(self) -> i32 {
        match self {
            Algorithm::Absolute => 0,
            Algorithm::Incremental => 1,
            Algorithm::PcentOverDiffTotal => 2,
            Algorithm::PcentOverRowTotal => 3,
        }
    }

    /// `RRD_ALGORITHM` as stored in SQL (`dimension.algorithm`): 0 absolute, 1 incremental, 2 percentage of the
    /// incremental row, 3 percentage of the absolute row; others are absolute, as `rrd_algorithm_name()` names them.
    pub fn from_id(id: i32) -> Self {
        match id {
            1 => Algorithm::Incremental,
            2 => Algorithm::PcentOverDiffTotal,
            3 => Algorithm::PcentOverRowTotal,
            _ => Algorithm::Absolute,
        }
    }

    /// `rrd_algorithm_name()`.
    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Absolute => "absolute",
            Algorithm::Incremental => "incremental",
            Algorithm::PcentOverRowTotal => "percentage-of-absolute-row",
            Algorithm::PcentOverDiffTotal => "percentage-of-incremental-row",
        }
    }
}

/// The chart flags this port tracks (`RRDSET_FLAG_*`).
pub mod flags {
    pub const SYNC_CLOCK: u32 = 1 << 0;
    pub const OBSOLETE: u32 = 1 << 1;
    pub const HIDDEN: u32 = 1 << 2;
    pub const STORE_FIRST: u32 = 1 << 3;
    pub const RECEIVER_REPLICATION_IN_PROGRESS: u32 = 1 << 4;
    pub const RECEIVER_REPLICATION_FINISHED: u32 = 1 << 5;
    pub const SENDER_REPLICATION_FINISHED: u32 = 1 << 6;
    pub const METADATA_UPDATE: u32 = 1 << 7;
    pub const HETEROGENEOUS: u32 = 1 << 8;
    pub const HOMOGENEOUS_CHECK: u32 = 1 << 9;
    /// `RRDSET_FLAG_BACKFILLED_HIGH_TIERS`: the chart's dimensions were queued for a backfill once; never cleared.
    pub const BACKFILLED_HIGH_TIERS: u32 = 1 << 10;
    /// `RRDSET_FLAG_OBSOLETE_DIMENSIONS`: some dimension turned obsolete, for the maintenance sweep.
    pub const OBSOLETE_DIMENSIONS: u32 = 1 << 11;

    /// `rrdset_is_replicating()`: a replication in progress and none finished. A chart starts with both finished bits
    /// and nothing clears the sender's without a stream sender, so on this agent no chart replicates in this sense.
    pub fn is_replicating(flags: u32) -> bool {
        flags & RECEIVER_REPLICATION_IN_PROGRESS != 0
            && flags & (SENDER_REPLICATION_FINISHED | RECEIVER_REPLICATION_FINISHED) == 0
    }

    /// `rrdset_is_discoverable()`: what the lookups find unless asked for obsolete charts too.
    pub fn is_discoverable(flags: u32) -> bool {
        is_replicating(flags) || flags & OBSOLETE == 0
    }
}

/// What `rrdset_create()` is called with.
#[derive(Debug, Clone)]
pub struct ChartSpec<'a> {
    pub type_: &'a str,
    pub id: &'a str,
    pub name: Option<&'a str>,
    pub family: Option<&'a str>,
    pub context: Option<&'a str>,
    pub title: &'a str,
    pub units: &'a str,
    pub plugin: &'a str,
    pub module: Option<&'a str>,
    pub priority: i64,
    pub update_every: i32,
    pub chart_type: ChartType,
    pub mode: DbMode,
    pub history_entries: i64,
    pub page_size: i64,
}

/// A chart's descriptive state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartMeta {
    pub name: Option<String>,
    pub family: String,
    pub context: String,
    pub title: String,
    pub units: String,
    pub plugin: String,
    pub module: String,
    pub priority: i64,
    pub update_every: i32,
    pub chart_type: ChartType,
    pub flags: u32,
    pub labels: Labels,
}

/// A chart's collection counters (`st->counter`, `st->db.current_entry`, the timestamps).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChartCollection {
    pub counter: usize,
    pub counter_done: usize,
    pub current_entry: usize,
    /// `st->last_collected_time` and `st->last_updated` as (seconds, microseconds).
    pub last_collected: (i64, i64),
    pub last_updated: (i64, i64),
    /// `st->usec_since_last_update`.
    pub usec_since_last_update: u64,
}

/// What the protocol parser keeps on a chart across connections (`st->pluginsd`, `st->replay`,
/// `st->replication_empty_response_count`).
#[derive(Debug, Default)]
pub struct ReceiverState {
    /// `dims_with_slots`: DIMENSION came with `SLOT:`; otherwise lookups cycle through `prd` by position.
    pub dims_with_slots: bool,
    /// `prd_array`: the dimension cache, by slot or by position.
    pub prd: Vec<Option<Arc<Dim>>>,
    /// `pos`: the next positional cache entry.
    pub pos: usize,
    /// `set`: a SET2/RSET happened since the positional cache was rewound.
    pub set: bool,
    /// The last replication request (`st->replay`).
    pub replay_after: i64,
    pub replay_before: i64,
    pub replay_start_streaming: bool,
    pub replication_empty_response_count: u32,
}

/// `RRDSET`.
#[derive(Debug)]
pub struct Chart {
    me: Weak<Chart>,
    /// `type.id`.
    id: String,
    /// `st->chart_uuid`.
    uuid: [u8; 16],
    /// The host's contexts (`st->rrdhost->rrdctx`).
    host_contexts: Arc<Contexts>,
    /// `st->rrdcontexts`.
    link: ChartLink,
    type_: String,
    id_part: String,
    mode: DbMode,
    /// `st->db.entries`.
    entries: usize,
    /// The host's storage (`st->rrdhost->db[]`).
    storage: Arc<StorageLayout>,
    /// `st->smg[tier]`: what staggers the chart's pages in each tier (D65.1), kept for the chart's life.
    alignment: Vec<Alignment>,
    /// `st->collection_modulo`: what spreads the tiers' writes of the chart's dimensions over time (D72.3).
    collection_modulo: u16,
    /// `st->parts.name`: the name `rrdset_create()` was first called with, as the metadata writer stores it.
    name_part: Option<String>,
    /// The host's `RRDHOST_FLAG_METADATA_*`, which this chart's metadata changes raise.
    host_meta: Arc<AtomicU32>,
    /// The host's `pending_flags`, which this chart's obsolete transitions raise.
    host_pending: Arc<AtomicU32>,
    /// `st->last_accessed_time_s`: the last lookup, creation or obsolete transition; the maintenance sweep frees an
    /// obsolete chart only after a quiet time.
    last_accessed_s: AtomicI64,
    /// Out of the host's index for good (D94.1): lookups miss it, the contexts hooks and the parser leave it alone.
    freed: AtomicBool,
    /// `st->rrdlabels_last_saved_version`.
    labels_saved_version: AtomicU32,
    meta: RwLock<ChartMeta>,
    collection: Mutex<ChartCollection>,
    dims: RwLock<Index<Dim>>,
    receiver: Mutex<ReceiverState>,
    /// Chart variables (`VARIABLE CHART`), used by health.
    variables: Mutex<HashMap<String, f64>>,
}

impl Chart {
    /// The daemon's storage (`host->db[]` and C's process globals).
    pub fn storage(&self) -> &Arc<StorageLayout> {
        &self.storage
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn uuid(&self) -> &[u8; 16] {
        &self.uuid
    }

    pub(crate) fn weak(&self) -> Weak<Chart> {
        self.me.clone()
    }

    /// The chart's context and instance.
    pub fn contexts(&self) -> &ChartLink {
        &self.link
    }

    pub(crate) fn host_contexts(&self) -> &Contexts {
        &self.host_contexts
    }

    /// `rrdset_metadata_updated()`: the metadata version moves with the stream sender; contexts follow now.
    pub fn metadata_updated(&self) {
        contexts::updated_rrdset(self);
    }

    /// `rrdset_touch_last_accessed_time_s()`.
    pub fn touch_last_accessed(&self) {
        self.last_accessed_s
            .store(now_realtime_s(), Ordering::Relaxed);
    }

    /// `st->last_accessed_time_s`.
    pub fn last_accessed_s(&self) -> i64 {
        self.last_accessed_s.load(Ordering::Relaxed)
    }

    /// `rrdset_is_discoverable()`.
    pub fn is_discoverable(&self) -> bool {
        flags::is_discoverable(self.flags())
    }

    /// Takes the chart's `OBSOLETE_DIMENSIONS` flag, for a sweep of its dimensions; whether it was set.
    pub fn take_obsolete_dimensions(&self) -> bool {
        self.update_meta(|m| {
            let was = m.flags & flags::OBSOLETE_DIMENSIONS != 0;
            m.flags &= !flags::OBSOLETE_DIMENSIONS;
            was
        })
    }

    /// Raises `OBSOLETE_DIMENSIONS` again: a sweep left some dimension for the next one.
    pub fn raise_obsolete_dimensions(&self) {
        self.update_meta(|m| m.flags |= flags::OBSOLETE_DIMENSIONS);
    }

    /// Whether the chart was freed (D94.1).
    pub fn is_freed(&self) -> bool {
        self.freed.load(Ordering::Acquire)
    }

    /// `rrddim_free()` of an obsolete dimension, when `free` still holds under the index lock: the dimension leaves the
    /// index and is marked freed (writes through a held `Arc` then do nothing, D94.1), then [`Chart::dim_freed`].
    /// Whether it was freed.
    pub fn free_dim_if(&self, dim: &Arc<Dim>, free: impl FnOnce(&Dim) -> bool) -> bool {
        {
            let mut index = self.dims.write().unwrap_or_else(PoisonError::into_inner);
            if dim.is_freed()
                || !index.get(dim.id()).is_some_and(|d| Arc::ptr_eq(&d, dim))
                || !free(dim)
            {
                return false;
            }
            dim.freed.store(true, Ordering::Release);
            index.remove(dim.id());
        }
        self.dim_freed(dim);
        true
    }

    /// `rrddim_delete_callback()`: the dimension's metric is archived and unlinked, its collection ends, its metadata
    /// row goes when no data remains (always in the memory modes), the RAM index forgets it and the receiver's
    /// dimension cache drops it.
    fn dim_freed(&self, dim: &Arc<Dim>) {
        contexts::removed_rrddim(dim);
        let has_retention = dim.finalize_collection();
        if freed_dimension_deletes(self.mode, has_retention) {
            self.storage.freed_dimension_row(dim.uuid());
        }
        self.host_contexts.ram_index().release(dim);
        for entry in &mut self.receiver().prd {
            if entry.as_ref().is_some_and(|d| Arc::ptr_eq(d, dim)) {
                *entry = None;
            }
        }
    }

    /// `rrdset_delete_callback()` of a chart out of the index: every dimension freed, then the chart's instance
    /// archived with the chart's labels and unlinked (`rrdcontext_removed_rrdset()`, after the dimensions, as C orders
    /// it), and the receiver's caches emptied.
    fn freed_contents(&self) {
        let dims = {
            let mut index = self.dims.write().unwrap_or_else(PoisonError::into_inner);
            for dim in index.items() {
                dim.freed.store(true, Ordering::Release);
            }
            std::mem::take(&mut *index)
        };
        for dim in dims.items() {
            self.dim_freed(dim);
        }
        contexts::removed_rrdset(self);
        *self.receiver() = ReceiverState::default();
    }

    /// `rrdset_is_obsolete___safe_from_collector_thread()`, without the sender's parts.
    pub fn is_obsolete(&self) {
        let was = self.update_meta(|m| {
            let was = m.flags;
            m.flags |= flags::OBSOLETE;
            was
        });
        if was & flags::OBSOLETE == 0 {
            self.host_pending
                .fetch_or(pending_flags::OBSOLETE_CHARTS, Ordering::AcqRel);
            self.touch_last_accessed();
            self.metadata_updated();
            contexts::updated_rrdset_flags(self);
        }
    }

    /// `rrdset_isnot_obsolete___safe_from_collector_thread()`.
    pub fn isnot_obsolete(&self) {
        let was = self.update_meta(|m| {
            let was = m.flags;
            m.flags &= !flags::OBSOLETE;
            was
        });
        if was & flags::OBSOLETE != 0 {
            self.touch_last_accessed();
            self.metadata_updated();
            contexts::updated_rrdset_flags(self);
        }
    }

    /// `rrddim_is_obsolete___safe_from_collector_thread()`.
    pub fn dim_is_obsolete(&self, dim: &Dim) {
        let was = dim.update_meta(|m| {
            let was = m.flags;
            m.flags |= dim_flags::OBSOLETE;
            was
        });
        if was & dim_flags::OBSOLETE == 0 {
            self.update_meta(|m| m.flags |= flags::OBSOLETE_DIMENSIONS);
            self.host_pending
                .fetch_or(pending_flags::OBSOLETE_DIMENSIONS, Ordering::AcqRel);
            contexts::updated_rrddim_flags(dim);
            self.metadata_updated();
        }
    }

    /// `rrddim_isnot_obsolete___safe_from_collector_thread()`: a dimension back from obsolete collects again.
    pub fn dim_isnot_obsolete(&self, dim: &Dim) {
        let was = dim.update_meta(|m| {
            let was = m.flags;
            m.flags &= !dim_flags::OBSOLETE;
            was
        });
        if was & dim_flags::OBSOLETE != 0 {
            self.reinitialize_collection(dim);
            contexts::updated_rrddim_flags(dim);
            self.metadata_updated();
        }
    }

    /// `rrddim_reinitialize_collection()`: each dbengine tier whose collection ended starts it again.
    fn reinitialize_collection(&self, dim: &Dim) {
        let Some(engine) = self.storage.dbengine() else {
            return;
        };
        let ue = i64::from(self.update_every());
        let mut store = lock(&dim.store);
        for (t, tier) in dim.tiers.iter().enumerate() {
            if let (TierMetric::Dbengine(metric), None) = (tier, &store.tiers[t].handle) {
                let tier_ue = (self.storage.tier_grouping(t) as i64 * ue) as u32;
                store.tiers[t].handle = Some(CollectHandle::init(
                    engine,
                    metric,
                    tier_ue,
                    self.alignment[t],
                ));
            }
        }
    }

    pub fn type_(&self) -> &str {
        &self.type_
    }

    pub fn id_part(&self) -> &str {
        &self.id_part
    }

    pub fn mode(&self) -> DbMode {
        self.mode
    }

    pub fn entries(&self) -> usize {
        self.entries
    }

    pub fn name_part(&self) -> Option<&str> {
        self.name_part.as_deref()
    }

    /// `RRDSET_FLAG_METADATA_UPDATE` and the host's `RRDHOST_FLAG_METADATA_UPDATE`: the metadata writer stores the
    /// chart at its next run.
    pub fn set_metadata_update(&self) {
        self.update_meta(|m| m.flags |= flags::METADATA_UPDATE);
        self.host_meta
            .fetch_or(meta_flags::UPDATE, Ordering::AcqRel);
    }

    /// The writer's check and clear of `RRDSET_FLAG_METADATA_UPDATE`: whether it was set.
    pub fn take_metadata_update(&self) -> bool {
        // the common case, nothing to store, under the read lock
        if self.flags() & flags::METADATA_UPDATE == 0 {
            return false;
        }
        self.update_meta(|m| {
            let was = m.flags & flags::METADATA_UPDATE != 0;
            m.flags &= !flags::METADATA_UPDATE;
            was
        })
    }

    /// `st->rrdlabels_last_saved_version`: the labels version the writer last stored.
    pub fn labels_saved_version(&self) -> u32 {
        self.labels_saved_version.load(Ordering::Acquire)
    }

    pub fn set_labels_saved_version(&self, version: u32) {
        self.labels_saved_version.store(version, Ordering::Release);
    }

    /// `RRDDIM_FLAG_METADATA_UPDATE` and the host's `RRDHOST_FLAG_METADATA_UPDATE`.
    pub fn set_dim_metadata_update(&self, dim: &Dim) {
        dim.update_meta(|m| m.flags |= dim_flags::METADATA_UPDATE);
        self.host_meta
            .fetch_or(meta_flags::UPDATE, Ordering::AcqRel);
    }

    /// The writer's step on a dimension (`metadata_scan_host()`): when `RRDDIM_FLAG_METADATA_UPDATE` is set, it is
    /// cleared, `RRDDIM_FLAG_META_HIDDEN` follows the `hidden` option, and the metadata to store is returned.
    pub fn take_dim_metadata_update(&self, dim: &Dim) -> Option<DimMeta> {
        let flags = dim
            .meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .flags;
        if flags & dim_flags::METADATA_UPDATE == 0 {
            return None;
        }
        dim.update_meta(|m| {
            if m.flags & dim_flags::METADATA_UPDATE == 0 {
                return None;
            }
            m.flags &= !dim_flags::METADATA_UPDATE;
            if m.flags & dim_flags::HIDDEN != 0 {
                m.flags |= dim_flags::META_HIDDEN;
            } else {
                m.flags &= !dim_flags::META_HIDDEN;
            }
            Some(m.clone())
        })
    }

    /// The DIMENSION `hidden` option (`pluginsd_dimension()`): the option follows, and the metadata is stored again
    /// when it differs from the stored one (`RRDDIM_FLAG_META_HIDDEN`).
    pub fn dim_set_hidden(&self, dim: &Dim, hidden: bool) {
        let update = dim.update_meta(|m| {
            if hidden {
                m.flags |= dim_flags::HIDDEN;
            } else {
                m.flags &= !dim_flags::HIDDEN;
            }
            hidden != (m.flags & dim_flags::META_HIDDEN != 0)
        });
        if update {
            self.set_dim_metadata_update(dim);
        }
    }

    pub fn meta(&self) -> ChartMeta {
        self.meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn update_meta<T>(&self, update: impl FnOnce(&mut ChartMeta) -> T) -> T {
        update(&mut self.meta.write().unwrap_or_else(PoisonError::into_inner))
    }

    /// `rrdset_flag_check()`: the chart's flags, without copying the rest of its metadata.
    pub fn flags(&self) -> u32 {
        self.meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .flags
    }

    pub fn collection(&self) -> ChartCollection {
        *lock(&self.collection)
    }

    pub fn update_collection<T>(&self, update: impl FnOnce(&mut ChartCollection) -> T) -> T {
        update(&mut lock(&self.collection))
    }

    /// `rrdvar_chart_variable_set()`.
    pub fn set_variable(&self, name: &str, value: f64) {
        lock(&self.variables).insert(name.to_string(), value);
    }

    /// The parser's state on this chart.
    pub fn receiver(&self) -> MutexGuard<'_, ReceiverState> {
        lock(&self.receiver)
    }

    pub fn dim_count(&self) -> usize {
        self.dims
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .items()
            .len()
    }

    /// `rrdset_first_entry_s_of_tier(st, 0)` and `rrdset_last_entry_s_of_tier(st, 0)`: the oldest and newest time any
    /// dimension holds in tier 0 (0 when none).
    pub fn tier0_retention(&self) -> (i64, i64) {
        span(self.dims().iter().map(|dim| dim.tier_retention(0)))
    }

    /// `rrdset_first_entry_s()` and `rrdset_last_entry_s()`: over every dimension and tier.
    pub fn retention(&self) -> (i64, i64) {
        span(self.dims().iter().map(|dim| dim.retention()))
    }

    /// `rrdset_finalize_collection()`: the dimensions' collection ends (with `dimensions_too`), their retention
    /// verdicts unused.
    pub fn finalize_collection(&self, dimensions_too: bool) {
        if dimensions_too {
            for dim in self.dims() {
                dim.finalize_collection();
            }
        }
    }

    pub fn update_every(&self) -> i32 {
        self.meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .update_every
    }

    pub fn dims(&self) -> Vec<Arc<Dim>> {
        self.dims
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .items()
            .to_vec()
    }

    /// `rrddim_find(st, id, include_obsolete = true)`: a dimension found touches its chart's last accessed time.
    pub fn dim(&self, id: &str) -> Option<Arc<Dim>> {
        let dim = self.dims.read().unwrap_or_else(PoisonError::into_inner).get(id);
        if dim.is_some() {
            self.touch_last_accessed();
        }
        dim
    }

    /// `rrdset_set_update_every_s()`: an invalid value is ignored; a change reaches every dimension's storage on every
    /// tier. Returns the value in force afterwards.
    pub fn set_update_every(&self, update_every: i64) -> i32 {
        let mut meta = self.meta.write().unwrap_or_else(PoisonError::into_inner);
        if update_every <= 0
            || update_every > i64::from(i32::MAX)
            || update_every == i64::from(meta.update_every)
        {
            return meta.update_every;
        }
        meta.update_every = update_every as i32;
        drop(meta);
        for dim in self.dims() {
            dim.change_collection_frequency(&self.storage, update_every);
        }
        update_every as i32
    }

    /// `rrddim_add()`: a new dimension gets a ring seeded from the chart's counters; an existing one is
    /// un-obsoleted and gets the changed fields. Returns the dimension and whether it is new.
    pub fn dim_add(
        &self,
        id: &str,
        name: Option<&str>,
        multiplier: i32,
        divisor: i32,
        algorithm: Algorithm,
    ) -> (Arc<Dim>, bool) {
        let divisor = if divisor == 0 { 1 } else { divisor };
        // read before the dimensions lock: a new dimension looks its UUID up in this context
        let context = self.meta().context;
        let mut index = self.dims.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(dim) = index.get(id) {
            drop(index);
            self.dim_isnot_obsolete(&dim);
            // freed by the maintenance sweep since the lookup: added again (C retries on the destroy lock)
            if dim.is_freed() {
                return self.dim_add(id, name, multiplier, divisor, algorithm);
            }
            // rrddim_conflict_callback(): rename, algorithm, multiplier, divisor, each reported as it changes.
            let (renamed, algorithm_changed, multiplier_changed, divisor_changed) = dim
                .update_meta(|m| {
                    let renamed = match name.filter(|n| !n.is_empty() && *n != m.name) {
                        Some(name) => {
                            m.name = rrd_string(name);
                            true
                        }
                        None => false,
                    };
                    let algorithm_changed = m.algorithm != algorithm;
                    m.algorithm = algorithm;
                    let multiplier_changed = m.multiplier != multiplier;
                    m.multiplier = multiplier;
                    let divisor_changed = m.divisor != divisor;
                    m.divisor = divisor;
                    (
                        renamed,
                        algorithm_changed,
                        multiplier_changed,
                        divisor_changed,
                    )
                });
            if renamed {
                self.dim_metadata_updated(&dim);
            }
            if algorithm_changed {
                self.dim_metadata_updated(&dim);
                contexts::updated_rrddim_algorithm(&dim);
            }
            for changed in [multiplier_changed, divisor_changed] {
                if changed {
                    self.dim_metadata_updated(&dim);
                    contexts::updated_rrddim_flags(&dim);
                }
            }
            if renamed || algorithm_changed || multiplier_changed || divisor_changed {
                // rrddim_react_callback(): RRDDIM_REACT_UPDATED
                self.set_dim_metadata_update(&dim);
                self.update_meta(|m| m.flags |= flags::SYNC_CLOCK | flags::HOMOGENEOUS_CHECK);
                self.dim_metadata_updated(&dim);
            }
            // the conflict callback ends by collecting again where the collection had ended
            self.reinitialize_collection(&dim);
            return (dim, false);
        }
        let meta = self.meta();
        let collection = self.collection();
        // rrdcontext_find_dimension_uuid(): a dimension created again keeps its metric's UUID
        let uuid = self
            .host_contexts
            .find_dimension_uuid(&context, &self.id, id)
            .unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());
        // the ram engine's tier 0 for the modes that are not dbengine, then the dbengine's tiers (N8); every tier
        // above 0 aggregates into windows that its flush modulo spreads (rrddim_collection_modulo())
        let (mut tiers, mut collect) = (Vec::new(), Vec::new());
        let rollup = |t: usize| {
            let grouping = self.storage.tier_grouping(t);
            let spread = (grouping as i64 * i64::from(meta.update_every)) as u32;
            Rollup::new(
                grouping,
                tiers::flush_modulo(self.collection_modulo, spread),
            )
        };
        if self.mode != DbMode::Dbengine {
            let entries = if self.mode == DbMode::Ram {
                self.entries.max(1)
            } else {
                self.entries.max(ALLOC_MIN_ENTRIES)
            };
            tiers.push(TierMetric::Ram(RamMetric::new(
                entries,
                Seed {
                    counter: collection.counter,
                    current_entry: collection.current_entry,
                    last_updated_s: collection.last_updated.0,
                    update_every_s: i64::from(meta.update_every),
                },
            )));
            collect.push(TierCollect {
                tier: 0,
                handle: None,
                rollup: rollup(0),
            });
        }
        if let Some(engine) = self.storage.dbengine() {
            for t in 0..self.storage.storage_tiers() {
                if !self.storage.tier_is_dbengine(self.mode, t) {
                    continue;
                }
                // rrdeng_metric_get_or_create(), then rrdeng_store_metric_init() at the tier's update every
                let (metric, _) = engine.mrg.add_and_acquire(&uuid, t, 0, 0, 0);
                let tier_ue =
                    (self.storage.tier_grouping(t) as i64 * i64::from(meta.update_every)) as u32;
                collect.push(TierCollect {
                    tier: t,
                    handle: Some(CollectHandle::init(
                        engine,
                        &metric,
                        tier_ue,
                        self.alignment[t],
                    )),
                    rollup: rollup(t),
                });
                tiers.push(TierMetric::Dbengine(metric));
            }
        }
        let dim = Arc::new_cyclic(|me| Dim {
            me: me.clone(),
            link: DimLink::default(),
            id: id.to_string(),
            uuid,
            meta: RwLock::new(DimMeta {
                name: name
                    .filter(|n| !n.is_empty())
                    .map_or_else(|| id.to_string(), rrd_string),
                algorithm,
                multiplier,
                divisor,
                // rrddim_react_callback(): RRDDIM_REACT_NEW
                flags: dim_flags::METADATA_UPDATE,
            }),
            collection: Mutex::new(DimCollection {
                counter: usize::from(meta.flags & flags::STORE_FIRST != 0),
                ..DimCollection::default()
            }),
            tiers,
            storage: Arc::clone(&self.storage),
            store: Mutex::new(DimStore {
                tiers: collect,
                update_every: i64::from(meta.update_every),
                backfilled: false,
            }),
            freed: AtomicBool::new(false),
        });
        // pulse_db_rrd_memory_add(), which the dimension's drop takes back
        if let Some(ring) = dim.ring() {
            self.storage.pulse().rrd_memory.add(ring.memsize());
        }
        // C compares with the first other dimension only (the loop breaks after it).
        let heterogeneous = index.items().first().is_some_and(|td| {
            let t = td.meta();
            t.algorithm != algorithm
                || i64::from(t.multiplier).abs() != i64::from(multiplier).abs()
                || i64::from(t.divisor).abs() != i64::from(divisor).abs()
        });
        index.insert(id, Arc::clone(&dim));
        drop(index);
        self.update_meta(|m| {
            m.flags |= flags::SYNC_CLOCK;
            if heterogeneous {
                m.flags |= flags::HETEROGENEOUS;
            }
        });
        if dim.ring().is_some() {
            self.host_contexts().ram_index().register(&dim);
        }
        self.host_meta
            .fetch_or(meta_flags::UPDATE, Ordering::AcqRel);
        self.dim_metadata_updated(&dim);
        (dim, true)
    }

    /// `rrddim_metadata_updated()`.
    fn dim_metadata_updated(&self, dim: &Dim) {
        contexts::updated_rrddim(self, dim);
        self.metadata_updated();
    }
}

/// The dimension flags this port tracks (`RRDDIM_FLAG_*`, `RRDDIM_OPTION_*`).
pub mod dim_flags {
    pub const OBSOLETE: u32 = 1 << 0;
    pub const HIDDEN: u32 = 1 << 1;
    pub const DONT_DETECT_RESETS_OR_OVERFLOWS: u32 = 1 << 2;
    pub const FLOAT: u32 = 1 << 3;
    pub const UPDATED: u32 = 1 << 4;
    /// `RRDDIM_FLAG_METADATA_UPDATE`: the metadata writer stores the dimension at its next run.
    pub const METADATA_UPDATE: u32 = 1 << 5;
    /// `RRDDIM_FLAG_META_HIDDEN`: the `hidden` option as last stored.
    pub const META_HIDDEN: u32 = 1 << 6;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DimMeta {
    pub name: String,
    pub algorithm: Algorithm,
    pub multiplier: i32,
    pub divisor: i32,
    pub flags: u32,
}

/// A dimension's collection state (`rd->collector`, `rd->last_*`).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DimCollection {
    pub counter: usize,
    pub last_collected_time: (i64, i64),
    /// The int and float lanes of C's `collector.collected` union; which one is live follows the FLOAT option.
    pub collected_value: i64,
    pub collected_value_float: f64,
    pub last_collected_value: i64,
    pub last_collected_value_float: f64,
    /// `collected_value_max`: the largest magnitude collected, which picks the incremental wrap cap.
    pub collected_value_max: i64,
    pub last_stored_value: f64,
    pub last_calculated_value: f64,
    pub calculated_value: f64,
}

/// `rd->tiers[t].smh`: a dimension's storage on one tier, fixed at insert.
#[derive(Debug)]
enum TierMetric {
    Ram(RamMetric),
    Dbengine(Handle),
}

/// The earliest non-zero first time and the latest last time of these retentions (0 when none).
fn span(retentions: impl Iterator<Item = (i64, i64)>) -> (i64, i64) {
    retentions.fold((0, 0), |(first, last), (f, l)| {
        let first = if f != 0 && (first == 0 || f < first) {
            f
        } else {
            first
        };
        (first, last.max(l))
    })
}

/// `RRDDIM`.
#[derive(Debug)]
pub struct Dim {
    me: Weak<Dim>,
    id: String,
    uuid: [u8; 16],
    /// `rd->rrdcontexts`.
    link: DimLink,
    meta: RwLock<DimMeta>,
    collection: Mutex<DimCollection>,
    /// The storage of each tier in use, by tier: the ram ring of a chart that is not dbengine at tier 0, the
    /// dbengine's registry entries elsewhere.
    tiers: Vec<TierMetric>,
    /// What the collection writes through, by tier; one lock serializes stores, finalization and frequency changes.
    store: Mutex<DimStore>,
    /// The host's storage (`rd->rrdset->rrdhost->db[]`): the backfill mode and the engine the tiers read from.
    storage: Arc<StorageLayout>,
    /// Out of its chart's index for good (D94.1).
    freed: AtomicBool,
}

/// `rd->tiers[t]`: a tier's collection (`sch`, `None` for a ram tier and once finalized) and, above tier 0, the
/// window it aggregates.
#[derive(Debug)]
struct TierCollect {
    /// The tier (its position among the dimension's tiers).
    tier: usize,
    handle: Option<CollectHandle>,
    rollup: Rollup,
}

impl TierCollect {
    /// `storage_engine_store_metric()` of a tier record: nothing once the collection ended (a NULL `sch`); a stored
    /// record counts in the pulse charts.
    fn write(&mut self, record: Option<TierRecord>) {
        if let (Some(r), Some(handle)) = (record, self.handle.as_mut()) {
            pulse::point_stored(self.tier);
            handle.store_next(
                r.end_time_s as u64 * 1_000_000,
                r.sum,
                r.min,
                r.max,
                r.count,
                r.anomaly_count,
                r.flags,
            );
        }
    }
}

impl Drop for Dim {
    /// `rrddim_free()`'s `pulse_db_rrd_memory_sub()` of the ring.
    fn drop(&mut self) {
        if let Some(ring) = self.ring() {
            self.storage.pulse().rrd_memory.sub(ring.memsize());
        }
    }
}

impl Drop for TierCollect {
    /// A dropped collection does finalize's work (D66 2.Q6), the parked window first.
    fn drop(&mut self) {
        let parked = self.rollup.flush();
        self.write(parked);
    }
}

/// A dimension's storage state.
#[derive(Debug)]
struct DimStore {
    tiers: Vec<TierCollect>,
    /// `rd->rrdset->update_every`, which the windows follow: the chart's, updated with its frequency changes.
    update_every: i64,
    /// `RRDDIM_OPTION_BACKFILLED_HIGH_TIERS`: set after the first store.
    backfilled: bool,
}

/// `rrddim_delete_callback()`'s row deletion: a dbengine dimension without retention, or any dimension of the
/// other modes (their data lives only in memory).
fn freed_dimension_deletes(mode: DbMode, has_retention: bool) -> bool {
    (mode == DbMode::Dbengine && !has_retention)
        || matches!(mode, DbMode::Ram | DbMode::Alloc | DbMode::None)
}

impl Dim {
    /// Whether the dimension was freed (D94.1).
    pub fn is_freed(&self) -> bool {
        self.freed.load(Ordering::Acquire)
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn uuid(&self) -> &[u8; 16] {
        &self.uuid
    }

    pub fn meta(&self) -> DimMeta {
        self.meta
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn update_meta<T>(&self, update: impl FnOnce(&mut DimMeta) -> T) -> T {
        update(&mut self.meta.write().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn collection(&self) -> DimCollection {
        *lock(&self.collection)
    }

    pub fn update_collection<T>(&self, update: impl FnOnce(&mut DimCollection) -> T) -> T {
        update(&mut lock(&self.collection))
    }

    /// The ram ring of tier 0, for a chart that is not dbengine.
    pub fn ring(&self) -> Option<&RamMetric> {
        match self.tiers.first() {
            Some(TierMetric::Ram(ring)) => Some(ring),
            _ => None,
        }
    }

    pub(crate) fn weak(&self) -> Weak<Dim> {
        self.me.clone()
    }

    /// The dimension's metric.
    pub fn contexts(&self) -> &DimLink {
        &self.link
    }

    /// `rrddim_first_entry_s_of_tier()` and `rrddim_last_entry_s_of_tier()`: the ring's oldest and newest points, or
    /// the registry's retention; `(0, 0)` past the tiers.
    pub fn tier_retention(&self, tier: usize) -> (i64, i64) {
        match self.tiers.get(tier) {
            Some(TierMetric::Ram(ring)) => (ring.oldest_time_s(), ring.latest_time_s()),
            Some(TierMetric::Dbengine(metric)) => (metric.first_time_s(), metric.latest_time_s()),
            None => (0, 0),
        }
    }

    /// Over every tier.
    fn retention(&self) -> (i64, i64) {
        span((0..self.tiers.len()).map(|t| self.tier_retention(t)))
    }

    /// `rrddim_first_entry_s()`: the oldest point of any tier.
    pub fn first_entry_s(&self) -> i64 {
        self.retention().0
    }

    /// `rrddim_last_entry_s()`: the newest point of any tier.
    pub fn last_entry_s(&self) -> i64 {
        self.retention().1
    }

    /// `rrddim_store_metric()`: the point into tier 0 (the ring, or the dbengine collection while it runs), then
    /// into the window of every dbengine tier above it, whose state advances even once its collection ended; the
    /// first store after a (re)link marks the metric collected.
    pub fn store_metric(&self, point_end_time_ut: u64, value: f64, flags: u32) {
        let mut store = lock(&self.store);
        match self.tiers.first() {
            Some(TierMetric::Ram(ring)) => ring.store(point_end_time_ut, value, flags),
            Some(TierMetric::Dbengine(_)) => {
                if let Some(handle) = store.tiers[0].handle.as_mut() {
                    handle.store_next(point_end_time_ut, value, 0.0, 0.0, 1, 0, flags);
                }
            }
            None => {}
        }
        pulse::point_stored(0);
        let update_every = store.update_every;
        let point = tiers::collected_point(point_end_time_ut, value, flags, update_every);
        for (t, tier) in self.tiers.iter().enumerate().skip(1) {
            if !matches!(tier, TierMetric::Dbengine(_)) {
                continue;
            }
            if !store.backfilled {
                self.backfill(&mut store, t, point.end_time_s);
            }
            let tier = &mut store.tiers[t];
            let record = tier.rollup.store(update_every, point);
            tier.write(record);
        }
        store.backfilled = true;
        drop(store);
        contexts::collected_rrddim(self);
    }

    /// `backfill_tier_from_smaller_tiers()`: a tier collected for the first time takes, into its window, the points
    /// the tiers below it hold after its own newest one, the closest tier first; not with the backfill off, not an
    /// empty tier unless it is `full`, and not when less than one window is missing. Whether it read the tiers below.
    fn backfill(&self, store: &mut DimStore, tier: usize, now_s: i64) -> bool {
        let backfill = self.storage.backfill();
        let (Some(engine), Some(TierMetric::Dbengine(metric))) =
            (self.storage.dbengine(), self.tiers.get(tier))
        else {
            return false;
        };
        if backfill == Backfill::None {
            return false;
        }
        let mut latest_s = metric.latest_time_s();
        let update_every = store.update_every;
        let granularity = store.tiers[tier].rollup.grouping() * update_every;
        if backfill == Backfill::New && latest_s <= 0 {
            return false;
        }
        if now_s <= latest_s || now_s - latest_s < granularity {
            return false;
        }
        let _running = BackfillRunning::start();
        for read_tier in (0..tier).rev() {
            let (first_s, last_s) = self.tier_retention(read_tier);
            if last_s <= latest_s {
                continue;
            }
            let after_s = latest_s.max(first_s);
            let mut q = match &self.tiers[read_tier] {
                TierMetric::Ram(ring) => StorageQuery::Ram(ring.query(after_s, last_s)),
                TierMetric::Dbengine(metric) => StorageQuery::Dbengine(engine.query(
                    metric,
                    after_s,
                    last_s,
                    Priority::SynchronousFirst,
                )),
            };
            let mut points_read = 0;
            while !q.is_finished() {
                let point = q.next_metric();
                points_read += 1;
                if point.end_time_s > latest_s {
                    latest_s = point.end_time_s;
                    let target = &mut store.tiers[tier];
                    let record = target.rollup.store(update_every, point);
                    target.write(record);
                }
            }
            drop(q);
            let pulse = self.storage.pulse();
            pulse
                .ingestion
                .collection_completed(self.storage.storage_tiers());
            pulse.queries.backfill_query_completed(points_read);
        }
        true
    }

    /// `RRDDIM_OPTION_BACKFILLED_HIGH_TIERS`.
    pub fn is_backfilled(&self) -> bool {
        lock(&self.store).backfilled
    }

    /// `backfill_execute()` for this dimension: every tier above 0 backfills up to the wall clock as it reads it;
    /// the dimension counts as backfilled only when some tier got past the backfill's checks.
    pub fn backfill_tiers(&self, clock: fn() -> i64) -> bool {
        let mut store = lock(&self.store);
        let mut success = false;
        for tier in 1..self.tiers.len() {
            success |= self.backfill(&mut store, tier, clock());
        }
        if success {
            store.backfilled = true;
        }
        success
    }

    /// A restart as the tiers see it: the windows and the backfilled option start over, the collections stay.
    #[cfg(test)]
    pub(crate) fn restarted(&self) {
        let mut store = lock(&self.store);
        store.backfilled = false;
        for tier in &mut store.tiers {
            tier.rollup = Rollup::new(tier.rollup.grouping() as u64, tier.rollup.flush_modulo());
        }
    }

    /// `rrdset_set_update_every_s()` for this dimension: each tier's storage takes the tier's update every.
    fn change_collection_frequency(&self, storage: &StorageLayout, update_every: i64) {
        let mut store = lock(&self.store);
        store.update_every = update_every;
        for (t, tier) in self.tiers.iter().enumerate() {
            match (tier, store.tiers[t].handle.as_mut()) {
                (TierMetric::Ram(ring), _) => ring.change_update_every(update_every),
                (TierMetric::Dbengine(_), Some(handle)) => handle.change_collection_frequency(
                    (storage.tier_grouping(t) as i64 * update_every) as u32,
                ),
                (TierMetric::Dbengine(_), None) => {}
            }
        }
    }

    /// `storage_engine_store_flush()` of every tier: the ring's pending point, a dbengine tier's page.
    pub(crate) fn store_flush(&self) {
        let mut store = lock(&self.store);
        for (t, tier) in self.tiers.iter().enumerate() {
            match (tier, store.tiers[t].handle.as_mut()) {
                (TierMetric::Ram(ring), _) => ring.flush(),
                (TierMetric::Dbengine(_), Some(handle)) => handle.flush_current_page(),
                (TierMetric::Dbengine(_), None) => {}
            }
        }
    }

    /// `rrddim_finalize_collection_and_check_retention()`: every tier's collection ends, a tier above 0 writing its
    /// parked window first (the one being filled is lost); whether the dimension still has data (a tier with
    /// retention, a ram tier, which C counts as retained, or no tier at all).
    pub fn finalize_collection(&self) -> bool {
        let mut store = lock(&self.store);
        let (mut available, mut said_no) = (0, 0);
        for (t, tier) in self.tiers.iter().enumerate() {
            match tier {
                TierMetric::Ram(_) => available += 1,
                TierMetric::Dbengine(_) => {
                    let tier = &mut store.tiers[t];
                    if tier.handle.is_none() {
                        continue;
                    }
                    if t > 0 {
                        let parked = tier.rollup.flush();
                        tier.write(parked);
                    }
                    if let Some(handle) = tier.handle.take() {
                        available += 1;
                        said_no += usize::from(handle.finalize());
                    }
                }
            }
        }
        said_no == 0 || available > said_no
    }
}

/// A host's charts: by id, by name, and in creation order (`rrdset_root_index`, `rrdset_root_index_name`).
#[derive(Debug, Default)]
pub struct Charts {
    inner: RwLock<ChartIndex>,
    /// The host's contexts, which every chart reports to.
    contexts: Arc<Contexts>,
    /// The host's `RRDHOST_FLAG_METADATA_*`.
    host_meta: Arc<AtomicU32>,
    /// The host's `pending_flags`.
    host_pending: Arc<AtomicU32>,
    /// The host's storage, which its charts' dimensions keep their points in.
    storage: Arc<StorageLayout>,
    /// The host's GUID, which staggers its charts' pages (D65.1).
    host_guid: String,
}

/// A host's charts by id, and their names (`rrdset_index_name`) with the id each names.
#[derive(Debug, Default)]
struct ChartIndex {
    charts: Index<Chart>,
    by_name: HashMap<String, String>,
}

impl ChartIndex {
    /// `rrdset_fix_name()`: `type.name`, sanitized; a taken name is refused, except that a chart named after its own
    /// id with no name yet gets the first free `_N` suffix.
    fn fix_name(
        &self,
        full_id: &str,
        type_: &str,
        current: Option<&str>,
        name: Option<&str>,
    ) -> Option<String> {
        let name = name.filter(|n| !n.is_empty())?;
        let full_name = bounded(format!("{type_}.{name}"), ID_LENGTH_MAX);
        let sanitized = text(&rrdset_strncpyz_name(full_name.as_bytes(), NAME_LENGTH_MAX));
        if !self.by_name.contains_key(&sanitized) {
            return Some(sanitized);
        }
        if full_id == full_name && current.is_none_or(str::is_empty) {
            let mut i = 1u32;
            loop {
                let candidate = bounded(format!("{sanitized}_{i}"), NAME_LENGTH_MAX);
                if !self.by_name.contains_key(&candidate) {
                    return Some(candidate);
                }
                i += 1;
            }
        }
        None
    }
}

impl Charts {
    pub fn new(
        contexts: Arc<Contexts>,
        host_meta: Arc<AtomicU32>,
        host_pending: Arc<AtomicU32>,
        storage: Arc<StorageLayout>,
        host_guid: &str,
    ) -> Self {
        Charts {
            inner: RwLock::default(),
            contexts,
            host_meta,
            host_pending,
            storage,
            host_guid: host_guid.to_string(),
        }
    }

    /// `rrdset_find()`: a chart that is not discoverable only with `include_obsolete`; a chart found touches its last
    /// accessed time.
    pub fn find(&self, id: &str, include_obsolete: bool) -> Option<Arc<Chart>> {
        let chart = self.inner.read().unwrap_or_else(PoisonError::into_inner).charts.get(id)?;
        if !include_obsolete && !chart.is_discoverable() {
            return None;
        }
        chart.touch_last_accessed();
        Some(chart)
    }

    /// `rrdset_free()` of an obsolete chart, when `free` still holds under the index lock: the chart leaves the indexes
    /// and is marked freed (D94.1), then its dimensions and its instance go as C's delete callback takes them. Whether
    /// it was freed.
    pub fn free_if(&self, chart: &Arc<Chart>, free: impl FnOnce(&Chart) -> bool) -> bool {
        {
            let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
            if chart.is_freed()
                || !index.charts.get(chart.id()).is_some_and(|c| Arc::ptr_eq(&c, chart))
                || !free(chart)
            {
                return false;
            }
            chart.freed.store(true, Ordering::Release);
            index.charts.remove(chart.id());
            if let Some(name) = chart.meta().name
                && index.by_name.get(&name).is_some_and(|id| id == chart.id())
            {
                index.by_name.remove(&name);
            }
        }
        chart.freed_contents();
        true
    }

    /// `rrdset_index_destroy()`: every chart freed (a host archived or freed).
    pub fn flush(&self) {
        let index = std::mem::take(&mut *self.inner.write().unwrap_or_else(PoisonError::into_inner));
        for chart in index.charts.items() {
            chart.freed.store(true, Ordering::Release);
        }
        for chart in index.charts.items() {
            chart.freed_contents();
        }
    }

    /// `rrdset_find_byname()`: a discoverable chart by name, touched.
    pub fn find_by_name(&self, name: &str) -> Option<Arc<Chart>> {
        let chart = {
            let index = self.inner.read().unwrap_or_else(PoisonError::into_inner);
            index.by_name.get(name).and_then(|id| index.charts.get(id))?
        };
        if !chart.is_discoverable() {
            return None;
        }
        chart.touch_last_accessed();
        Some(chart)
    }

    pub fn all(&self) -> Vec<Arc<Chart>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .charts
            .items()
            .to_vec()
    }

    /// `rrdset_create()`: a new chart, or the existing one un-obsoleted and updated by the conflict rules, then
    /// named. Returns the chart and whether it is new.
    pub fn create(&self, spec: &ChartSpec<'_>) -> (Arc<Chart>, bool) {
        let full_id = bounded(format!("{}.{}", spec.type_, spec.id), ID_LENGTH_MAX);
        let existing = self.inner.read().unwrap_or_else(PoisonError::into_inner).charts.get(&full_id);
        if let Some(existing) = existing {
            existing.isnot_obsolete();
        }
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        // rrdset_conflict_callback() reports whether anything changed, and whether the plugin or the module did
        // (RRDSET_REACT_PLUGIN_UPDATED, RRDSET_REACT_MODULE_UPDATED); the react step then runs.
        let (chart, is_new, changed, plugin_or_module) = match index.charts.get(&full_id) {
            Some(chart) => {
                let (mut changed, plugin_or_module) = chart.update_meta(|m| {
                    let mut changed = false;
                    if m.priority != spec.priority {
                        m.priority = spec.priority;
                        changed = true;
                    }
                    // rrdset_metadata_field_update(): a non-empty value whose sanitized text differs
                    let update = |field: &mut String, value: Option<&str>| {
                        let Some(value) = value.filter(|v| !v.is_empty()).map(rrd_string) else {
                            return false;
                        };
                        let changed = value != *field;
                        *field = value;
                        changed
                    };
                    let plugin_or_module = update(&mut m.plugin, Some(spec.plugin))
                        | update(&mut m.module, spec.module);
                    for (field, value) in [
                        (&mut m.title, Some(spec.title)),
                        (&mut m.units, Some(spec.units)),
                        (&mut m.family, spec.family),
                        (&mut m.context, spec.context),
                    ] {
                        changed |= update(field, value);
                    }
                    if m.chart_type != spec.chart_type {
                        m.chart_type = spec.chart_type;
                        changed = true;
                    }
                    let (plugin, module) = (m.plugin.clone(), m.module.clone());
                    m.labels.add(
                        b"_collect_plugin",
                        plugin.as_bytes(),
                        SRC_AUTO | FLAG_DONT_DELETE,
                    );
                    m.labels.add(
                        b"_collect_module",
                        module.as_bytes(),
                        SRC_AUTO | FLAG_DONT_DELETE,
                    );
                    m.flags |= flags::SYNC_CLOCK;
                    (changed || plugin_or_module, plugin_or_module)
                });
                if chart.update_every() != spec.update_every {
                    chart.set_update_every(i64::from(spec.update_every));
                    changed = true;
                }
                (chart, false, changed, plugin_or_module)
            }
            None => {
                let mut labels = Labels::default();
                let plugin = rrd_string(spec.plugin);
                let module = rrd_string(spec.module.unwrap_or(""));
                labels.add(
                    b"_collect_plugin",
                    plugin.as_bytes(),
                    SRC_AUTO | FLAG_DONT_DELETE,
                );
                labels.add(
                    b"_collect_module",
                    module.as_bytes(),
                    SRC_AUTO | FLAG_DONT_DELETE,
                );
                let entries = if spec.mode == DbMode::Dbengine {
                    5
                } else {
                    align_entries_to_pagesize(spec.mode, spec.history_entries, spec.page_size)
                        as usize
                };
                // rrdcontext_find_chart_uuid(): a chart created again keeps its instance's UUID
                let context = spec.context.filter(|c| !c.is_empty()).unwrap_or(&full_id);
                let uuid = self
                    .contexts
                    .find_chart_uuid(&rrd_string(context), &full_id)
                    .unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());
                let chart = Arc::new_cyclic(|me| Chart {
                    me: me.clone(),
                    id: full_id.clone(),
                    uuid,
                    host_contexts: Arc::clone(&self.contexts),
                    link: ChartLink::default(),
                    type_: spec.type_.to_string(),
                    id_part: spec.id.to_string(),
                    mode: spec.mode,
                    entries,
                    storage: Arc::clone(&self.storage),
                    // rrdset_index_insert(): the metrics group of every tier with an engine
                    alignment: (0..self.storage.storage_tiers())
                        .map(|t| Alignment::new(&self.host_guid, &full_id, t))
                        .collect(),
                    collection_modulo: self.storage.next_collection_modulo(),
                    name_part: spec.name.filter(|n| !n.is_empty()).map(str::to_string),
                    host_meta: Arc::clone(&self.host_meta),
                    host_pending: Arc::clone(&self.host_pending),
                    last_accessed_s: AtomicI64::new(0),
                    freed: AtomicBool::new(false),
                    labels_saved_version: AtomicU32::new(0),
                    meta: RwLock::new(ChartMeta {
                        name: None,
                        family: rrd_string(
                            spec.family.filter(|f| !f.is_empty()).unwrap_or(spec.type_),
                        ),
                        context: rrd_string(
                            spec.context.filter(|c| !c.is_empty()).unwrap_or(&full_id),
                        ),
                        title: rrd_string(spec.title),
                        units: rrd_string(spec.units),
                        plugin,
                        module,
                        priority: spec.priority,
                        update_every: spec.update_every,
                        chart_type: spec.chart_type,
                        flags: flags::SYNC_CLOCK
                            | flags::RECEIVER_REPLICATION_FINISHED
                            | flags::SENDER_REPLICATION_FINISHED,
                        labels,
                    }),
                    collection: Mutex::new(ChartCollection::default()),
                    dims: RwLock::new(Index::default()),
                    receiver: Mutex::new(ReceiverState::default()),
                    variables: Mutex::new(HashMap::new()),
                });
                index.charts.insert(&full_id, Arc::clone(&chart));
                (chart, true, false, false)
            }
        };
        drop(index);
        // rrdset_react_callback(): created or updated, the chart is accessed
        chart.touch_last_accessed();
        if is_new || plugin_or_module {
            chart.set_metadata_update();
        }
        if is_new || changed {
            chart.metadata_updated();
        }
        // Naming, under the index lock as C's name index is.
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        let current = chart.meta().name;
        let requested = spec.name.filter(|n| !n.is_empty()).unwrap_or(spec.id);
        let new_name = match &current {
            None => index
                .fix_name(&full_id, spec.type_, None, spec.name)
                .or_else(|| index.fix_name(&full_id, spec.type_, None, Some(spec.id))),
            // rrdset_reset_name(): nothing to do when the requested name is the current one.
            Some(current) if current == requested => None,
            Some(current) => index.fix_name(&full_id, spec.type_, Some(current), Some(requested)),
        };
        if let Some(new_name) = new_name.filter(|n| current.as_deref() != Some(n.as_str())) {
            if let Some(old) = &current {
                index.by_name.remove(old);
            }
            index.by_name.insert(new_name.clone(), full_id.clone());
            chart.update_meta(|m| m.name = Some(new_name));
            drop(index);
            chart.set_metadata_update();
            // rrdset_reset_name() reports a rename itself; rrdset_create() then reports the name update.
            if current.is_some() {
                chart.metadata_updated();
                contexts::updated_rrdset_name(&chart);
            }
            chart.metadata_updated();
        }
        (chart, is_new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(type_: &'a str, id: &'a str, name: Option<&'a str>) -> ChartSpec<'a> {
        ChartSpec {
            type_,
            id,
            name,
            family: None,
            context: None,
            title: "t",
            units: "u",
            plugin: "fixture-pusher",
            module: Some("corpus"),
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        }
    }

    #[test]
    fn charts_get_c_defaults_and_names() {
        let charts = Charts::default();
        let (a, new) = charts.create(&spec("system", "cpu", None));
        assert!(new);
        let m = a.meta();
        assert_eq!(
            (m.name.as_deref(), m.family.as_str(), m.context.as_str()),
            (Some("system.cpu"), "system", "system.cpu")
        );
        assert_eq!(a.entries(), 4096);
        assert_eq!(
            m.labels.get(b"_collect_plugin"),
            Some(&b"fixture-pusher"[..])
        );
        // Another chart asking for a taken name keeps its id as its name.
        let (b, _) = charts.create(&spec("system", "load", Some("cpu")));
        assert_eq!(b.meta().name.as_deref(), Some("system.load"));
        // Re-creating updates in place.
        let (again, new) = charts.create(&ChartSpec {
            title: "new title",
            ..spec("system", "cpu", None)
        });
        assert!(!new && Arc::ptr_eq(&again, &a));
        assert_eq!(a.meta().title, "new title");
        assert!(charts.find_by_name("system.cpu").is_some());
    }

    #[test]
    fn dimensions_seed_their_rings_from_the_chart() {
        let charts = Charts::default();
        let (chart, _) = charts.create(&spec("t", "c", None));
        chart.update_collection(|c| {
            c.counter = 7;
            c.current_entry = 7;
            c.last_updated = (1_700_000_007, 0);
        });
        let (d, new) = chart.dim_add("d", Some(""), 1, 0, Algorithm::Absolute);
        assert!(new);
        assert_eq!(d.meta().name, "d");
        assert_eq!(d.meta().divisor, 1);
        let ring = d.ring().unwrap();
        assert_eq!(
            (ring.latest_time_s(), ring.oldest_time_s()),
            (1_700_000_007, 1_700_000_000)
        );
        let (same, new) = chart.dim_add("d", Some("renamed"), 2, 1, Algorithm::Incremental);
        assert!(!new && Arc::ptr_eq(&same, &d));
        assert_eq!(
            (d.meta().name.as_str(), d.meta().multiplier),
            ("renamed", 2)
        );
        chart.set_update_every(2);
        assert_eq!((ring.latest_time_s(), ring.update_every_s()), (0, 2));
    }

    #[test]
    fn heterogeneity_compares_with_the_first_dimension() {
        let charts = Charts::default();
        let (chart, _) = charts.create(&spec("t", "h", None));
        chart.dim_add("a", None, 1, 1, Algorithm::Absolute);
        chart.dim_add("b", None, 1, 1, Algorithm::Absolute);
        chart.dim_add("b", None, 5, 1, Algorithm::Absolute);
        // `c` matches `a`, the first dimension; C does not look further.
        chart.dim_add("c", None, 1, 1, Algorithm::Absolute);
        assert_eq!(chart.meta().flags & flags::HETEROGENEOUS, 0);
        chart.dim_add("d", None, 7, 1, Algorithm::Absolute);
        assert_ne!(chart.meta().flags & flags::HETEROGENEOUS, 0);
    }

    /// The obsolete transitions as the maintenance sweep reads them: a chart raises the host's pending charts bit
    /// and touches its last accessed time, a dimension raises its chart's and host's obsolete dimensions bits; an
    /// obsolete chart is found only when obsolete ones are asked for, by id, and a lookup touches it.
    #[test]
    fn obsolete_transitions_raise_the_pending_bits_and_hide_the_chart() {
        let pending = Arc::new(AtomicU32::new(0));
        let charts = Charts::new(Arc::default(), Arc::default(), Arc::clone(&pending), Arc::default(), "");
        let (chart, _) = charts.create(&spec("t", "c", Some("named")));
        assert!(chart.last_accessed_s() > 0, "created: accessed");
        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        chart.last_accessed_s.store(1, Ordering::Relaxed);
        chart.dim_is_obsolete(&dim);
        assert_ne!(chart.flags() & flags::OBSOLETE_DIMENSIONS, 0);
        assert_eq!(pending.swap(0, Ordering::AcqRel), pending_flags::OBSOLETE_DIMENSIONS);
        assert_eq!(chart.last_accessed_s(), 1, "a dimension's transition does not touch");
        chart.is_obsolete();
        assert_eq!(pending.swap(0, Ordering::AcqRel), pending_flags::OBSOLETE_CHARTS);
        assert!(chart.last_accessed_s() > 1, "obsolete: touched");
        chart.is_obsolete();
        assert_eq!(pending.load(Ordering::Acquire), 0, "only the transition raises it");
        assert!(charts.find("t.c", false).is_none());
        assert!(charts.find_by_name("t.named").is_none());
        chart.last_accessed_s.store(1, Ordering::Relaxed);
        assert!(charts.find("t.c", true).is_some_and(|c| c.last_accessed_s() > 1), "found: touched");
        chart.last_accessed_s.store(1, Ordering::Relaxed);
        chart.isnot_obsolete();
        assert!(chart.last_accessed_s() > 1, "back: touched");
        assert!(charts.find("t.c", false).is_some() && charts.find_by_name("t.named").is_some());
    }

    /// `rrddim_delete_callback()`: a dbengine row goes only without retention; the other modes' rows always go.
    #[test]
    fn freed_dimensions_delete_their_rows_as_c() {
        for (mode, has_retention, deletes) in [
            (DbMode::Dbengine, true, false),
            (DbMode::Dbengine, false, true),
            (DbMode::Ram, true, true),
            (DbMode::Alloc, false, true),
            (DbMode::None, true, true),
        ] {
            assert_eq!(
                freed_dimension_deletes(mode, has_retention),
                deletes,
                "{mode:?} {has_retention}"
            );
        }
    }

    /// `rrdset_is_replicating()`: a chart keeps the sender's finished bit without a sender, so a receiver's replication
    /// in progress does not make an obsolete chart discoverable.
    #[test]
    fn a_chart_never_replicates_without_a_sender() {
        let (chart, _) = Charts::default().create(&spec("t", "c", None));
        chart.update_meta(|m| {
            m.flags |= flags::RECEIVER_REPLICATION_IN_PROGRESS | flags::OBSOLETE;
            m.flags &= !flags::RECEIVER_REPLICATION_FINISHED;
        });
        assert!(!flags::is_replicating(chart.flags()));
        assert!(!chart.is_discoverable());
        assert!(flags::is_replicating(flags::RECEIVER_REPLICATION_IN_PROGRESS));
        assert!(flags::is_discoverable(flags::RECEIVER_REPLICATION_IN_PROGRESS | flags::OBSOLETE));
    }

    /// The metadata writer's flags as C raises them: a new chart or dimension, a plugin or module change, a name, a
    /// dimension's name, algorithm, multiplier or divisor, and its `hidden` option against the stored one; not the
    /// other conflict fields. Each raises the host's `UPDATE`.
    #[test]
    fn metadata_flags_follow_cs_setters() {
        let host = Arc::new(AtomicU32::new(0));
        let charts = Charts::new(Arc::default(), Arc::clone(&host), Arc::default(), Arc::default(), "");
        let take_host = || host.swap(0, Ordering::AcqRel) & meta_flags::UPDATE != 0;
        let (chart, _) = charts.create(&spec("t", "c", Some("named")));
        assert!(chart.take_metadata_update() && take_host(), "new chart");
        assert_eq!(chart.name_part(), Some("named"));

        let mut s = spec("t", "c", Some("named"));
        s.title = "other";
        s.units = "other";
        s.priority = 1;
        s.update_every = 2;
        s.family = Some("f");
        s.context = Some("ctx");
        s.chart_type = ChartType::Area;
        charts.create(&s);
        assert!(
            !chart.take_metadata_update() && !take_host(),
            "fields that need no store"
        );
        s.plugin = "other";
        charts.create(&s);
        assert!(chart.take_metadata_update() && take_host(), "plugin");
        s.module = Some("other");
        charts.create(&s);
        assert!(chart.take_metadata_update() && take_host(), "module");
        s.name = Some("renamed");
        charts.create(&s);
        assert!(chart.take_metadata_update() && take_host(), "renamed");
        assert_eq!(chart.name_part(), Some("named"), "the first name stays");
        assert_eq!(charts.create(&spec("t", "x", None)).0.name_part(), None);
        host.store(0, Ordering::Release);

        let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        assert!(take_host(), "new dimension");
        assert_eq!(
            chart.take_dim_metadata_update(&dim).map(|m| m.flags),
            Some(0)
        );
        assert_eq!(chart.take_dim_metadata_update(&dim), None);
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        assert!(
            chart.take_dim_metadata_update(&dim).is_none() && !take_host(),
            "unchanged"
        );
        for (name, multiplier, divisor, algorithm) in [
            (Some("n"), 1, 1, Algorithm::Absolute),
            (Some("n"), 1, 1, Algorithm::Incremental),
            (Some("n"), 2, 1, Algorithm::Incremental),
            (Some("n"), 2, 3, Algorithm::Incremental),
        ] {
            chart.dim_add("d", name, multiplier, divisor, algorithm);
            assert!(chart.take_dim_metadata_update(&dim).is_some() && take_host());
        }

        chart.dim_set_hidden(&dim, true);
        let stored = chart.take_dim_metadata_update(&dim).map(|m| m.flags);
        assert_eq!(stored, Some(dim_flags::HIDDEN | dim_flags::META_HIDDEN));
        assert!(take_host());
        chart.dim_set_hidden(&dim, true);
        assert!(
            chart.take_dim_metadata_update(&dim).is_none() && !take_host(),
            "stored hidden"
        );
        chart.dim_set_hidden(&dim, false);
        assert_eq!(
            chart.take_dim_metadata_update(&dim).map(|m| m.flags),
            Some(0)
        );
        assert!(take_host());
    }

    #[test]
    fn sql_ids_are_cs() {
        let algorithms = [
            Algorithm::Absolute,
            Algorithm::Incremental,
            Algorithm::PcentOverDiffTotal,
            Algorithm::PcentOverRowTotal,
        ];
        for (id, a) in algorithms.into_iter().enumerate() {
            assert_eq!((a.id(), Algorithm::from_id(a.id())), (id as i32, a));
        }
        let types = [
            ChartType::Line,
            ChartType::Area,
            ChartType::Stacked,
            ChartType::Heatmap,
        ];
        for (id, t) in types.into_iter().enumerate() {
            assert_eq!((t.id(), ChartType::from_id(t.id())), (id as i32, t));
        }
    }

    /// C compares the sanitized strings (`rrdset_metadata_field_update()`): a chart defined again with a plugin or a
    /// title that sanitizing changes is unchanged.
    #[test]
    fn a_redefinition_compares_sanitized_fields() {
        let host = Arc::new(AtomicU32::new(0));
        let charts = Charts::new(Arc::default(), Arc::clone(&host), Arc::default(), Arc::default(), "");
        let mut s = spec("t", "c", None);
        s.plugin = " spaced  plugin ";
        s.title = " spaced  title ";
        s.module = Some(" spaced  module ");
        let (chart, _) = charts.create(&s);
        assert!(chart.take_metadata_update());
        charts.create(&s);
        assert!(!chart.take_metadata_update(), "the same plugin and module");
    }
}
