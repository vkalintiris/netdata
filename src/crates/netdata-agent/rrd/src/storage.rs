//! The hosts' storage (`host->db[]`, set up in `rrdhost_create()` from `nd_profile.storage_tiers` and `multidb_ctx`):
//! the dbengine tiers when the engine runs. Brief `knowledge/brief-dbengine-s2-query-map.md` §3 in the status
//! repository; D64.

use std::sync::Arc;

use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
use netdata_agent_storage::dbengine::engine::mrg::{Handle, Mrg};
use netdata_agent_storage::dbengine::engine::query::Dbengine;
use netdata_agent_storage::query::{Priority, StorageQuery};

use crate::chart::Dim;
use crate::contexts::{RamIndex, TierRetention};
use crate::mode::DbMode;

/// `storage_tiers_grouping_iterations` before the configuration: tier 0 the update every, the others 60.
const GROUPING_ITERATIONS: [u64; RRD_STORAGE_TIERS] = [1, 60, 60, 60, 60];

/// `nd_profile` (`storage_tiers`, `update_every`) with `multidb_ctx` and `storage_tiers_grouping_iterations`: the
/// dbengine, when it runs, its tiers in use and their grouping.
#[derive(Debug)]
pub struct StorageLayout {
    dbengine: Option<Arc<Dbengine>>,
    grouping_iterations: Vec<u64>,
    update_every: i64,
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
        }
    }

    /// The configured grouping iterations (`[db] dbengine tier N update every iterations`, tier 0 first) and
    /// `nd_profile.update_every`.
    pub fn with_profile(mut self, grouping_iterations: Vec<u64>, update_every: i64) -> Self {
        self.grouping_iterations = grouping_iterations;
        self.update_every = update_every;
        self
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
                if tier == 0 && mode != DbMode::Dbengine {
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
}

/// A metric's storage on one tier, as the query target holds it (`qm->tiers[t].smh`): a ram ring or a dbengine
/// registry entry, released when dropped.
#[derive(Debug)]
pub enum TierHandle {
    Ram(Arc<Dim>),
    Dbengine { engine: Arc<Dbengine>, metric: Handle },
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

    /// `storage_engine_query_init()` of `start_s..=end_s`; `now_s` is the dbengine's clock (D64.8).
    pub fn query(&self, start_s: i64, end_s: i64, priority: Priority, now_s: i64) -> StorageQuery<'_> {
        match self {
            TierHandle::Ram(dim) => {
                let ring = dim.ring().expect("a ram tier handle has a ring");
                StorageQuery::Ram(ring.query(start_s, end_s))
            }
            TierHandle::Dbengine { engine, metric } => {
                StorageQuery::Dbengine(engine.query(metric, start_s, end_s, priority, now_s))
            }
        }
    }
}

