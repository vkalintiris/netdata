//! Charts and dimensions, ported from `src/database/rrdset-index-id.c`, `rrdset-index-name.c`, `rrdset-collection.c`
//! (`rrdset_set_update_every_s()`) and `rrddim.c`: the per-host chart index with C's insert/conflict rules and chart
//! naming, and the dimensions with their tier-0 ring.
//!
//! One collector (a stream thread) changes a chart while queries read it: the mutable state sits behind locks that
//! are held only for short, bounded steps.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock, Weak};

use netdata_agent_storage::ram::{ALLOC_MIN_ENTRIES, RamMetric, Seed};
use netdata_agent_text::sanitize::{rrd_string_sanitize, rrdset_strncpyz_name};

use crate::contexts::{self, ChartLink, Contexts, DimLink};
use crate::labels::{FLAG_DONT_DELETE, Labels, SRC_AUTO};
use crate::mode::{DbMode, align_entries_to_pagesize};

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
    meta: RwLock<ChartMeta>,
    collection: Mutex<ChartCollection>,
    dims: RwLock<DimIndex>,
    receiver: Mutex<ReceiverState>,
    /// Chart variables (`VARIABLE CHART`), used by health.
    variables: Mutex<HashMap<String, f64>>,
}

#[derive(Debug, Default)]
struct DimIndex {
    ordered: Vec<Arc<Dim>>,
    by_id: HashMap<String, usize>,
}

impl Chart {
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

    /// `rrdset_is_obsolete___safe_from_collector_thread()`, without the sender's parts.
    pub fn is_obsolete(&self) {
        let was = self.update_meta(|m| {
            let was = m.flags;
            m.flags |= flags::OBSOLETE;
            was
        });
        if was & flags::OBSOLETE == 0 {
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
            contexts::updated_rrddim_flags(dim);
            self.metadata_updated();
        }
    }

    /// `rrddim_isnot_obsolete___safe_from_collector_thread()`.
    pub fn dim_isnot_obsolete(&self, dim: &Dim) {
        let was = dim.update_meta(|m| {
            let was = m.flags;
            m.flags &= !dim_flags::OBSOLETE;
            was
        });
        if was & dim_flags::OBSOLETE != 0 {
            contexts::updated_rrddim_flags(dim);
            self.metadata_updated();
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
            .ordered
            .len()
    }

    /// `rrdset_first_entry_s_of_tier(st, 0)` and `rrdset_last_entry_s_of_tier(st, 0)`: the oldest and newest time any
    /// dimension's ring holds (0 when none).
    pub fn tier0_retention(&self) -> (i64, i64) {
        let mut first = 0;
        let mut last = 0;
        for dim in self.dims() {
            if let Some(ring) = dim.ring() {
                let (f, l) = (ring.oldest_time_s(), ring.latest_time_s());
                if f != 0 && (first == 0 || f < first) {
                    first = f;
                }
                if l > last {
                    last = l;
                }
            }
        }
        (first, last)
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
            .ordered
            .clone()
    }

    pub fn dim(&self, id: &str) -> Option<Arc<Dim>> {
        let dims = self.dims.read().unwrap_or_else(PoisonError::into_inner);
        dims.by_id.get(id).map(|&i| Arc::clone(&dims.ordered[i]))
    }

    /// `rrdset_set_update_every_s()`: an invalid value is ignored; a change flushes every ring. Returns the value
    /// in force afterwards.
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
            if let Some(ring) = &dim.ring {
                ring.change_update_every(update_every);
            }
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
        if let Some(&i) = index.by_id.get(id) {
            let dim = Arc::clone(&index.ordered[i]);
            drop(index);
            self.dim_isnot_obsolete(&dim);
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
                self.update_meta(|m| m.flags |= flags::SYNC_CLOCK | flags::HOMOGENEOUS_CHECK);
                self.dim_metadata_updated(&dim);
            }
            return (dim, false);
        }
        let meta = self.meta();
        let collection = self.collection();
        let ring = match self.mode {
            DbMode::Ram | DbMode::Alloc | DbMode::None => {
                let entries = if self.mode == DbMode::Ram {
                    self.entries.max(1)
                } else {
                    self.entries.max(ALLOC_MIN_ENTRIES)
                };
                Some(RamMetric::new(
                    entries,
                    Seed {
                        counter: collection.counter,
                        current_entry: collection.current_entry,
                        last_updated_s: collection.last_updated.0,
                        update_every_s: i64::from(meta.update_every),
                    },
                ))
            }
            DbMode::Dbengine => None,
        };
        // rrdcontext_find_dimension_uuid(): a dimension created again keeps its metric's UUID
        let uuid = self
            .host_contexts
            .find_dimension_uuid(&context, &self.id, id)
            .unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());
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
                flags: 0,
            }),
            collection: Mutex::new(DimCollection {
                counter: usize::from(meta.flags & flags::STORE_FIRST != 0),
                ..DimCollection::default()
            }),
            ring,
        });
        // C compares with the first other dimension only (the loop breaks after it).
        let heterogeneous = index.ordered.first().is_some_and(|td| {
            let t = td.meta();
            t.algorithm != algorithm
                || i64::from(t.multiplier).abs() != i64::from(multiplier).abs()
                || i64::from(t.divisor).abs() != i64::from(divisor).abs()
        });
        let position = index.ordered.len();
        index.by_id.insert(id.to_string(), position);
        index.ordered.push(Arc::clone(&dim));
        drop(index);
        self.update_meta(|m| {
            m.flags |= flags::SYNC_CLOCK;
            if heterogeneous {
                m.flags |= flags::HETEROGENEOUS;
            }
        });
        if dim.ring.is_some() {
            self.host_contexts().ram_index().register(&dim);
        }
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
    /// Tier 0 of a ram/alloc/none chart; dbengine storage comes with slice 2.
    ring: Option<RamMetric>,
}

