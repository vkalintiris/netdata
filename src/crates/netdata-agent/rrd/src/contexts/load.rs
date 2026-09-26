//! `rrdhost_load_rrdcontext_data()` (`src/database/contexts/rrdcontext-loading.c`): a host's contexts, instances
//! and metrics from the metadata databases, as archived objects, in C's order and with C's counts and record
//! (decisions D59.3, D60). Rows stream in (contexts, then charts, then dimensions), as C's callbacks take them; the
//! daemon reads them and maps C's integer ids, so this crate knows no SQL.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use netdata_agent_log::{Priority, Source, nd_log};

use super::{
    ContextState, Contexts, Flags, Hub, Index, Instance, InstanceState, Metric, MetricState, flags,
    lock, post_process_updates,
};
use crate::chart::{Algorithm, ChartType};
use crate::labels::Labels;

/// A row of the context database's `context` table (`VERSIONED_CONTEXT_DATA`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SqlContext {
    pub id: Option<String>,
    pub version: u64,
    pub title: Option<Vec<u8>>,
    /// The type's name, as stored.
    pub chart_type: Option<String>,
    pub units: Option<String>,
    pub priority: u64,
    pub first_time_s: u64,
    pub last_time_s: u64,
    pub deleted: bool,
    pub family: Option<Vec<u8>>,
}

/// A row of `chart` (`SQL_CHART_DATA`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlChart {
    pub chart_id: [u8; 16],
    /// `type.id`.
    pub id: Option<String>,
    pub name: Option<String>,
    pub context: Option<String>,
    pub title: Option<String>,
    pub units: Option<String>,
    pub priority: i32,
    pub update_every: i32,
    pub chart_type: ChartType,
    pub family: Option<String>,
}

/// A row of `dimension` joined with its chart (`SQL_DIMENSION_DATA`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlDim {
    pub dim_id: [u8; 16],
    pub id: Option<String>,
    pub name: Option<String>,
    pub hidden: bool,
    /// The chart's `type.id`.
    pub chart_id: Option<String>,
    pub context: Option<String>,
    pub algorithm: Algorithm,
}

/// The outcome of a load: C's counts, and the contexts left without instances, which C queues for the metadata
/// writer to clean up (`metadata_queue_ctx_host_cleanup()`), and those the garbage collection deleted, whose rows C
/// deletes from the context database.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoadReport {
    pub contexts: usize,
    pub contexts_deleted: usize,
    pub instances: usize,
    pub instances_deleted: usize,
    pub instances_ignored: usize,
    pub metrics: usize,
    pub metrics_ignored: usize,
    pub metrics_zero_retention: usize,
    pub cleanup: Vec<String>,
    pub deleted_from_sql: Vec<String>,
}

/// A load in progress; see [`Contexts::loader`].
#[derive(Debug)]
pub struct Loader<'a> {
    contexts: &'a Contexts,
    instances_ignored: usize,
    metrics_ignored: usize,
    metrics_zero_retention: usize,
}

/// `string_strdupz()`: NULL and empty are both no string.
fn non_empty(s: &Option<String>) -> Option<&str> {
    s.as_deref().filter(|s| !s.is_empty())
}

impl Contexts {
    /// The loader of this tree, once: `None` when it was loaded before (C's contexts dictionary exists).
    pub fn loader(&self) -> Option<Loader<'_>> {
        if self.loaded.swap(true, Ordering::AcqRel) {
            return None;
        }
        Some(Loader {
            contexts: self,
            instances_ignored: 0,
            metrics_ignored: 0,
            metrics_zero_retention: 0,
        })
    }
}

