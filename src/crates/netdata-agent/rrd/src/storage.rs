//! The hosts' storage (`host->db[]`, set up in `rrdhost_create()` from `nd_profile.storage_tiers` and `multidb_ctx`):
//! the dbengine tiers when the engine runs. Brief `knowledge/brief-dbengine-s2-query-map.md` §3 in the status
//! repository; D64.

use std::sync::{Arc, OnceLock};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
use netdata_agent_storage::dbengine::engine::mrg::{Handle, Mrg};
use netdata_agent_storage::dbengine::engine::query::Dbengine;
use netdata_agent_storage::query::{Priority, StorageQuery};

use crate::chart::{Chart, Dim};
use crate::host::Host;
use crate::contexts::{DbRotation, ExtremeCardinality, RamIndex, TierRetention};
use crate::mode::DbMode;
use crate::pulse::Pulse;

/// `storage_tiers_grouping_iterations` before the configuration: tier 0 the update every, the others 60.
const GROUPING_ITERATIONS: [u64; RRD_STORAGE_TIERS] = [1, 60, 60, 60, 60];

/// `RRD_BACKFILL` (`[db] dbengine tier backfill`): what a tier collected for the first time takes from the tiers
/// below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backfill {
    /// Only a tier that already has data.
    #[default]
    New,
    Full,
    None,
}

/// `nd_profile` (`storage_tiers`, `update_every`) with `multidb_ctx` and `storage_tiers_grouping_iterations`: the
/// dbengine, when it runs, its tiers in use and their grouping.
#[derive(Debug)]
pub struct StorageLayout {
    dbengine: Option<Arc<Dbengine>>,
    grouping_iterations: Vec<u64>,
    update_every: i64,
    /// `default_backfill`.
    backfill: Backfill,
    /// `backfill_globals`: the BACKFILL queue of every host of the daemon.
    backfill_queue: crate::backfill::BackfillQueue,
    /// `global_rrdset_counter`: the charts created by every host of the daemon.
    charts_created: AtomicUsize,
    /// `rrdcontext_next_db_rotation_ut`: the deadline the engine's rotations arm for the contexts' deep pass.
    db_rotation: Arc<DbRotation>,
    /// `extreme_cardinality`: the protection's settings.
    extreme_cardinality: ExtremeCardinality,
    /// The counters of the pulse charts.
    pulse: Pulse,
    /// `metaqueue_delete_dimension_uuid()`: what removes a freed dimension's metadata row, installed by the daemon.
    freed_dimension_row: OnceLock<DimensionRowHook>,
    /// What health does when a chart or a host goes, installed by the daemon.
    health_hook: OnceLock<HealthHook>,
    alert_view: OnceLock<AlertViewHook>,
    /// `health_evloop_iteration`: the passes of the HEALTH loop.
    health_iteration: AtomicU64,
    /// The `[db]` cleanup times the maintenance and its readers share.
    cleanup: OnceLock<CleanupTimes>,
}

/// `rrdset_free_obsolete_time_s`, `rrdhost_cleanup_orphan_to_archive_time_s` and `rrdhost_free_ephemeral_time_s`
/// (`[db] cleanup obsolete charts after`, `cleanup orphan hosts after`, `cleanup ephemeral hosts after`), seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CleanupTimes {
    pub obsolete_charts_s: i64,
    pub orphan_hosts_s: i64,
    /// 0: ephemeral hosts are not freed.
    pub ephemeral_hosts_s: i64,
}

impl Default for CleanupTimes {
    /// C's defaults.
    fn default() -> Self {
        CleanupTimes {
            obsolete_charts_s: 3600,
            orphan_hosts_s: 3600,
            ephemeral_hosts_s: 0,
        }
    }
}

/// A freed dimension's UUID to the metadata writer.
struct DimensionRowHook(Box<dyn Fn([u8; 16]) + Send + Sync>);

