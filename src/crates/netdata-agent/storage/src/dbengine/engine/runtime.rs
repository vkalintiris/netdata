//! The engine's lifecycle for a read-only engine (S2, D62): `rrdeng_init()` per tier on its `DBENGINIT[t]` thread (or
//! on the caller's when there are more tiers than CPUs), `rrdeng_dbengine_spawn()`'s one-time work under its lock (the
//! metric registry pre-populated from the metadata database, then the `DBEV` thread), the joins with C's records,
//! readiness, quiesce and exit. Brief `knowledge/brief-dbengine-s2-runtime-map.md` §3 in the status repository.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::Duration;

use netdata_agent_evloop::thread_create_failed;
use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{
    Priority, Source, fatal, nd_log, netdata_log_error, netdata_log_info, thread_created,
    thread_finished,
};

use super::cache::{ExtentCache, MainCache};
use super::load::{Tier, TierConfig, load};
use super::mrg::Mrg;
use super::query::{Dbengine, TierData};
use super::v2index::{Slots, populate_files, populating_record, readiness};

/// `RRDENG_FD_BUDGET_PER_INSTANCE`.
const FD_BUDGET_PER_TIER: u64 = 50;

/// `global_stats.rrdeng_reserved_file_descriptors`.
static RESERVED_FDS: AtomicU64 = AtomicU64::new(0);

/// `populate_metrics_from_database()`: calls back with every stored metric's UUID, and logs C's record.
pub type Prepopulate = Box<dyn FnOnce(&mut dyn FnMut(&[u8; 16])) + Send>;

/// What `netdata_conf_dbengine_init()` hands the engine.
#[derive(Debug, Clone)]
pub struct InitConfig {
    /// The hostname C's join records name.
    pub host: String,
    /// The cache directory, which the record of a start without any tier names.
    pub cache_dir: String,
    /// The configured tiers in order; `None` for one whose directory could not be created.
    pub tiers: Vec<Option<TierConfig>>,
    /// `netdata_conf_cpus()`: the tiers start in parallel when they are no more than this; it also sizes the
    /// population's semaphore.
    pub cpus: usize,
    /// `rlimit_nofile.rlim_cur`.
    pub nofile_limit: u64,
    /// The caches' budgets in bytes (`cache_budgets()`).
    pub main_cache_bytes: usize,
    pub extent_cache_bytes: usize,
    /// `nd_profile.update_every`.
    pub update_every_s: u32,
    pub stack_size: usize,
}

/// The commands of the `DBEV` thread.
enum Cmd {
    /// `RRDENG_OPCODE_CTX_POPULATE_MRG`: the tier's population runs as a pool job, which hands the tier back.
    PopulateMrg {
        tier: Tier,
        done: mpsc::Sender<Tier>,
    },
    /// `RRDENG_OPCODE_CTX_QUIESCE`.
    Quiesce { engine: Arc<Dbengine>, tier: usize },
    /// `RRDENG_OPCODE_CTX_SHUTDOWN`: a pool job waits for the tier's queries in flight.
    CtxShutdown {
        engine: Arc<Dbengine>,
        tier: usize,
        done: mpsc::Sender<()>,
    },
    /// `RRDENG_OPCODE_SHUTDOWN_EVLOOP`.
    Shutdown,
}

/// The `DBEV` thread.
struct Dbev {
    tx: mpsc::Sender<Cmd>,
    thread: JoinHandle<()>,
}

