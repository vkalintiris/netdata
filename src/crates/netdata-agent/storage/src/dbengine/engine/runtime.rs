//! The engine's lifecycle: `rrdeng_init()` per tier on its `DBENGINIT[t]` thread (or on the caller's when there are
//! more tiers than CPUs), `rrdeng_dbengine_spawn()`'s one-time work under its lock (the metric registry pre-populated
//! from the metadata database, then the `DBEV` thread), the joins with C's records, readiness; then `DBEV`'s event loop
//! (its 1 s timer starting flushers, journal indexing after rotations, the tiers' flushes and shutdowns), quiesce,
//! the shutdown flushes and exit. Briefs `knowledge/brief-dbengine-s2-runtime-map.md` §3 and
//! `knowledge/brief-dbengine-s3-commit45-map.md` §2 in the status repository; decisions D63 and D67.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::RecvTimeoutError;
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_evloop::thread_create_failed;
use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{
    ErrorLimit, Priority, Source, fatal, nd_log, nd_log_limit, netdata_log_error, netdata_log_info,
    thread_created, thread_finished,
};
use netdata_agent_text::size::size_to_string;

use super::index::journal_index;
use super::load::{Tier, TierConfig, load};
use super::mrg::Mrg;
use super::query::{Dbengine, EngineConfig, RotationHook};
use super::rotate::database_rotate;
use super::v2index::{Slots, populate_files, populating_record, readiness};

/// `RRDENG_FD_BUDGET_PER_INSTANCE`.
const FD_BUDGET_PER_TIER: u64 = 50;
/// `retention_timer`'s period in timer periods (`TIMER_PERIOD_MS * 60`).
const RETENTION_PERIODS: u32 = 60;

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
    /// `rrdeng_pages_per_extent`.
    pub pages_per_extent: usize,
    /// `nd_profile.update_every`.
    pub update_every_s: u32,
    pub stack_size: usize,
    /// `TIMER_PERIOD_MS`: how often `DBEV` starts flushers.
    pub timer_period: Duration,
    /// Called after each data file deletion (`rrdcontext_db_rotation()`).
    pub rotation: Option<RotationHook>,
    /// The tiers the retention timer checks: those localhost stores in the dbengine (`localhost->db[tier].eng`), not
    /// a ram or alloc localhost's tier 0.
    pub retention_tiers: Vec<bool>,
}