impl std::fmt::Debug for DimensionRowHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DimensionRowHook")
    }
}

/// What health hears from the database. C calls `rrdcalc.c` from these places directly, on the object being
/// freed; here health is above this crate, so the daemon installs what answers. A chart index knows its host by
/// machine GUID only.
#[derive(Debug, Clone, Copy)]
pub enum HealthEvent<'a> {
    /// `rrdset_delete_callback()`: this chart of that host was freed.
    ChartFreed(&'a str, &'a Chart),
    /// `rrdhost_cleanup_data_collection_and_health()`: this host's charts are about to be freed.
    HostCleanup(&'a Host),
    /// The same cleanup, once the charts are gone: C destroys the host's alert index and frees its alert log then.
    HostChartsFlushed(&'a Host),
    /// `rrdhost_free_unlinked()`: this host is gone and its data collection was cleaned up. It need not be the host
    /// its GUID names in the index: a host that found its GUID taken there is freed the same way.
    HostFreed(&'a Host),
}

struct HealthHook(Box<dyn Fn(HealthEvent<'_>) + Send + Sync>);

impl std::fmt::Debug for HealthHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HealthHook")
    }
}

/// What a query sees of health: C's query target reads the host's alert dictionary and each chart's alert list
/// directly (`database/contexts/query_target.c`); here health is above this crate, so the daemon installs the view.
pub trait AlertView: Send + Sync {
    /// `dictionary_version(host->rrdcalc_root_index)` and `host->health_transitions`: what a data answer prints as
    /// `alerts_hard_hash` and `alerts_soft_hash`. Zeros for a host health never ran for.
    fn versions(&self, host: &Host) -> (u64, u64);
    /// The alerts linked to this chart object, in link order (`st->alerts.base`).
    fn chart_alerts(&self, host: &Host, chart: &Chart) -> Vec<ChartAlert>;
}

/// One alert of a chart as a query sees it: its rule's name and units, and its published status and value.
#[derive(Debug, Clone, PartialEq)]
pub struct ChartAlert {
    pub name: Vec<u8>,
    /// How a data answer counts the status.
    pub class: AlertClass,
    /// `rrdcalc_status2string()` of the status, which the `alerts=` filter matches as `NAME:STATUS`.
    pub status_name: &'static str,
    /// The status is CLEAR or above: the detailed tree of a data answer shows the alert.
    pub at_least_clear: bool,
    pub value: f64,
    /// The rule's units, empty for none.
    pub units: Vec<u8>,
}

/// The four counters of `QUERY_ALERTS_COUNTS`: every status that is not CLEAR, WARNING or CRITICAL is `Other`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertClass {
    Clear,
    Warning,
    Critical,
    Other,
}

struct AlertViewHook(Arc<dyn AlertView>);

impl std::fmt::Debug for AlertViewHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AlertViewHook")
    }
}

impl Default for StorageLayout {
    fn default() -> Self {
        StorageLayout::new(None)
    }
}

impl StorageLayout {
    /// The layout with C's default grouping and update every.
    pub fn new(dbengine: Option<Arc<Dbengine>>) -> Self {
        StorageLayout {
            dbengine,
            grouping_iterations: GROUPING_ITERATIONS.to_vec(),
            update_every: 1,
            backfill: Backfill::New,
            backfill_queue: crate::backfill::BackfillQueue::default(),
            charts_created: AtomicUsize::new(0),
            db_rotation: Arc::default(),
            extreme_cardinality: ExtremeCardinality::default(),
            pulse: Pulse::default(),
            freed_dimension_row: OnceLock::new(),
            health_hook: OnceLock::new(),
            alert_view: OnceLock::new(),
            health_iteration: AtomicU64::new(0),
            cleanup: OnceLock::new(),
        }
    }

    /// Sets the cleanup times the configuration gives; once.
    pub fn set_cleanup_times(&self, times: CleanupTimes) {
        let _ = self.cleanup.set(times);
    }

