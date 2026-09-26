//! The hosts' storage (`host->db[]`, set up in `rrdhost_create()` from `nd_profile.storage_tiers` and `multidb_ctx`):
//! the dbengine tiers when the engine runs. Brief `knowledge/brief-dbengine-s2-query-map.md` §3 in the status
//! repository; D64.

use std::sync::Arc;

use netdata_agent_storage::dbengine::engine::mrg::Mrg;
use netdata_agent_storage::dbengine::engine::query::Dbengine;

use crate::contexts::{RamIndex, TierRetention};
use crate::mode::DbMode;

/// `nd_profile.storage_tiers` with `multidb_ctx`: the dbengine, when it runs, and its tiers in use.
#[derive(Debug, Default)]
pub struct StorageLayout {
    dbengine: Option<Arc<Dbengine>>,
}

impl StorageLayout {
    pub fn new(dbengine: Option<Arc<Dbengine>>) -> Self {
        StorageLayout { dbengine }
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