/// The commands of the `DBEV` thread.
pub(crate) enum Cmd {
    /// `RRDENG_OPCODE_CTX_POPULATE_MRG`: the tier's population runs as a pool job, which hands the tier back.
    PopulateMrg {
        tier: Tier,
        done: mpsc::Sender<Tier>,
    },
    /// The engine, once its tiers started: every command below acts on it (before it they do nothing).
    Attach(Arc<Dbengine>),
    /// `RRDENG_OPCODE_FLUSH_MAIN`, which the timer sends each period: a flusher job, up to one per CPU.
    FlushMain,
    /// `after_do_cache_flush()`.
    FlushDone,
    /// `after_extent_write()`.
    ExtentWritten(usize),
    /// `RRDENG_OPCODE_JOURNAL_INDEX`.
    JournalIndex(usize),
    /// `after_journal_v2_indexing()`.
    IndexDone(usize),
    /// `RRDENG_OPCODE_DATABASE_ROTATE`.
    DatabaseRotate(usize),
    /// `after_database_rotate()`.
    RotateDone(usize),
    /// `retention_timer_cb()`, every 60 periods.
    RetentionTick,
    /// `RRDENG_OPCODE_CTX_FLUSH_DIRTY`, `RRDENG_OPCODE_CTX_FLUSH_HOT_DIRTY`.
    CtxFlushDirty(usize),
    CtxFlushHotDirty(usize),
    /// `RRDENG_OPCODE_CTX_QUIESCE`.
    Quiesce(usize),
    /// `RRDENG_OPCODE_CTX_SHUTDOWN`: a pool job waits for the tier's extents and queries in flight.
    CtxShutdown {
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

/// What `DBEV` alone decides: the flushers running (`rrdeng_main.flushes_running`) and, per tier, whether an index
/// is queued (`pending_index`) or running (`migration_to_v2_running`), and whether a rotation is queued
/// (`pending_rotate`) or deleting (`now_deleting_files`).
#[derive(Debug, Default)]
struct Sched {
    cpus: usize,
    flushes_running: usize,
    tiers: Vec<TierSched>,
}

#[derive(Debug, Default, Clone, Copy)]
struct TierSched {
    pending_index: bool,
    indexing: bool,
    pending_rotate: bool,
    deleting: bool,
}

impl Sched {
    /// `FLUSH_MAIN`: whether to start another flusher (`pgc_max_flushers()`: one per CPU).
    fn flush_main(&mut self) -> bool {
        if self.flushes_running < self.cpus {
            self.flushes_running += 1;
            return true;
        }
        false
    }

    fn flush_done(&mut self) {
        self.flushes_running = self.flushes_running.saturating_sub(1);
    }

    /// `check_and_schedule_db_rotation()`'s index half: whether to queue the tier's indexing.
    fn check_and_schedule(&mut self, tier: usize, needs_indexing: bool) -> bool {
        let t = &mut self.tiers[tier];
        if needs_indexing && !t.pending_index {
            t.pending_index = true;
            return true;
        }
        false
    }

    /// `JOURNAL_INDEX`: whether to start the tier's indexer (none running, the tier not shutting down).
    fn journal_index(&mut self, tier: usize, quiesced: bool) -> bool {
        let t = &mut self.tiers[tier];
        t.pending_index = false;
        if t.indexing || quiesced {
            return false;
        }
        t.indexing = true;
        true
    }

    fn index_done(&mut self, tier: usize) {
        self.tiers[tier].indexing = false;
    }

    /// `check_and_schedule_db_rotation()`'s rotation half: whether to queue a rotation for a tier over its caps; not
    /// while one is queued, which is recorded.
    fn schedule_rotation(&mut self, tier: usize, cap_exceeded: impl FnOnce() -> bool) -> bool {
        let t = &mut self.tiers[tier];
        if t.pending_rotate {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "DBENGINE: tier {tier} is already pending rotation"
            );
            return false;
        }
        if cap_exceeded() {
            t.pending_rotate = true;
            return true;
        }
        false
    }

    /// `DATABASE_ROTATE`: whether to start a deletion (none running, more than two files, the tier still over).
    fn database_rotate(
        &mut self,
        tier: usize,
        files: usize,
        cap_exceeded: impl FnOnce() -> bool,
    ) -> bool {
        let t = &mut self.tiers[tier];
        t.pending_rotate = false;
        if !t.deleting && files > 2 && cap_exceeded() {
            t.deleting = true;
            return true;
        }
        false
    }

    fn rotate_done(&mut self, tier: usize) {
        self.tiers[tier].deleting = false;
    }
}

/// `ctx_shutdown_tp_worker()`: waits for the tier's extents being written and its queries in flight, with C's record
/// once (it counts the queries only).
fn ctx_shutdown_wait(engine: &Dbengine, tier: usize) {
    let td = &engine.tiers[tier];
    let mut logged = false;
    while td.extents_in_flight() > 0 || td.inflight() > 0 {
        if !logged {
            logged = true;
            netdata_log_info!(
                "DBENGINE: waiting for {} inflight queries to finish to shutdown tier {tier}...",
                td.inflight()
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// What `DBEV` works with.
struct Loop {
    tx: mpsc::Sender<Cmd>,
    pool: WorkPool,
    sched: Sched,
    retention_tiers: Vec<bool>,
}

impl Loop {
    /// A command for the attached engine.
    fn handle(&mut self, cmd: Cmd, e: &Arc<Dbengine>) {
        match cmd {
            Cmd::FlushMain => {
                if self.sched.flush_main() {
                    let (e, tx) = (Arc::clone(e), self.tx.clone());
                    let job = move || {
                        // `cache_flush_tp_worker()`
                        while e.flush_pages(0, None, true, false) {
                            std::thread::yield_now();
                        }
                        let _ = tx.send(Cmd::FlushDone);
                    };
                    if self.pool.queue(job).is_err() {
                        self.sched.flush_done();
                    }
                }
            }
            Cmd::FlushDone => self.sched.flush_done(),
            Cmd::ExtentWritten(tier) => self.check_and_schedule(e, tier),
            Cmd::IndexDone(tier) => {
                self.sched.index_done(tier);
                self.check_and_schedule(e, tier);
            }
            Cmd::DatabaseRotate(tier) => {
                let td = &e.tiers[tier];
                let files = td.filenos().len();
                if self
                    .sched
                    .database_rotate(tier, files, || td.cap_exceeded(e.now_s()))
                {
                    let (e, tx) = (Arc::clone(e), self.tx.clone());
                    let job = move || {
                        database_rotate(&e, tier);
                        let _ = tx.send(Cmd::RotateDone(tier));
                    };
                    if self.pool.queue(job).is_err() {
                        self.sched.rotate_done(tier);
                    }
                }
            }
            Cmd::RotateDone(tier) => {
                self.sched.rotate_done(tier);
                self.check_and_schedule(e, tier);
            }
            Cmd::RetentionTick => {
                for tier in 0..e.tiers.len() {
                    if self.retention_tiers.get(tier).copied().unwrap_or(false) {
                        self.check_and_schedule(e, tier);
                    }
                }
            }
            Cmd::JournalIndex(tier) => {
                if self.sched.journal_index(tier, e.tiers[tier].quiesced()) {
                    e.tiers[tier].clear_needs_indexing();
                    let (e, tx) = (Arc::clone(e), self.tx.clone());
                    let job = move || {
                        journal_index(&e, tier);
                        let _ = tx.send(Cmd::IndexDone(tier));
                    };
                    if self.pool.queue(job).is_err() {
                        self.sched.index_done(tier);
                    }
                }
            }
            Cmd::CtxFlushDirty(tier) | Cmd::CtxFlushHotDirty(tier) => {
                let hot = matches!(cmd, Cmd::CtxFlushHotDirty(_));
                let (e, tx, flushers) = (Arc::clone(e), self.tx.clone(), self.sched.cpus);
                let _ = self.pool.queue(move || {
                    if hot {
                        e.flush_all_hot_and_dirty(tier);
                    } else {
                        e.flush_dirty(tier);
                    }
                    for _ in 0..flushers {
                        let _ = tx.send(Cmd::FlushMain);
                    }
                });
            }
            Cmd::Quiesce(tier) => {
                netdata_log_info!(
                    "DBENGINE: Tier {tier} is shutting down — query processing disabled"
                );
                e.tiers[tier].quiesce();
            }
            Cmd::CtxShutdown { tier, done } => {
                let e = Arc::clone(e);
                let _ = self.pool.queue(move || {
                    ctx_shutdown_wait(&e, tier);
                    let _ = done.send(());
                });
            }
            Cmd::PopulateMrg { .. } | Cmd::Attach(_) | Cmd::Shutdown => {}
        }
    }

    /// `check_and_schedule_db_rotation()`: a tier with a file to index gets its indexing queued, once, and a tier over
    /// its caps its rotation.
    fn check_and_schedule(&mut self, e: &Dbengine, tier: usize) {
        let td = &e.tiers[tier];
        if self.sched.check_and_schedule(tier, td.needs_indexing()) {
            let _ = self.tx.send(Cmd::JournalIndex(tier));
        }
        if self
            .sched
            .schedule_rotation(tier, || td.cap_exceeded(e.now_s()))
        {
            let _ = self.tx.send(Cmd::DatabaseRotate(tier));
        }
    }
}

/// `dbengine_event_loop()`: the commands in order, the timer's tick (`timer_per_sec_cb()`: a flusher; C's cleanup has
/// nothing to do here) each period, and the retention timer's (`retention_timer_cb()`) every 60 periods from 60 periods
/// after the start.
#[allow(clippy::too_many_arguments)]
fn dbev_loop(
    rx: mpsc::Receiver<Cmd>,
    tx: mpsc::Sender<Cmd>,
    pool: WorkPool,
    mrg: Mrg,
    slots: Arc<Slots>,
    now: fn() -> i64,
    cpus: usize,
    period: Duration,
    retention_tiers: Vec<bool>,
) {
    thread_created();
    let mut engine: Option<Arc<Dbengine>> = None;
    let mut state = Loop {
        tx,
        pool,
        sched: Sched {
            cpus,
            ..Sched::default()
        },
        retention_tiers,
    };
    let retention_period = period * RETENTION_PERIODS;
    let mut next = Instant::now() + period;
    let mut next_retention = Instant::now() + retention_period;
    // a timer running late restarts its period from now, as libuv's repeat does
    let advance = |at: Instant, period: Duration, now: Instant| {
        if at + period > now {
            at + period
        } else {
            now + period
        }
    };
    loop {
        let deadline = next.min(next_retention);
        let cmd = match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(cmd) => cmd,
            Err(RecvTimeoutError::Timeout) => {
                let now = Instant::now();
                if now >= next_retention {
                    next_retention = advance(next_retention, retention_period, now);
                    Cmd::RetentionTick
                } else {
                    next = advance(next, period, now);
                    Cmd::FlushMain
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        match cmd {
            Cmd::PopulateMrg { mut tier, done } => {
                let (job_pool, mrg, slots) = (state.pool.clone(), mrg.clone(), Arc::clone(&slots));
                let _ = state.pool.queue(move || {
                    populate_files(&mut tier, &mrg, &job_pool, &slots, now());
                    let _ = done.send(tier);
                });
            }
            Cmd::Attach(e) => {
                state.sched.tiers = vec![TierSched::default(); e.tiers.len()];
                engine = Some(e);
            }
            Cmd::Shutdown => break,
            cmd => {
                if let Some(e) = &engine {
                    state.handle(cmd, e);
                }
            }
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
    timer_period: Duration,
    retention_tiers: Vec<bool>,
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
            let (loop_tx, pool, mrg, slots, now) = (
                tx.clone(),
                self.pool.clone(),
                self.mrg.clone(),
                Slots::new(self.cpus),
                self.now,
            );
            let (cpus, period) = (self.cpus, self.timer_period);
            let retention_tiers = self.retention_tiers.clone();
            let thread = std::thread::Builder::new()
                .name("DBEV".into())
                .stack_size(self.stack_size)
                .spawn(move || {
                    dbev_loop(
                        rx,
                        loop_tx,
                        pool,
                        mrg,
                        slots,
                        now,
                        cpus,
                        period,
                        retention_tiers,
                    )
                })
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
    /// `rrdeng_flush_everything_and_wait()`'s `starting_size_to_flush`, kept across its calls.
    flush_start: AtomicUsize,
}

thread_local! {
    /// `nd_log_limit_static_thread_var(erl, 1, 100 * USEC_PER_MS)`: the collector wait's record, and its pause.
    static WAITING_COLLECTORS: ErrorLimit = const { ErrorLimit::new(1, 100_000) };
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
            timer_period: cfg.timer_period,
            retention_tiers: cfg.retention_tiers.clone(),
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
        // each tier's shutdown holds a pool thread while it waits: flushes still queueing need one more
        debug_assert!(pool.size() > created, "a pool larger than the tiers in use");
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
            tiers.push(tier);
        }
        let engine = Dbengine::new(
            shared.mrg.clone(),
            tiers,
            EngineConfig {
                main_cache_bytes: cfg.main_cache_bytes,
                extent_cache_bytes: cfg.extent_cache_bytes,
                pages_per_extent: cfg.pages_per_extent,
                update_every_s: cfg.update_every_s,
                pool: Some(pool.clone()),
                now,
                rotation: cfg.rotation.clone(),
            },
        );
        let dbev = shared
            .spawn
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .dbev
            .take()
            .expect("DBEV starts with the first tier");
        let _ = engine.events.set(dbev.tx.clone());
        let _ = dbev.tx.send(Cmd::Attach(Arc::clone(&engine)));
        Runtime {
            engine,
            dbev,
            stack_size: cfg.stack_size,
            flush_start: AtomicUsize::new(0),
        }
    }

    pub fn engine(&self) -> &Arc<Dbengine> {
        &self.engine
    }

    /// `nd_profile.storage_tiers` after the start: the tiers in use.
    pub fn storage_tiers(&self) -> usize {
        self.engine.tiers.len()
    }

    /// `rrdeng_quiesce_all()`: each tier in use stops preparing queries, when `DBEV` gets to it.
    pub fn quiesce(&self) {
        for tier in 0..self.storage_tiers() {
            let _ = self.dbev.tx.send(Cmd::Quiesce(tier));
        }
    }

    /// `rrdeng_flush_everything_and_wait()`: each tier in use flushes its dirty pages (with `dirty_only`) or all of
    /// them; then, as asked, a wait for the collectors to finish (up to 50 checks, each record paused 100 ms) and for
    /// the flushes, with C's progress records. Silent when every page is on disk.
    pub fn flush_everything(&self, wait_flush: bool, wait_collectors: bool, dirty_only: bool) {
        let e = &self.engine;
        if e.main.hot_and_dirty_entries() == 0 {
            return;
        }
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "Flushing DBENGINE {} dirty pages...",
            if dirty_only { "only" } else { "hot &" }
        );
        for tier in 0..self.storage_tiers() {
            let _ = self.dbev.tx.send(if dirty_only {
                Cmd::CtxFlushDirty(tier)
            } else {
                Cmd::CtxFlushHotDirty(tier)
            });
        }
        let pending = |s: &super::cache::CacheStats| s.hot_bytes + s.dirty_bytes;
        let size = pending(&e.main.stats());
        let start = self.flush_start.load(Ordering::Relaxed);
        if size > start || start == 0 {
            self.flush_start.store(size, Ordering::Relaxed);
        }
        if wait_collectors {
            let (mut running, mut count) = (1, 50);
            while running != 0 && count != 0 {
                running = e.tiers.iter().map(|td| td.collectors_running()).sum();
                if running != 0 {
                    WAITING_COLLECTORS.with(|erl| {
                        nd_log_limit!(
                            erl,
                            Source::Daemon,
                            Priority::Notice,
                            "waiting for {running} collectors to finish"
                        );
                    });
                }
                count -= 1;
            }
        }
        if !wait_flush {
            return;
        }
        for iterations in 0usize.. {
            let stats = e.main.stats();
            let size = pending(&stats);
            let mut start = self.flush_start.load(Ordering::Relaxed);
            if start == 0 || size > start {
                start = size;
                self.flush_start.store(size, Ordering::Relaxed);
            }
            if size == 0 || stats.hot_entries + stats.dirty_entries == 0 {
                break;
            }
            if iterations.is_multiple_of(10) {
                let text = |n: usize| size_to_string(n as u64, "B", false).unwrap_or_default();
                nd_log!(
                    Source::Daemon,
                    Priority::Info,
                    "DBENGINE: flushing at {:.2}% {{ hot: {}, dirty: {} }}...",
                    (start - size) as f64 * 100.0 / start as f64,
                    text(stats.hot_bytes),
                    text(stats.dirty_bytes)
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "DBENGINE: flushing completed!"
        );
    }

    /// Step "stop dbengine tiers": one `rrdeng-exit` thread per tier in use (`rrdeng_exit()`) waits for its
    /// collectors, flushes every page and waits for its extents and queries in flight, while this thread flushes
    /// everything too; then `dbengine_shutdown()` stops `DBEV`.
    pub fn exit(self) {
        let exits: Vec<_> = (0..self.storage_tiers())
            .filter_map(|tier| {
                let (engine, tx) = (Arc::clone(&self.engine), self.dbev.tx.clone());
                std::thread::Builder::new()
                    .name("rrdeng-exit".into())
                    .stack_size(self.stack_size)
                    .spawn(move || {
                        thread_created();
                        tier_exit(&engine, tier, &tx);
                        RESERVED_FDS.fetch_sub(FD_BUDGET_PER_TIER, Ordering::AcqRel);
                        thread_finished();
                    })
                    .map_err(|err| {
                        netdata_log_error!("{}", thread_create_failed("rrdeng-exit", &err))
                    })
                    .ok()
            })
            .collect();
        self.flush_everything(true, true, false);
        for exit in exits {
            let _ = exit.join();
        }
        let _ = self.dbev.tx.send(Cmd::Shutdown);
        if self.dbev.thread.join().is_ok() {
            netdata_log_info!("DBENGINE: thread shutdown completed");
        }
    }
}

/// `rrdeng_exit()` of one tier: up to a second for its collectors (C's record once), every page flushed, then its
/// shutdown on `DBEV`.
fn tier_exit(engine: &Arc<Dbengine>, tier: usize, tx: &mpsc::Sender<Cmd>) {
    let td = &engine.tiers[tier];
    let (mut logged, mut count) = (false, 10);
    while td.collectors_running() > 0 && count > 0 {
        if !logged {
            netdata_log_info!("DBENGINE: waiting for collectors to finish on tier {tier}...");
            logged = true;
        }
        std::thread::sleep(Duration::from_millis(100));
        count -= 1;
    }
    engine.flush_all_hot_and_dirty(tier);
    let (done, wait) = mpsc::channel();
    if tx.send(Cmd::CtxShutdown { tier, done }).is_ok() {
        let _ = wait.recv();
    }
}

#[cfg(test)]
mod tests;