    /// The cleanup times, C's defaults until set.
    pub fn cleanup_times(&self) -> CleanupTimes {
        self.cleanup.get().copied().unwrap_or_default()
    }

    /// `health_evloop_current_iteration()`.
    pub fn health_iteration(&self) -> u64 {
        self.health_iteration.load(Ordering::Relaxed)
    }

    /// A pass of the HEALTH loop begins; its number.
    pub fn next_health_iteration(&self) -> u64 {
        self.health_iteration.fetch_add(1, Ordering::Relaxed) + 1
    }

    /// Installs what health does when a chart or a host goes; once.
    pub fn set_health_hook(&self, hook: impl Fn(HealthEvent<'_>) + Send + Sync + 'static) {
        let _ = self.health_hook.set(HealthHook(Box::new(hook)));
    }

    /// Tells health, when it listens. The caller holds no index lock.
    pub(crate) fn health_event(&self, event: HealthEvent<'_>) {
        if let Some(hook) = self.health_hook.get() {
            (hook.0)(event);
        }
    }

    /// Installs what queries see of the hosts' alerts; once.
    pub fn set_alert_view(&self, view: Arc<dyn AlertView>) {
        let _ = self.alert_view.set(AlertViewHook(view));
    }

    /// What queries see of the hosts' alerts; none while health is not there.
    pub fn alert_view(&self) -> Option<&dyn AlertView> {
        self.alert_view.get().map(|view| &*view.0)
    }

    /// Installs what removes a freed dimension's metadata row; once.
    pub fn set_freed_dimension_hook(&self, hook: impl Fn([u8; 16]) + Send + Sync + 'static) {
        let _ = self.freed_dimension_row.set(DimensionRowHook(Box::new(hook)));
    }

    /// A freed dimension leaves no data behind: its metadata row goes.
    pub(crate) fn freed_dimension_row(&self, uuid: &[u8; 16]) {
        if let Some(hook) = self.freed_dimension_row.get() {
            (hook.0)(*uuid);
        }
    }

    /// `rrdset_collection_modulo_init()`: a new chart's number, which spreads its tier writes over time.
    pub fn next_collection_modulo(&self) -> u16 {
        (self.charts_created.fetch_add(1, Ordering::Relaxed)
            % crate::tiers::COLLECTION_MODULO_RANGE) as u16
    }

    /// The configured grouping iterations (`[db] dbengine tier N update every iterations`, tier 0 first) and
    /// `nd_profile.update_every`.
    pub fn with_profile(mut self, grouping_iterations: Vec<u64>, update_every: i64) -> Self {
        self.grouping_iterations = grouping_iterations;
        self.update_every = update_every;
        self
    }

    /// `default_backfill`, as `netdata_conf_dbengine_init()` reads it.
    pub fn with_backfill(mut self, backfill: Backfill) -> Self {
        self.backfill = backfill;
        self
    }

    /// The deep pass's deadline, which the engine's rotation hook arms: the engine starts before the layout, so the
    /// daemon makes the slot first.
    pub fn with_db_rotation(mut self, db_rotation: Arc<DbRotation>) -> Self {
        self.db_rotation = db_rotation;
        self
    }

    pub fn db_rotation(&self) -> &Arc<DbRotation> {
        &self.db_rotation
    }

    pub fn extreme_cardinality(&self) -> &ExtremeCardinality {
        &self.extreme_cardinality
    }

    pub fn pulse(&self) -> &Pulse {
        &self.pulse
    }

    pub fn backfill(&self) -> Backfill {
        self.backfill
    }

    /// The BACKFILL queue the daemon's threads work on.
    pub fn backfill_queue(&self) -> &crate::backfill::BackfillQueue {
        &self.backfill_queue
    }

    /// `nd_profile.update_every`.
    pub fn update_every(&self) -> i64 {
        self.update_every
    }

    /// `get_tier_grouping()`: the product of the iterations of tiers 1 to `tier`, a tier past the last one taken as
    /// the last one.
    pub fn tier_grouping(&self, tier: usize) -> u64 {
        let tier = tier.min(self.storage_tiers() - 1);
        (1..=tier)
            .map(|t| {
                self.grouping_iterations
                    .get(t)
                    .copied()
                    .unwrap_or(GROUPING_ITERATIONS[t.min(RRD_STORAGE_TIERS - 1)])
            })
            .product()
    }

    pub fn dbengine(&self) -> Option<&Arc<Dbengine>> {
        self.dbengine.as_ref()
    }

    /// `nd_profile.storage_tiers`: 1 without the engine.
    pub fn storage_tiers(&self) -> usize {
        self.dbengine.as_ref().map_or(1, |e| e.tiers.len())
    }

    /// Whether a host of `mode` keeps `tier` in the dbengine (`rrdhost_create()`'s `host->db[]`): every tier in use
    /// of a dbengine host, the tiers above 0 of the others.
    pub fn tier_is_dbengine(&self, mode: DbMode, tier: usize) -> bool {
        self.dbengine.is_some()
            && tier < self.storage_tiers()
            && (tier > 0 || mode == DbMode::Dbengine)
    }

    /// The retention view of a host of `mode` (`rrdhost_create()`'s `host->db[]`): every tier from the engine for a
    /// dbengine host; tier 0 from the RAM index and the others from the engine for the other modes; nothing without
    /// the engine (the contexts tree then keeps the RAM index).
    pub(crate) fn tiers_for(
        &self,
        mode: DbMode,
        ram: &Arc<RamIndex>,
    ) -> Vec<Arc<dyn TierRetention>> {
        let Some(engine) = &self.dbengine else {
            return Vec::new();
        };
        (0..engine.tiers.len())
            .map(|tier| -> Arc<dyn TierRetention> {
                if !self.tier_is_dbengine(mode, tier) {
                    Arc::clone(ram) as Arc<dyn TierRetention>
                } else {
                    Arc::new(MrgTier {
                        mrg: engine.mrg.clone(),
                        tier,
                    })
                }
            })
            .collect()
    }
}

/// A dbengine tier's `rrdeng_metric_retention_by_id()`.
#[derive(Debug)]
pub(crate) struct MrgTier {
    pub(crate) mrg: Mrg,
    pub(crate) tier: usize,
}

impl TierRetention for MrgTier {
    fn retention_by_id(&self, uuid: &[u8; 16]) -> Option<(i64, i64)> {
        self.mrg
            .retention_by_uuid(uuid, self.tier)
            .map(|r| (r.first_time_s, r.last_time_s))
    }