/// `dbengine_event_loop()` without its timers, which print nothing on a read-only engine (D62.10).
fn dbev_loop(
    rx: mpsc::Receiver<Cmd>,
    pool: WorkPool,
    mrg: Mrg,
    slots: Arc<Slots>,
    now: fn() -> i64,
) {
    thread_created();
    for cmd in rx {
        match cmd {
            Cmd::PopulateMrg { mut tier, done } => {
                let (job_pool, mrg, slots) = (pool.clone(), mrg.clone(), Arc::clone(&slots));
                let _ = pool.queue(move || {
                    populate_files(&mut tier, &mrg, &job_pool, &slots, now());
                    let _ = done.send(tier);
                });
            }
            Cmd::Quiesce { engine, tier } => {
                netdata_log_info!(
                    "DBENGINE: Tier {tier} is shutting down — query processing disabled"
                );
                engine.tiers[tier].quiesce();
            }
            Cmd::CtxShutdown { engine, tier, done } => {
                let _ = pool.queue(move || {
                    let mut logged = false;
                    loop {
                        let inflight = engine.tiers[tier].inflight();
                        if inflight == 0 {
                            break;
                        }
                        if !logged {
                            logged = true;
                            netdata_log_info!(
                                "DBENGINE: waiting for {inflight} inflight queries to finish to shutdown tier {tier}..."
                            );
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    let _ = done.send(());
                });
            }
            Cmd::Shutdown => break,
        }
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Shutting down dbengine thread"
    );
    thread_finished();
}

/// What every tier's start shares: `rrdeng_dbengine_spawn()`'s lock with the pre-population still to run, the
/// `DBEV` thread once started, and the registry.
struct Shared {
    spawn: Mutex<Spawn>,
    mrg: Mrg,
    pool: WorkPool,
    configured_tiers: usize,
    cpus: usize,
    nofile_limit: u64,
    stack_size: usize,
    now: fn() -> i64,
}

struct Spawn {
    prepopulate: Option<Prepopulate>,
    dbev: Option<Dbev>,
}

impl Shared {
    /// `rrdeng_dbengine_spawn()`: the first tier to take the lock pre-populates the registry for every configured
    /// tier and starts `DBEV`, while the others wait; the command sender for the caller.
    fn spawn(&self) -> mpsc::Sender<Cmd> {
        let mut spawn = self.spawn.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(prepopulate) = spawn.prepopulate.take() {
            let mrg = &self.mrg;
            prepopulate(&mut |uuid| {
                for tier in 0..self.configured_tiers {
                    mrg.prepopulate(uuid, tier);
                }
            });
            let (tx, rx) = mpsc::channel();
            let (pool, mrg, slots, now) = (
                self.pool.clone(),
                self.mrg.clone(),
                Slots::new(self.cpus),
                self.now,
            );
            let thread = std::thread::Builder::new()
                .name("DBEV".into())
                .stack_size(self.stack_size)
                .spawn(move || dbev_loop(rx, pool, mrg, slots, now))
                .unwrap_or_else(|err| fatal!("{}", thread_create_failed("DBEV", &err)));
            spawn.dbev = Some(Dbev { tx, thread });
        }
        spawn
            .dbev
            .as_ref()
            .map(|dbev| dbev.tx.clone())
            .expect("DBEV starts with the first tier")
    }

    /// `rrdeng_init()`: the file descriptor budget, the spawn, the tier's files, then its population queued on
    /// `DBEV`; the tier comes back on the receiver once populated. `None` is C's failure.
    fn tier_init(&self, cfg: TierConfig) -> Option<mpsc::Receiver<Tier>> {
        let max_open_files = self.nofile_limit / 4;
        let reserved =
            RESERVED_FDS.fetch_add(FD_BUDGET_PER_TIER, Ordering::AcqRel) + FD_BUDGET_PER_TIER;
        if reserved > max_open_files {
            netdata_log_error!(
                "Exceeded the budget of available file descriptors ({}/{}), cannot create new dbengine instance.",
                reserved as u32,
                max_open_files as u32
            );
            RESERVED_FDS.fetch_sub(FD_BUDGET_PER_TIER, Ordering::AcqRel);
            return None;
        }
        let dbev = self.spawn();
        match load(cfg, &self.mrg, (self.now)()) {
            Ok(tier) => {
                populating_record(&tier, self.cpus);
                let (done, rx) = mpsc::channel();
                let _ = dbev.send(Cmd::PopulateMrg { tier, done });
                Some(rx)
            }
            Err(_) => {
                RESERVED_FDS.fetch_sub(FD_BUDGET_PER_TIER, Ordering::AcqRel);
                None
            }
        }
    }
}

/// The tiers in use after the joins: those that started, counted from tier 0 up to the first that did not.
pub fn created_tiers(started: &[bool]) -> usize {
    started.iter().take_while(|&&ok| ok).count()
}

/// A running engine.
pub struct Runtime {
    engine: Arc<Dbengine>,
    dbev: Dbev,
    stack_size: usize,
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime")
            .field("storage_tiers", &self.storage_tiers())
            .finish_non_exhaustive()
    }
}

impl Runtime {
    /// The tiers' starts, as `netdata_conf_dbengine_init()` runs them from its mkdir loop on: C's records at the
    /// joins, then each tier in use made ready in order. `fatal!` when no tier started, as C.
    pub fn start(
        cfg: InitConfig,
        pool: &WorkPool,
        prepopulate: Prepopulate,
        now: fn() -> i64,
    ) -> Runtime {
        let configured = cfg.tiers.len();
        let shared = Arc::new(Shared {
            spawn: Mutex::new(Spawn {
                prepopulate: Some(prepopulate),
                dbev: None,
            }),
            mrg: Mrg::new(),
            pool: pool.clone(),
            configured_tiers: configured,
            cpus: cfg.cpus,
            nofile_limit: cfg.nofile_limit,
            stack_size: cfg.stack_size,
            now,
        });
        let parallel = configured <= cfg.cpus;
        enum Started {
            Thread(JoinHandle<Option<mpsc::Receiver<Tier>>>),
            Done(Option<mpsc::Receiver<Tier>>),
        }
        let mut starts = Vec::with_capacity(configured);
        for (t, tier) in cfg.tiers.iter().enumerate() {
            // a tier without its directory is counted as failed (C would wait for it forever, DEFECTS)
            let Some(tier) = tier.clone() else {
                starts.push(Started::Done(None));
                continue;
            };
            if !parallel {
                starts.push(Started::Done(shared.tier_init(tier)));
                continue;
            }
            let name = format!("DBENGINIT[{t}]");
            let thread_shared = Arc::clone(&shared);
            let thread_tier = tier.clone();
            match std::thread::Builder::new()
                .name(name.clone())
                .stack_size(cfg.stack_size)
                .spawn(move || {
                    thread_created();
                    let rx = thread_shared.tier_init(thread_tier);
                    thread_finished();
                    rx
                }) {
                Ok(thread) => starts.push(Started::Thread(thread)),
                // C would wait for the tier forever; this port starts it on the caller's thread
                Err(err) => {
                    netdata_log_error!("{}", thread_create_failed(&name, &err));
                    starts.push(Started::Done(shared.tier_init(tier)));
                }
            }
        }
        let mut ready = Vec::with_capacity(configured);
        for (t, start) in starts.into_iter().enumerate() {
            let rx = match start {
                Started::Thread(thread) => thread.join().ok().flatten(),
                Started::Done(rx) => rx,
            };
            if rx.is_none() && cfg.tiers[t].is_some() {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "DBENGINE on '{}': Failed to initialize multi-host database tier {t} on path '{}'",
                    cfg.host,
                    cfg.tiers[t]
                        .as_ref()
                        .map(|c| c.path.display().to_string())
                        .unwrap_or_default()
                );
            }
            ready.push(rx);
        }
        let started: Vec<bool> = ready.iter().map(Option::is_some).collect();
        let created = created_tiers(&started);
        if created == 0 {
            fatal!(
                "DBENGINE on '{}', failed to initialize databases at '{}'.",
                cfg.host,
                cfg.cache_dir
            );
        }
        if created < configured {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "DBENGINE on '{}': Managed to create {created} tiers instead of {configured}. Continuing with {created} \
                 available.",
                cfg.host
            );
        }
        // rrdeng_readiness_wait(), in order; the tiers past the first failure are never waited for
        let mut tiers = Vec::with_capacity(created);
        for rx in ready.into_iter().take(created).flatten() {
            let Ok(mut tier) = rx.recv() else {
                fatal!(
                    "DBENGINE on '{}', failed to initialize databases at '{}'.",
                    cfg.host,
                    cfg.cache_dir
                );
            };
            readiness(&mut tier, now());
            tiers.push(TierData::new(tier));
        }
        let engine = Arc::new(Dbengine {
            mrg: shared.mrg.clone(),
            tiers,
            main: MainCache::new(cfg.main_cache_bytes),
            extents: ExtentCache::new(cfg.extent_cache_bytes),
            pool: Some(pool.clone()),
            update_every_s: cfg.update_every_s,
        });
        let dbev = shared
            .spawn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .dbev
            .take()
            .expect("DBEV starts with the first tier");
        Runtime {
            engine,
            dbev,
            stack_size: cfg.stack_size,
        }
    }

    pub fn engine(&self) -> &Arc<Dbengine> {
        &self.engine
    }

    /// `nd_profile.storage_tiers` after the start: the tiers in use.
    pub fn storage_tiers(&self) -> usize {
        self.engine.tiers.len()
    }

    /// "mrg cleanup": `mrg_metric_prepopulate_cleanup()`.
    pub fn prepopulate_cleanup(&self) {
        self.engine.mrg.prepopulate_cleanup();
    }

    /// `rrdeng_quiesce_all()`: each tier in use stops preparing queries, when `DBEV` gets to it.
    pub fn quiesce(&self) {
        for tier in 0..self.storage_tiers() {
            let _ = self.dbev.tx.send(Cmd::Quiesce {
                engine: Arc::clone(&self.engine),
                tier,
            });
        }
    }

    /// Step "stop dbengine tiers": one `rrdeng-exit` thread per tier in use waits for its queries in flight, then
    /// `dbengine_shutdown()` stops `DBEV`.
    pub fn exit(self) {
        let exits: Vec<_> = (0..self.storage_tiers())
            .filter_map(|tier| {
                let (engine, tx) = (Arc::clone(&self.engine), self.dbev.tx.clone());
                std::thread::Builder::new()
                    .name("rrdeng-exit".into())
                    .stack_size(self.stack_size)
                    .spawn(move || {
                        thread_created();
                        let (done, wait) = mpsc::channel();
                        if tx.send(Cmd::CtxShutdown { engine, tier, done }).is_ok() {
                            let _ = wait.recv();
                        }
                        RESERVED_FDS.fetch_sub(FD_BUDGET_PER_TIER, Ordering::AcqRel);
                        thread_finished();
                    })
                    .map_err(|err| {
                        netdata_log_error!("{}", thread_create_failed("rrdeng-exit", &err))
                    })
                    .ok()
            })
            .collect();
        for exit in exits {
            let _ = exit.join();
        }
        let _ = self.dbev.tx.send(Cmd::Shutdown);
        if self.dbev.thread.join().is_ok() {
            netdata_log_info!("DBENGINE: thread shutdown completed");
        }
    }
}

#[cfg(test)]
mod tests;
