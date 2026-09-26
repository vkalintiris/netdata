//! The dbengine at `rrd_init()` (brief `knowledge/brief-dbengine-s2-runtime-map.md` §3 in the status repository):
//! `netdata_conf_dbengine_init()`'s keys, then the tiers' start with the metric registry pre-populated from the
//! metadata database. "mrg cleanup", quiesce and the tiers' stop are the runtime's.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_metadata::read::populate_metrics;
use netdata_agent_storage::dbengine::engine::cache::cache_budgets;
use netdata_agent_storage::dbengine::engine::load::TierConfig;
use netdata_agent_storage::dbengine::engine::runtime::{InitConfig, Runtime};

use crate::conf::{self, Conf, DbSection};
use crate::metasync::now_realtime_s;
use crate::system;

/// `rrd_init()`'s engine start: its record, the keys, then the tiers; and the tiers' grouping iterations
/// (`storage_tiers_grouping_iterations`). C's fallbacks after it (one tier, alloc mode) cannot run in a dbengine
/// build, where the start either brings a tier up or is fatal, so they are not ported.
pub fn start(
    conf: &mut Conf,
    db: &DbSection,
    parent_profile: bool,
    pool: &WorkPool,
    meta: Option<Arc<MetaDb>>,
) -> (Runtime, Vec<u64>) {
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
                    tier,
                    path: PathBuf::from(path),
                    direct_io: settings.direct_io,
                    max_disk_space: u64::from(mb) * 1024 * 1024,
                    journal_check: db.journal_check,
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
            update_every_s: db.update_every as u32,
            stack_size: conf.threads.thread_stack_size,
        },
        pool,
        prepopulate,
        now_realtime_s,
    );
    (runtime, settings.grouping_iterations)
}