    /// `rrdeng_metric_retention_delete_by_id()`: the metric's times cleared; releasing it then removes it from the
    /// registry unless someone else holds it.
    fn delete_by_id(&self, uuid: &[u8; 16]) {
        if let Some(metric) = self.mrg.get_and_acquire(uuid, self.tier) {
            metric.clear_retention();
        }
    }
}

/// A metric's storage on one tier, as the query target holds it (`qm->tiers[t].smh`): a ram ring or a dbengine
/// registry entry, released when dropped.
#[derive(Debug)]
pub enum TierHandle {
    Ram(Arc<Dim>),
    Dbengine {
        engine: Arc<Dbengine>,
        metric: Handle,
    },
}

impl Clone for TierHandle {
    /// `metric_dup()`.
    fn clone(&self) -> Self {
        match self {
            TierHandle::Ram(dim) => TierHandle::Ram(Arc::clone(dim)),
            TierHandle::Dbengine { engine, metric } => TierHandle::Dbengine {
                engine: Arc::clone(engine),
                metric: metric.dup(),
            },
        }
    }
}

impl TierHandle {
    /// `storage_engine_oldest_time_s()` and `storage_engine_latest_time_s()`.
    pub fn retention(&self) -> (i64, i64) {
        match self {
            TierHandle::Ram(dim) => dim
                .ring()
                .map_or((0, 0), |ring| (ring.oldest_time_s(), ring.latest_time_s())),
            TierHandle::Dbengine { metric, .. } => {
                let r = metric.retention();
                (r.first_time_s, r.last_time_s)
            }
        }
    }