impl Dim {
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

    pub fn ring(&self) -> Option<&RamMetric> {
        self.ring.as_ref()
    }

    pub(crate) fn weak(&self) -> Weak<Dim> {
        self.me.clone()
    }

    /// The dimension's metric.
    pub fn contexts(&self) -> &DimLink {
        &self.link
    }

    /// `rrddim_first_entry_s()`: the oldest point of any tier (one tier until dbengine, D15).
    pub fn first_entry_s(&self) -> i64 {
        self.ring.as_ref().map_or(0, RamMetric::oldest_time_s)
    }

    /// `rrddim_last_entry_s()`.
    pub fn last_entry_s(&self) -> i64 {
        self.ring.as_ref().map_or(0, RamMetric::latest_time_s)
    }

    /// `rrddim_store_metric()` at tier 0; the first store after a (re)link marks the metric collected.
    pub fn store_metric(&self, point_end_time_ut: u64, value: f64, flags: u32) {
        if let Some(ring) = &self.ring {
            ring.store(point_end_time_ut, value, flags);
        }
        contexts::collected_rrddim(self);
    }
}

/// A host's charts: by id, by name, and in creation order (`rrdset_root_index`, `rrdset_root_index_name`).
#[derive(Debug, Default)]
pub struct Charts {
    inner: RwLock<ChartIndex>,
    /// The host's contexts, which every chart reports to.
    contexts: Arc<Contexts>,
}

#[derive(Debug, Default)]
struct ChartIndex {
    ordered: Vec<Arc<Chart>>,
    by_id: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
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
    pub fn new(contexts: Arc<Contexts>) -> Self {
        Charts {
            inner: RwLock::default(),
            contexts,
        }
    }

    pub fn find(&self, id: &str) -> Option<Arc<Chart>> {
        let index = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        index.by_id.get(id).map(|&i| Arc::clone(&index.ordered[i]))
    }

    pub fn find_by_name(&self, name: &str) -> Option<Arc<Chart>> {
        let index = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        index
            .by_name
            .get(name)
            .map(|&i| Arc::clone(&index.ordered[i]))
    }

    pub fn all(&self) -> Vec<Arc<Chart>> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .ordered
            .clone()
    }

    /// `rrdset_create()`: a new chart, or the existing one un-obsoleted and updated by the conflict rules, then
    /// named. Returns the chart and whether it is new.
    pub fn create(&self, spec: &ChartSpec<'_>) -> (Arc<Chart>, bool) {
        let full_id = bounded(format!("{}.{}", spec.type_, spec.id), ID_LENGTH_MAX);
        if let Some(existing) = self.find(&full_id) {
            existing.isnot_obsolete();
        }
        let mut index = self.inner.write().unwrap_or_else(PoisonError::into_inner);
        // rrdset_conflict_callback() reports whether anything changed; the react step then runs.
        let (chart, is_new, changed) = match index.by_id.get(&full_id) {
            Some(&i) => {
                let chart = Arc::clone(&index.ordered[i]);
                let mut changed = chart.update_meta(|m| {
                    let mut changed = false;
                    if m.priority != spec.priority {
                        m.priority = spec.priority;
                        changed = true;
                    }
                    for (field, value) in [
                        (&mut m.plugin, Some(spec.plugin)),
                        (&mut m.module, spec.module),
                        (&mut m.title, Some(spec.title)),
                        (&mut m.units, Some(spec.units)),
                        (&mut m.family, spec.family),
                        (&mut m.context, spec.context),
                    ] {
                        // rrdset_metadata_field_update(): only non-empty values that differ.
                        if let Some(value) = value.filter(|v| !v.is_empty() && *v != field.as_str())
                        {
                            *field = rrd_string(value);
                            changed = true;
                        }
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
                    changed
                });
                if chart.update_every() != spec.update_every {
                    chart.set_update_every(i64::from(spec.update_every));
                    changed = true;
                }
                (chart, false, changed)
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
                    dims: RwLock::new(DimIndex::default()),
                    receiver: Mutex::new(ReceiverState::default()),
                    variables: Mutex::new(HashMap::new()),
                });
                let position = index.ordered.len();
                index.by_id.insert(full_id.clone(), position);
                index.ordered.push(Arc::clone(&chart));
                (chart, true, false)
            }
        };
        drop(index);
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
            let position = index.by_id[&full_id];
            index.by_name.insert(new_name.clone(), position);
            chart.update_meta(|m| {
                m.name = Some(new_name);
                m.flags |= flags::METADATA_UPDATE;
            });
            drop(index);
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
}
