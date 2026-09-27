//! The dbengine at `rrd_init()` (brief `knowledge/brief-dbengine-s2-runtime-map.md` §3 in the status repository):
//! `netdata_conf_dbengine_init()`'s keys, then the tiers' start with the metric registry pre-populated from the
//! metadata database. "mrg cleanup", quiesce and the tiers' stop are the runtime's.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_metadata::read::populate_metrics;
use netdata_agent_rrd::contexts::DbRotation;
use netdata_agent_rrd::mode::DbMode;
use netdata_agent_rrd::storage::Backfill;
use netdata_agent_storage::dbengine::engine::cache::cache_budgets;
use netdata_agent_storage::dbengine::engine::load::TierConfig;
use netdata_agent_storage::dbengine::engine::query::RotationHook;
use netdata_agent_storage::dbengine::engine::runtime::{InitConfig, Runtime};

use crate::conf::{self, Conf, DbSection};
use crate::metasync::now_realtime_s;
use crate::rrdcontext::now_realtime_ut;
use crate::system;

/// `rrdcontext_db_rotation()` as the engine's rotation hook: each rotation arms the contexts' deep pass in `slot`,
/// the one the storage layout holds (D75.6, D77).
fn rotation_hook(slot: &Arc<DbRotation>) -> RotationHook {
    let slot = Arc::clone(slot);
    RotationHook(Arc::new(move || slot.rotated(now_realtime_ut())))
}

/// What the engine's start gives the rest of `rrd_init()`.
pub struct Started {
    pub runtime: Runtime,
    /// `storage_tiers_grouping_iterations`.
    pub grouping: Vec<u64>,
    /// `default_backfill`.
    pub backfill: Backfill,
    /// `dbengine_out_of_memory_protection`, in bytes (0 without one).
    pub out_of_memory_protection: u64,
}

/// `rrd_init()`'s engine start: its record, the keys, then the tiers. C's fallbacks after it (one tier, alloc mode)
/// cannot run in a dbengine build, where the start either brings a tier up or is fatal, so they are not ported.
pub fn start(
    conf: &mut Conf,
    db: &DbSection,
    parent_profile: bool,
    pool: &WorkPool,
    meta: Option<Arc<MetaDb>>,
    db_rotation: &Arc<DbRotation>,
) -> Started {
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "DBENGINE: Initializing ..."
    );
    let settings = conf::dbengine_init(
        &mut conf.netdata,
        &conf.hostname,
        &conf.dirs.cache,
        parent_profile,
        db.update_every,
        conf.legacy_multihost_db_space,
        system::system_memory(Path::new("/")),
    );
    let retention_tiers = settings.tiers.len();
    let tiers = settings
        .tiers
        .iter()
        .enumerate()
        .map(|(tier, t)| {
            t.path.as_ref().map(|path| {
                // rrdeng_init() takes the quota as unsigned
                let mb = t.disk_space_mb as u32;
                // rrdeng_init() raises a smaller non-zero quota silently
                let min = conf::MIN_DISK_SPACE_MB as u32;
                let mb = if mb != 0 && mb < min { min } else { mb };
                TierConfig {
                    direct_io: settings.direct_io,
                    max_disk_space: u64::from(mb) * 1024 * 1024,
                    journal_check: db.journal_check,
                    max_retention_s: t.retention_s,
                    // `[db] dbengine page type` is tier 0's
                    page_type: if tier == 0 {
                        db.page_type
                    } else {
                        TierConfig::default_page_type(tier)
                    },
                    ..TierConfig::new(tier, PathBuf::from(path))
                }
            })
        })
        .collect();
    let (main_cache_bytes, extent_cache_bytes) =
        cache_budgets(db.page_cache_mb, db.extent_cache_mb);
    let cache_dir = PathBuf::from(&conf.dirs.cache);
    let prepopulate = Box::new(move |cb: &mut dyn FnMut(&[u8; 16])| {
        populate_metrics(&cache_dir, meta.as_deref(), cb);
    });
    let nofile_limit = nix::sys::resource::getrlimit(nix::sys::resource::Resource::RLIMIT_NOFILE)
        .map_or(0, |(soft, _)| soft);
    let runtime = Runtime::start(
        InitConfig {
            host: conf.hostname.clone(),
            cache_dir: conf.dirs.cache.clone(),
            tiers,
            cpus: conf.threads.cpus as usize,
            nofile_limit,
            main_cache_bytes,
            extent_cache_bytes,
            pages_per_extent: settings.pages_per_extent as usize,
            update_every_s: db.update_every as u32,
            stack_size: conf.threads.thread_stack_size,
            timer_period: std::time::Duration::from_secs(1),
            rotation: Some(rotation_hook(db_rotation)),
            // localhost->db[tier].eng: a ram or alloc localhost keeps tier 0 out of the dbengine
            retention_tiers: (0..retention_tiers)
                .map(|t| t > 0 || db.mode == DbMode::Dbengine)
                .collect(),
        },
        pool,
        prepopulate,
        now_realtime_s,
    );
    Started {
        runtime,
        grouping: settings.grouping_iterations,
        backfill: settings.backfill,
        out_of_memory_protection: settings.out_of_memory_protection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hook arms the slot it was made from, 120 s past now, and counts the rotation.
    #[test]
    fn the_rotation_hook_arms_the_layouts_slot() {
        let slot = Arc::new(DbRotation::default());
        let hook = rotation_hook(&slot);
        let before = now_realtime_ut();
        (hook.0)();
        assert_eq!(slot.rotations(), 1);
        assert_eq!(slot.due(before + 119_000_000), None);
        assert!(slot.due(now_realtime_ut() + 121_000_000).is_some());
    }
}