    /// `storage_engine_query_init()` of `start_s..=end_s`; the dbengine validates pages against its wall clock (N2).
    pub fn query(&self, start_s: i64, end_s: i64, priority: Priority) -> StorageQuery<'_> {
        match self {
            TierHandle::Ram(dim) => {
                let ring = dim.ring().expect("a ram tier handle has a ring");
                StorageQuery::Ram(ring.query(start_s, end_s))
            }
            TierHandle::Dbengine { engine, metric } => {
                StorageQuery::Dbengine(engine.query(metric, start_s, end_s, priority))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The alert view is what the daemon installed, and the first one stays; a host's health delay is what was
    /// last set, 0 for none.
    #[test]
    fn the_alert_view_and_a_host_s_delay_are_what_was_set() {
        struct Fixed(u64);
        impl AlertView for Fixed {
            fn versions(&self, _: &Host) -> (u64, u64) {
                (self.0, self.0 + 1)
            }
            fn chart_alerts(&self, _: &Host, _: &Chart) -> Vec<ChartAlert> {
                vec![ChartAlert {
                    name: b"a".to_vec(),
                    class: AlertClass::Clear,
                    status_name: "CLEAR",
                    at_least_clear: true,
                    value: 1.5,
                    units: b"things".to_vec(),
                }]
            }
        }
        let host = Host::new("guid-view", false, crate::testutil::info("view"));
        assert!(host.storage().alert_view().is_none());
        host.storage().set_alert_view(Arc::new(Fixed(5)));
        host.storage().set_alert_view(Arc::new(Fixed(50)));
        let view = host.storage().alert_view().expect("the view");
        assert_eq!(view.versions(&host), (5, 6));
        let chart = host.charts().create(&crate::testutil::chart_spec(crate::mode::DbMode::Ram)).0;
        let alerts = view.chart_alerts(&host, &chart);
        assert_eq!((alerts.len(), &alerts[0].name[..], alerts[0].status_name), (1, &b"a"[..], "CLEAR"));

        assert_eq!(host.health_delay_up_to(), 0);
        host.set_health_delay_up_to(1_700_000_060);
        assert_eq!(host.health_delay_up_to(), 1_700_000_060);
        host.set_health_delay_up_to(0);
        assert_eq!(host.health_delay_up_to(), 0);
    }

    /// `rrdeng_metric_retention_delete_by_id()`: the metric's times cleared; one nobody holds leaves the registry,
    /// and its tier counts one metric less; one held stays until released.
    #[test]
    fn a_tier_forgets_a_metric_by_id() {
        let (_dirs, layout) = crate::testutil::engine(2);
        let mrg = layout.dbengine().unwrap().mrg.clone();
        let tier = MrgTier {
            mrg: mrg.clone(),
            tier: 1,
        };
        drop(mrg.add_and_acquire(&[1; 16], 1, 100, 200, 1));
        let (held, _) = mrg.add_and_acquire(&[2; 16], 1, 100, 200, 1);
        assert_eq!(mrg.metrics(1), 2);
        tier.delete_by_id(&[1; 16]);
        tier.delete_by_id(&[2; 16]);
        tier.delete_by_id(&[3; 16]);
        assert!(mrg.get_and_acquire(&[1; 16], 1).is_none());
        assert_eq!(tier.retention_by_id(&[2; 16]), Some((0, 0)));
        assert_eq!(mrg.metrics(1), 1);
        drop(held);
        assert!(mrg.get_and_acquire(&[2; 16], 1).is_none());
        assert_eq!(mrg.metrics(1), 0);
    }
}