impl Loader<'_> {
    /// `rrdcontext_load_context_callback()`: a context as Netdata Cloud last saw it. Rows without a version are
    /// skipped, and so are rows without an id (the dictionary refuses them).
    pub fn context(&mut self, row: &SqlContext) {
        let Some(id) = non_empty(&row.id) else {
            return;
        };
        if row.version == 0 || self.contexts.get(id).is_some() {
            return;
        }
        let text = |v: &Option<Vec<u8>>| v.clone().unwrap_or_default();
        let chart_type = ChartType::from_name(row.chart_type.as_deref().unwrap_or("").as_bytes());
        let hub = Hub {
            version: row.version,
            id: Some(id.to_string()),
            title: Some(text(&row.title)),
            chart_type: Some(chart_type.name().to_string()),
            units: Some(row.units.clone().unwrap_or_default()),
            family: Some(text(&row.family)),
            priority: row.priority,
            first_time_s: row.first_time_s,
            last_time_s: row.last_time_s,
            deleted: row.deleted,
        };
        let state = ContextState {
            version: row.version,
            title: text(&row.title),
            units: row.units.clone().unwrap_or_default(),
            family: text(&row.family),
            priority: row.priority as u32,
            chart_type,
            first_time_s: row.first_time_s as i64,
            last_time_s: row.last_time_s as i64,
            hub,
        };
        let rc = self
            .contexts
            .new_context(id, state, flags::ARCHIVED | flags::REASON_LOAD_SQL);
        if row.deleted || row.first_time_s == 0 {
            rc.flags.set_deleted(0);
        } else if row.last_time_s == 0 {
            rc.flags.set_collected();
        } else {
            rc.flags.set_archived();
        }
        rc.flags.set(flags::REASON_LOAD_SQL);
        rc.flags.set_updated(flags::REASON_NEW_OBJECT);
        lock(&self.contexts.index).insert(id, Arc::clone(&rc));
        self.contexts.version.fetch_add(1, Ordering::Relaxed);
        rc.trigger_updates();
    }

    /// `rrdinstance_load_instance_callback()`: the chart's context (archived metadata) and its instance. A row
    /// without a context or a chart id is an ignored instance.
    pub fn chart(&mut self, row: &SqlChart) {
        let Some(context) = non_empty(&row.context) else {
            self.instances_ignored += 1;
            return;
        };
        let text = |v: &Option<String>| v.clone().unwrap_or_default();
        let rc = self.contexts.upsert_context(context, true, |state| {
            state.title = text(&row.title).into_bytes();
            state.units = text(&row.units);
            state.family = text(&row.family).into_bytes();
            state.priority = row.priority as u32;
            state.chart_type = row.chart_type;
        });
        let Some(id) = non_empty(&row.id) else {
            self.instances_ignored += 1;
            return;
        };
        let priority = (row.priority as u32) & 0x00FF_FFFF;
        let mut instances = lock(&rc.instances);
        match instances.get(id) {
            None => {
                let ri = Arc::new(Instance {
                    id: id.to_string(),
                    context: Arc::downgrade(&rc),
                    flags: Flags(AtomicU32::new(
                        flags::ARCHIVED
                            | flags::REASON_LOAD_SQL
                            | flags::OWN_LABELS
                            | flags::DEMAND_LABELS,
                    )),
                    state: Mutex::new(InstanceState {
                        uuid: row.chart_id,
                        name: non_empty(&row.name).unwrap_or(id).to_string(),
                        title: text(&row.title),
                        units: text(&row.units),
                        family: text(&row.family),
                        chart_type: row.chart_type,
                        priority,
                        update_every_s: row.update_every,
                        first_time_s: 0,
                        last_time_s: 0,
                        own_labels: Labels::default(),
                    }),
                    chart: Mutex::new(None),
                    metrics: Mutex::new(Index::default()),
                    collected_metrics_count: AtomicU32::new(0),
                });
                ri.flags.set_updated(flags::REASON_NEW_OBJECT);
                instances.insert(id, Arc::clone(&ri));
                drop(instances);
                ri.trigger_updates();
            }
            Some(ri) => {
                drop(instances);
                if instance_conflict_row(&ri, row, priority) {
                    ri.trigger_updates();
                }
            }
        }
    }

    /// `rrdinstance_load_dimension_callback()`: a metric of an instance loaded before, when some tier holds data for
    /// it; one without retention counts as such, one whose context, instance or id is missing is ignored.
    pub fn dim(&mut self, row: &SqlDim) {
        let (first, last, _) = self.contexts.metric_retention(&row.dim_id);
        if (first == 0 || first == i64::MAX) && last == 0 {
            self.metrics_zero_retention += 1;
            return;
        }
        let ri = non_empty(&row.context)
            .and_then(|context| self.contexts.get(context))
            .and_then(|rc| non_empty(&row.chart_id).and_then(|chart| rc.instance(chart)));
        let (Some(ri), Some(id)) = (ri, non_empty(&row.id)) else {
            self.metrics_ignored += 1;
            return;
        };
        let mut metrics = lock(&ri.metrics);
        match metrics.get(id) {
            None => {
                let mut initial = flags::ARCHIVED | flags::REASON_LOAD_SQL;
                if row.hidden {
                    initial |= flags::HIDDEN;
                }
                let rm = Arc::new(Metric {
                    id: id.to_string(),
                    instance: Arc::downgrade(&ri),
                    flags: Flags(AtomicU32::new(initial)),
                    state: Mutex::new(MetricState {
                        uuid: row.dim_id,
                        name: row.name.clone().unwrap_or_default(),
                        algorithm: row.algorithm,
                        first_time_s: 0,
                        last_time_s: 0,
                    }),
                    dim: Mutex::new(None),
                });
                rm.flags.set_updated(flags::REASON_NEW_OBJECT);
                metrics.insert(id, Arc::clone(&rm));
                drop(metrics);
                rm.trigger_updates();
            }
            Some(rm) => {
                drop(metrics);
                if self.metric_conflict_row(&rm, row) {
                    rm.trigger_updates();
                }
            }
        }
    }

    /// `rrdmetric_conflict_callback()` for a second row of the same dimension id: the later row's UUID wins, with
    /// both retentions merged.
    fn metric_conflict_row(&self, rm: &Metric, row: &SqlDim) -> bool {
        let (new_first, new_last) = if lock(&rm.state).uuid == row.dim_id {
            (0, 0)
        } else {
            rm.update_retention();
            let (first, last, _) = self.contexts.metric_retention(&row.dim_id);
            let first = if first == i64::MAX { 0 } else { first };
            let (first, last) = if first > last {
                (last, first)
            } else {
                (first, last)
            };
            let mut state = lock(&rm.state);
            state.uuid = row.dim_id;
            rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
            drop(state);
            (first, last)
        };
        let mut state = lock(&rm.state);
        let name = row.name.clone().unwrap_or_default();
        if state.name != name {
            state.name = name;
            rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
        if state.algorithm != row.algorithm {
            state.algorithm = row.algorithm;
            rm.flags.set_updated(flags::REASON_CHANGED_METADATA);
        }
        if state.first_time_s == 0 || (new_first != 0 && new_first < state.first_time_s) {
            state.first_time_s = new_first;
            rm.flags.set_updated(flags::REASON_CHANGED_FIRST_TIME_T);
        }
        if state.last_time_s == 0 || (new_last != 0 && new_last > state.last_time_s) {
            state.last_time_s = new_last;
            rm.flags.set_updated(flags::REASON_CHANGED_LAST_TIME_T);
        }
        drop(state);
        let mut newcomer = flags::ARCHIVED | flags::REASON_LOAD_SQL;
        if row.hidden {
            newcomer |= flags::HIDDEN;
        }
        rm.flags.set(newcomer);
        if rm.flags.is_collected() && rm.flags.is_archived() {
            rm.flags.set_collected();
        }
        if rm.flags.is_updated() {
            rm.flags.set(flags::REASON_UPDATED_OBJECT);
        }
        rm.flags.is_updated()
    }

    /// The rest of `rrdhost_load_rrdcontext_data()`: every loaded object's updates triggered, instances without
    /// metrics and contexts without instances removed (the contexts for the metadata writer to clean up), the
    /// others post-processed once, then the garbage collection and C's record. `exiting` stops the pass between
    /// contexts, as C's exit check does.
    pub fn finish(self, hostname: &str, exiting: impl Fn() -> bool) -> LoadReport {
        let contexts = self.contexts;
        let mut report = LoadReport {
            instances_ignored: self.instances_ignored,
            metrics_ignored: self.metrics_ignored,
            metrics_zero_retention: self.metrics_zero_retention,
            ..LoadReport::default()
        };
        for rc in contexts.all() {
            if exiting() {
                break;
            }
            let mut kept = 0;
            for ri in rc.instances() {
                let metrics = ri.metrics();
                for rm in &metrics {
                    rm.trigger_updates();
                    report.metrics += 1;
                }
                if metrics.is_empty() {
                    lock(&rc.instances).retain(|i| !Arc::ptr_eq(i, &ri));
                    report.instances_deleted += 1;
                } else {
                    ri.trigger_updates();
                    report.instances += 1;
                    kept += 1;
                }
            }
            if kept == 0 {
                // rrdcontext_delete_after_loading()
                report.cleanup.push(rc.id().to_string());
                contexts.remove_context(&rc);
                report.contexts_deleted += 1;
            } else {
                rc.trigger_updates();
                // rrdcontext_initial_processing_after_loading()
                {
                    let mut q = lock(&contexts.queue.inner);
                    super::PpQueue::del_locked(&mut q, &rc);
                }
                post_process_updates(&rc, false, 0);
                report.contexts += 1;
            }
        }
        report.deleted_from_sql = contexts.garbage_collect(true);
        let priority = if report.metrics_ignored != 0 || report.instances_ignored != 0 {
            Priority::Warning
        } else {
            Priority::Notice
        };
        nd_log!(
            Source::Daemon,
            priority,
            "RRDCONTEXT: metadata for node '{hostname}': contexts {} (deleted {}), instances {} (deleted {}, ignored \
             {}), and metrics {} (ignored {}, zero retention {})",
            report.contexts,
            report.contexts_deleted,
            report.instances,
            report.instances_deleted,
            report.instances_ignored,
            report.metrics,
            report.metrics_ignored,
            report.metrics_zero_retention
        );
        report
    }
}

/// `rrdinstance_conflict_callback()` for a second row of the same chart id: the later row's UUID and metadata win;
/// the chart link and labels stay.
fn instance_conflict_row(ri: &Instance, row: &SqlChart, priority: u32) -> bool {
    let mut guard = lock(&ri.state);
    let state = &mut *guard;
    let changed = |ri: &Instance| ri.flags.set_updated(flags::REASON_CHANGED_METADATA);
    if state.uuid != row.chart_id {
        state.uuid = row.chart_id;
        changed(ri);
    }
    // the newcomer's strings as C holds them: a missing one is empty
    let text = |v: &Option<String>| v.clone().unwrap_or_default();
    for (field, new) in [
        (&mut state.name, text(&row.name)),
        (&mut state.title, text(&row.title)),
        (&mut state.units, text(&row.units)),
        (&mut state.family, text(&row.family)),
    ] {
        if *field != new {
            *field = new;
            changed(ri);
        }
    }
    if state.chart_type != row.chart_type {
        state.chart_type = row.chart_type;
        changed(ri);
    }
    if state.priority != priority {
        state.priority = priority;
        changed(ri);
    }
    if state.update_every_s != row.update_every {
        state.update_every_s = row.update_every;
        changed(ri);
    }
    drop(guard);
    ri.flags.set(flags::ARCHIVED | flags::REASON_LOAD_SQL);
    if ri.flags.is_collected() && ri.flags.is_archived() {
        ri.flags.set_collected();
    }
    if ri.flags.is_updated() {
        ri.flags.set(flags::REASON_UPDATED_OBJECT);
    }
    ri.flags.is_updated()
}
