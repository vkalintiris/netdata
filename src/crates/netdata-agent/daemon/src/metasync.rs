//! The METASYNC thread of `src/database/sqlite/sqlite_metadata.c` (`metadata_event_loop()`): its lifecycle records;
//! the metadata writer, whose store job it hands to the `UV_WORKER` pool 6 s after it starts and then about every 6 s
//! (a 1 s timer, and 5 s after each job ends), with a final store at shutdown (D61); the context cleanups and the
//! freed dimensions' rows that job writes first; the claim id of an unclaimed start; and the context load of the
//! archived hosts, also on the pool (`ctx_hosts_load()`), one `CTXLOAD` thread per host while slots last (D59.5),
//! reading a dbengine host's contexts from SQL (D64).

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_health::Health;
use netdata_agent_health::alerts::HostAlerts;
use netdata_agent_log::{Priority, Source, nd_log, netdata_log_info};
use netdata_agent_metadata::Connection;
use netdata_agent_metadata::cleanup::{CleanupCycle, CycleEnv, CycleKind};
use netdata_agent_metadata::health::AlertHashRow;
use netdata_agent_metadata::open::{ContextDb, MetaDb};
use netdata_agent_metadata::read;
use netdata_agent_rrd::clock::now_realtime_s;
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::duration::duration_to_string;

use crate::startup::now_ut;

/// `METADATA_HOST_CHECK_FIRST_CHECK`, `METADATA_HOST_CHECK_INTERVAL`: seconds before the next store job may run.
const HOST_CHECK_FIRST_S: i64 = 5;
const HOST_CHECK_INTERVAL_S: i64 = 5;

/// `run_metadata_cleanup()`'s first context cleanup scan, 5 s after the first job reached it, and
/// `METADATA_MAINTENANCE_CTX_CLEAN_REPEAT`, the seconds from a scan's end to the next.
const CTX_CLEANUP_FIRST_S: i64 = 5;
const CTX_CLEANUP_REPEAT_S: i64 = 300;
/// `METADATA_MAINTENANCE_FIRST_CHECK` and `METADATA_HEALTH_LOG_INTERVAL`: the health log's cleanup comes this long
/// after the first job reached it, and then this long after each start of it.
const HEALTH_LOG_CLEANUP_FIRST_S: i64 = 1800;
const HEALTH_LOG_CLEANUP_REPEAT_S: i64 = 3600;

/// The loop's timer (`TIMER_INITIAL_PERIOD_MS`, `TIMER_REPEAT_PERIOD_MS`).
const TIMER_PERIOD: Duration = Duration::from_secs(1);

/// `MAX_SHUTDOWN_TIMEOUT_SECONDS`, `SHUTDOWN_SLEEP_INTERVAL_MS`: how long the shutdown waits for a running job.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(15);
const SHUTDOWN_POLL: Duration = Duration::from_millis(100);

/// What the loop and its store jobs share (`struct meta_config_s`).
struct Shared {
    /// `metadata_check_after`: the timer asks for a store once the wall clock is past it.
    check_after: AtomicI64,
    /// `shutdown_requested`.
    shutdown: AtomicBool,
    /// `next_vacuum_run` and `next_context_list_cleanup` of `run_metadata_cleanup()`, 0 before the first job.
    next_vacuum_run: AtomicI64,
    next_ctx_cleanup: AtomicI64,
    /// `next_execution_t` of `cleanup_health_log()`, 0 before the first job.
    next_health_log_cleanup: AtomicI64,
    /// `dim_cleanup_cycle`, `chart_cleanup_cycle` and `label_cleanup_cycle`.
    cycles: Mutex<[CleanupCycle; 3]>,
    /// `ctx_load_running`: a context load job runs; the shutdown waits for it.
    ctx_load_running: AtomicBool,
    /// When the store job runs the maintenance next.
    maintenance: crate::maintenance::Schedule,
}

/// The writer's databases and hosts, once localhost exists.
#[derive(Clone)]
struct Writer {
    meta: Arc<MetaDb>,
    /// The shared context database, which the context loads read and delete in.
    context_db: Weak<ContextDb>,
    hosts: Arc<Hosts>,
    /// `dbengine_datafiles_present`: without the dbengine, freed dimensions keep their rows, which may describe
    /// dbengine data.
    datafiles_present: bool,
    /// The health plugin, for a host's alert log: its retention, and its entries in memory.
    health: Arc<Health>,
}

/// `dimension_can_be_deleted()`: no dbengine tier holds retention for the dimension; never without the dbengine,
/// whose files may still hold it.
fn dimension_can_be_deleted(writer: &Writer, uuid: &[u8; 16]) -> bool {
    writer.hosts.storage().dbengine().is_some_and(|engine| {
        (0..engine.tiers.len()).all(|tier| {
            engine
                .mrg
                .retention_by_uuid(uuid, tier)
                .is_none_or(|r| r.first_time_s <= 0)
        })
    })
}

/// `do_pending_uuid_deletion()`'s check of a freed dimension: without the dbengine and with no datafiles found at
/// start nothing can hold its data, so its row goes.
fn freed_dimension_can_be_deleted(writer: &Writer, uuid: &[u8; 16]) -> bool {
    (writer.hosts.storage().dbengine().is_none() && !writer.datafiles_present)
        || dimension_can_be_deleted(writer, uuid)
}

/// `do_pending_uuid_deletion()`: the rows of the dimensions freed since the last job.
fn delete_pending_dimensions(writer: &Writer, shared: &Shared, pending: Vec<[u8; 16]>) {
    let started = now_ut();
    for uuid in &pending {
        if !shared.shutdown.load(Ordering::Acquire) && freed_dimension_can_be_deleted(writer, uuid)
        {
            writer.meta.delete_dimension(uuid);
        }
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Processed {} dimension delete items in {:.2} ms",
        pending.len(),
        now_ut().saturating_sub(started) as f64 / 1000.0
    );
}

/// `store_ctx_cleanup_list()`: the context cleanups queued since the last job; each skipped once a shutdown began.
fn store_ctx_cleanup(writer: &Writer, shared: &Shared, pending: Vec<([u8; 16], String)>) {
    let started = now_ut();
    writer
        .meta
        .schedule_host_ctx_cleanup(&pending, || shared.shutdown.load(Ordering::Acquire));
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Stored {} host context cleanup items in {:.2} ms",
        pending.len(),
        now_ut().saturating_sub(started) as f64 / 1000.0
    );
}

/// What a store job takes over from the loop (`worker->pending_*`).
#[derive(Default)]
struct Pending {
    ctx_cleanup: Option<Vec<([u8; 16], String)>>,
    deletions: Option<Vec<[u8; 16]>>,
    /// `pending_sql_statement`: statements bound elsewhere and stepped by the next job, in queue order.
    statements: Option<Vec<AlertHashRow>>,
    /// `pending_alert_list`: the alert log entries HEALTH and the chart-freeing threads asked to be saved, each by
    /// its host's alerts and its unique id, in queue order.
    alerts: Option<Vec<(Arc<HostAlerts>, u32)>>,
}

/// `store_sql_statements()`: each queued statement stepped on its own, in queue order, with no transaction; at the
/// shutdown (`only_finalize`) they are dropped unstepped, and the record is the same.
fn store_sql_statements(writer: Option<&Writer>, statements: Vec<AlertHashRow>) {
    let started = now_ut();
    if let Some(writer) = writer {
        for row in &statements {
            writer.meta.store_alert_config(row);
        }
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Stored and processed {} sql statements in {}",
        statements.len(),
        duration(now_ut().saturating_sub(started))
    );
}

/// `store_alert_transitions()`: each queued entry is saved as it stands by now (an insert, or an update when HEALTH
/// saved it meanwhile), and its two pending counters are taken back whatever the save did. C's list holds a host
/// and an entry per save, and its record counts both. At the shutdown the list is dropped unsaved, without a record.
fn store_alert_transitions(writer: &Writer, pending: Vec<(Arc<HostAlerts>, u32)>) {
    let started = now_ut();
    let entries = pending.len() * 2;
    for (alerts, unique_id) in pending {
        let host = alerts.host();
        let save = |entry: &_| host.as_ref().is_some_and(|host| crate::health::save_entry(&writer.meta, host, entry));
        alerts.save_queued(unique_id, &save);
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Stored and processed {entries} alert transitions in {:.2} ms",
        now_ut().saturating_sub(started) as f64 / 1000.0
    );
}

/// `cleanup_health_log()`: 1800 s after the first job reached it, and then 3600 s after each start: every host's
/// alert log loses the entries older than the host's retention that a newer one replaced, in the table and in
/// memory; then the alarms of hosts the table `host` no longer has go, with their entries and versions. A
/// shutdown stops it between hosts.
fn cleanup_health_log(writer: &Writer, shared: &Shared) {
    let shutting_down = || shared.shutdown.load(Ordering::Acquire);
    let now = now_realtime_s();
    if shared.next_health_log_cleanup.load(Ordering::Acquire) == 0 {
        shared.next_health_log_cleanup.store(now.saturating_add(HEALTH_LOG_CLEANUP_FIRST_S), Ordering::Release);
    }
    if shared.next_health_log_cleanup.load(Ordering::Acquire) > now {
        return;
    }
    shared.next_health_log_cleanup.store(now.saturating_add(HEALTH_LOG_CLEANUP_REPEAT_S), Ordering::Release);
    for host in writer.hosts.all() {
        if let Some(host_id) = crate::meta_store::host_id(&host) {
            let alerts = writer.health.host(&host);
            netdata_agent_health::sql::cleanup(&writer.meta, &host, &host_id, alerts.as_deref(), &now_realtime_s);
        }
        if shutting_down() {
            return;
        }
    }
    writer.meta.delete_orphan_health_rows();
}

fn cleanup_cycles() -> [CleanupCycle; 3] {
    [
        CleanupCycle::new(CycleKind::Dimension),
        CleanupCycle::new(CycleKind::Chart),
        CleanupCycle::new(CycleKind::ChartLabel),
    ]
}

/// `run_metadata_cleanup()`: the context cleanup scan of every host, 5 s after the first job reached it and then
/// 300 s after each scan ends, skipped (and cut short) while the WAL is too large; then the dimension cycle, the chart
/// cycle when the dimension cycle had nothing to do, and the chart-label cycle when neither had; then the database's
/// cycle when the dimension cycle had nothing to do, and the chart-label cycle when neither had; then the health
/// log's cleanup; then the database's upkeep. A shutdown stops it between the steps.
fn run_metadata_cleanup(writer: &Writer, shared: &Shared) {
    let shutting_down = || shared.shutdown.load(Ordering::Acquire);
    let now = now_realtime_s();
    if shared.next_ctx_cleanup.load(Ordering::Acquire) == 0 {
        shared
            .next_ctx_cleanup
            .store(now.saturating_add(CTX_CLEANUP_FIRST_S), Ordering::Release);
    }
    if shared.next_ctx_cleanup.load(Ordering::Acquire) < now && writer.meta.wal_size_acceptable() {
        for host in writer.hosts.all() {
            if let Some(host_id) = crate::meta_store::host_id(&host) {
                writer.meta.cleanup_host_contexts(
                    &host_id,
                    &host.hostname(),
                    |uuid| dimension_can_be_deleted(writer, uuid),
                    shutting_down,
                );
            }
            if shutting_down() || !writer.meta.wal_size_acceptable() {
                break;
            }
        }
        shared.next_ctx_cleanup.store(
            now_realtime_s().saturating_add(CTX_CLEANUP_REPEAT_S),
            Ordering::Release,
        );
    }
    if shutting_down() {
        return;
    }
    {
        let can_be_deleted = |uuid: &[u8; 16]| dimension_can_be_deleted(writer, uuid);
        let monotonic = || (now_ut() / 1_000_000) as i64;
        let env = CycleEnv {
            now: &now_realtime_s,
            monotonic: &monotonic,
            shutting_down: &shutting_down,
            dimension_can_be_deleted: &can_be_deleted,
        };
        let mut cycles = shared.cycles.lock().unwrap_or_else(PoisonError::into_inner);
        let [dimension, chart, label] = &mut *cycles;
        if writer.meta.run_cleanup_cycle(dimension, &env)
            && writer.meta.run_cleanup_cycle(chart, &env)
        {
            writer.meta.run_cleanup_cycle(label, &env);
        }
    }
    cleanup_health_log(writer, shared);
    if shutting_down() {
        return;
    }
    let mut next = shared.next_vacuum_run.load(Ordering::Acquire);
    writer.meta.vacuum(&mut next, now_realtime_s());
    shared.next_vacuum_run.store(next, Ordering::Release);
    writer.meta.wal_checkpoint();
}

/// `start_metadata_hosts()`, on a pool thread: the maintenance at most every 10 s (`run_maintenace()`, whose freed
/// dimensions reach the next job), the queued statements, the queued alert log entries, the context cleanups, the
/// freed dimensions, the hosts' pending metadata, then the metadata cleanup, and the next store no sooner than 5 s
/// from now.
fn store_job(writer: &Writer, shared: &Shared, pending: Pending) {
    shared
        .maintenance
        .run_if_due(&writer.hosts, Some(&writer.meta), now_realtime_s());
    if let Some(statements) = pending.statements {
        store_sql_statements(Some(writer), statements);
    }
    if let Some(alerts) = pending.alerts {
        store_alert_transitions(writer, alerts);
    }
    if let Some(cleanup) = pending.ctx_cleanup {
        store_ctx_cleanup(writer, shared, cleanup);
    }
    // before the store: a dimension freed and created again keeps the row the store writes
    if let Some(pending) = pending.deletions {
        delete_pending_dimensions(writer, shared, pending);
    }
    let started = now_ut();
    crate::meta_store::store_hosts_metadata(
        &writer.meta,
        &writer.hosts,
        &shared.shutdown,
        true,
        false,
    );
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Checking all hosts completed in {}",
        duration(now_ut().saturating_sub(started))
    );
    if !shared.shutdown.load(Ordering::Acquire) {
        run_metadata_cleanup(writer, shared);
    }
    shared.check_after.store(
        now_realtime_s().saturating_add(HOST_CHECK_INTERVAL_S),
        Ordering::Release,
    );
}

enum Cmd {
    /// `METADATA_LOAD_HOST_CONTEXT`: load the pending hosts' contexts; vnodes report on the channel.
    LoadHostContexts(Arc<Hosts>, mpsc::Sender<()>),
    /// `METADATA_STORE_CLAIM_ID`: a host's node instance with no claim id (D61.3).
    StoreClaimId(Option<Arc<MetaDb>>, [u8; 16]),
    /// The writer's database and hosts: C's writer reads the global host index, which exists once localhost does.
    Writer(Writer),
    /// `after_metadata_hosts()`: the store job has finished.
    StoreDone,
    /// `METADATA_DEL_DIMENSION`: a freed dimension's row, deleted by the next job.
    DelDimension([u8; 16]),
    /// `METADATA_ADD_CTX_CLEANUP`: a host's context for the metadata cleanup, stored by the next job.
    AddCtxCleanup([u8; 16], String),
    /// `METADATA_EXECUTE_STORE_STATEMENT`: a statement bound by its caller, stepped by the next job.
    ExecuteStore(Box<AlertHashRow>),
    /// `METADATA_ADD_HOST_AE`: an alert log entry of a host, saved by the next job.
    AddHostAe(Arc<HostAlerts>, u32),
    /// `METADATA_STORE` as a command (`commit_alert_transitions()`): a job now, unless one runs.
    Store,
    Shutdown,
}

/// A handle on METASYNC's command queue for other threads.
#[derive(Clone)]
pub struct MetaQueue(mpsc::Sender<Cmd>);

impl MetaQueue {
    /// `metaqueue_store_claim_id()`.
    pub fn store_claim_id(&self, meta: Option<Arc<MetaDb>>, id: [u8; 16]) {
        let _ = self.0.send(Cmd::StoreClaimId(meta, id));
    }

    /// `metaqueue_delete_dimension_uuid()`: a freed dimension's row goes at the next job (a failed queue drops it).
    pub fn delete_dimension(&self, uuid: [u8; 16]) {
        let _ = self.0.send(Cmd::DelDimension(uuid));
    }

    /// `metadata_queue_ctx_host_cleanup()`: stored by the next job (a failed queue drops it).
    pub fn ctx_host_cleanup(&self, host_id: [u8; 16], context: String) {
        let _ = self.0.send(Cmd::AddCtxCleanup(host_id, context));
    }

    /// `metadata_execute_store_statement()` of an alert configuration's row: stored by the next job, lost when the
    /// agent stops before it (a failed queue drops it).
    pub fn execute_store_statement(&self, row: AlertHashRow) {
        let _ = self.0.send(Cmd::ExecuteStore(Box::new(row)));
    }

    /// `metadata_queue_ae_save()`'s command: the entry of that unique id is saved by the next job, as it stands
    /// then. False when the queue is gone: the caller takes its counters back.
    pub fn ae_save(&self, alerts: &Arc<HostAlerts>, unique_id: u32) -> bool {
        self.0.send(Cmd::AddHostAe(Arc::clone(alerts), unique_id)).is_ok()
    }

    /// `commit_alert_transitions()`: a store job is asked for now; the command is dropped while one runs.
    pub fn store(&self) {
        let _ = self.0.send(Cmd::Store);
    }
}

/// A queue no thread reads, for tests of what queues on it.
#[cfg(test)]
pub(crate) struct Unread(mpsc::Receiver<Cmd>);

#[cfg(test)]
impl Unread {
    /// The rows `execute_store_statement()` queued since the last call, in order.
    pub(crate) fn statements(&self) -> Vec<AlertHashRow> {
        let rows = self.0.try_iter().filter_map(|cmd| match cmd {
            Cmd::ExecuteStore(row) => Some(*row),
            _ => None,
        });
        rows.collect()
    }

    /// The alert log entries `ae_save()` queued since the last call, in order, and how many stores `store()` asked
    /// for.
    pub(crate) fn alert_commands(&self) -> (Vec<(Arc<HostAlerts>, u32)>, usize) {
        let (mut saves, mut stores) = (Vec::new(), 0);
        for cmd in self.0.try_iter() {
            match cmd {
                Cmd::AddHostAe(alerts, unique_id) => saves.push((alerts, unique_id)),
                Cmd::Store => stores += 1,
                _ => {}
            }
        }
        (saves, stores)
    }
}

#[cfg(test)]
impl MetaQueue {
    pub(crate) fn unread() -> (MetaQueue, Unread) {
        let (tx, rx) = mpsc::channel();
        (MetaQueue(tx), Unread(rx))
    }
}

/// The running METASYNC thread.
pub struct MetaSync {
    tx: mpsc::Sender<Cmd>,
    done: mpsc::Receiver<()>,
    thread: JoinHandle<()>,
}

pub(crate) fn duration(us: u64) -> String {
    duration_to_string(i64::try_from(us).unwrap_or(i64::MAX), "us", true).unwrap_or_default()
}

impl MetaSync {
    /// `metadata_sync_init()`: the thread, once it runs. `None` when it cannot start (C's creation asserts).
    pub fn start(pool: &WorkPool, cpus: usize, stack_size: usize) -> std::io::Result<MetaSync> {
        let (tx, rx) = mpsc::channel();
        let job_tx = tx.clone();
        let (done_tx, done) = mpsc::channel();
        let pool = pool.clone();
        let thread = std::thread::Builder::new()
            .name("METASYNC".into())
            .stack_size(stack_size)
            .spawn(move || {
                netdata_agent_log::thread_created();
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "Starting metadata sync thread"
                );
                netdata_log_info!("METADATA: Synchronization thread is up and running");
                let shared = Arc::new(Shared {
                    check_after: AtomicI64::new(now_realtime_s() + HOST_CHECK_FIRST_S),
                    shutdown: AtomicBool::new(false),
                    next_vacuum_run: AtomicI64::new(0),
                    next_ctx_cleanup: AtomicI64::new(0),
                    next_health_log_cleanup: AtomicI64::new(0),
                    cycles: Mutex::new(cleanup_cycles()),
                    ctx_load_running: AtomicBool::new(false),
                    maintenance: Default::default(),
                });
                let _ = done_tx.send(());
                let mut writer: Option<Writer> = None;
                // pending_ctx_cleanup_list, pending_uuid_deletion, pending_sql_statement: handed to the next job;
                // dropped at shutdown, as C frees them
                let mut pending = Pending::default();
                let (mut store_metadata, mut running) = (false, false);
                let mut next_tick = Instant::now() + TIMER_PERIOD;
                loop {
                    let cmd = match rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                        Ok(cmd) => Some(cmd),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    // a store asked for by command starts a job or is dropped; the timer's wish stays until a job
                    // starts
                    let mut store_now = false;
                    // metadata_event_loop_timer_cb()
                    let now = Instant::now();
                    if now >= next_tick {
                        next_tick = now + TIMER_PERIOD;
                        if shared.check_after.load(Ordering::Acquire) < now_realtime_s() {
                            store_metadata = true;
                        }
                    }
                    match cmd {
                        Some(Cmd::LoadHostContexts(hosts, vnodes)) => {
                            let load = Arc::new(CtxLoad {
                                writer: writer.clone(),
                                queue: MetaQueue(job_tx.clone()),
                                shared: Arc::clone(&shared),
                                vnodes,
                            });
                            shared.ctx_load_running.store(true, Ordering::Release);
                            let job_shared = Arc::clone(&shared);
                            if pool
                                .queue(move || {
                                    ctx_hosts_load(&hosts, cpus, stack_size, &load);
                                    job_shared.ctx_load_running.store(false, Ordering::Release);
                                })
                                .is_err()
                            {
                                shared.ctx_load_running.store(false, Ordering::Release);
                            }
                        }
                        Some(Cmd::StoreClaimId(meta, id)) => {
                            crate::meta_store::store_claim_id(meta.as_deref(), &id);
                        }
                        Some(Cmd::Writer(w)) => writer = Some(w),
                        Some(Cmd::StoreDone) => running = false,
                        Some(Cmd::DelDimension(uuid)) => {
                            pending.deletions.get_or_insert_with(Vec::new).push(uuid);
                        }
                        Some(Cmd::AddCtxCleanup(host_id, context)) => {
                            pending
                                .ctx_cleanup
                                .get_or_insert_with(Vec::new)
                                .push((host_id, context));
                        }
                        Some(Cmd::ExecuteStore(row)) => {
                            pending.statements.get_or_insert_with(Vec::new).push(*row);
                        }
                        Some(Cmd::AddHostAe(alerts, unique_id)) => {
                            pending.alerts.get_or_insert_with(Vec::new).push((alerts, unique_id));
                        }
                        Some(Cmd::Store) => store_now = true,
                        Some(Cmd::Shutdown) => {
                            shared.shutdown.store(true, Ordering::Release);
                            break;
                        }
                        None => {}
                    }
                    // METADATA_STORE: one job at a time; without a database the writer stays off (D61.8)
                    if (store_metadata || store_now) && !running && let Some(w) = &writer {
                        if !store_now {
                            store_metadata = false;
                        }
                        running = true;
                        let (w, shared, tx) = (w.clone(), Arc::clone(&shared), job_tx.clone());
                        let taken = std::mem::take(&mut pending);
                        if pool
                            .queue(move || {
                                store_job(&w, &shared, taken);
                                // the exit closes the database once METASYNC has seen this job end
                                drop(w);
                                let _ = tx.send(Cmd::StoreDone);
                            })
                            .is_err()
                        {
                            running = false;
                        }
                    }
                }
                // the shutdown waits for a running job, then stores what is still pending
                let deadline = Instant::now() + SHUTDOWN_WAIT;
                // and for a running context load
                while (running || shared.ctx_load_running.load(Ordering::Acquire))
                    && Instant::now() < deadline
                {
                    if let Ok(Cmd::StoreDone) = rx.recv_timeout(SHUTDOWN_POLL) {
                        running = false;
                    }
                }
                // what no job took is finalized without a step: rows queued and not stored yet are lost
                if let Some(statements) = pending.statements.take() {
                    store_sql_statements(None, statements);
                }
                if running {
                    nd_log!(
                        Source::Daemon,
                        Priority::Warning,
                        "METADATA: skipping the final host metadata flush - a metadata scan is still running"
                    );
                } else if let Some(w) = &writer {
                    crate::meta_store::store_hosts_metadata(&w.meta, &w.hosts, &shared.shutdown, false, true);
                }
                let _ = done_tx.send(());
                netdata_agent_log::thread_finished();
            })?;
        let _ = done.recv();
        Ok(MetaSync { tx, done, thread })
    }

    /// `metadata_queue_load_host_context()`: false when the command cannot be queued.
    pub fn load_host_contexts(&self, hosts: &Arc<Hosts>, vnodes: mpsc::Sender<()>) -> bool {
        self.tx
            .send(Cmd::LoadHostContexts(Arc::clone(hosts), vnodes))
            .is_ok()
    }

    /// The metadata writer's databases and hosts, once localhost exists; without them the writer stays off.
    pub fn set_writer(
        &self,
        meta: Arc<MetaDb>,
        context_db: Weak<ContextDb>,
        hosts: Arc<Hosts>,
        datafiles_present: bool,
        health: Arc<Health>,
    ) {
        let _ = self.tx.send(Cmd::Writer(Writer {
            meta,
            context_db,
            hosts,
            datafiles_present,
            health,
        }));
    }

    /// A handle that queues commands from other threads.
    pub fn queue(&self) -> MetaQueue {
        MetaQueue(self.tx.clone())
    }

    /// `metadata_sync_shutdown()`, at the exit step "stop metasync threads".
    pub fn shutdown(self) {
        if self.tx.send(Cmd::Shutdown).is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "METADATA: Failed to send a shutdown command"
            );
            return;
        }
        netdata_log_info!("METADATA: Submitted shutdown command, waiting for ACK");
        let _ = self.done.recv();
        if self.thread.join().is_err() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "METADATA: Failed to join synchronization thread"
            );
        } else {
            netdata_log_info!("METADATA: synchronization thread shutdown completed");
        }
    }
}

/// What a context load needs: the writer's databases (none without a metadata database), METASYNC's queue for the
/// cleanups it asks for, its shutdown flag, and where vnodes report.
struct CtxLoad {
    writer: Option<Writer>,
    queue: MetaQueue,
    shared: Arc<Shared>,
    vnodes: mpsc::Sender<()>,
}

/// A context-load slot's read-only handles (`hclt->db_meta_thread`, `db_context_thread`): opened by the first load on
/// the slot and handed to the next, as C does, since opening one parses the whole schema.
#[derive(Default)]
struct ThreadDbs {
    meta: Option<Connection>,
    context: Option<Connection>,
}

/// `restore_host_context()`: nothing once the exit started; else the host's contexts, read on the slot's read-only
/// handles (opened here when missing; the shared ones when they do not open), then the host no longer waits for them.
fn restore_host_context(host: &Host, load: &CtxLoad, dbs: &mut ThreadDbs) {
    if crate::shutdown::exiting() {
        return;
    }
    if let Some(w) = &load.writer {
        let cache_dir = w.meta.cache_dir();
        if dbs.meta.is_none() {
            dbs.meta = read::read_only(&MetaDb::path(cache_dir));
            dbs.context = read::read_only(&ContextDb::path(cache_dir));
        }
    }
    let started = now_ut();
    if let Some(w) = &load.writer {
        let (meta_thread, context_thread) = (dbs.meta.as_ref(), dbs.context.as_ref());
        let context_db = w.context_db.upgrade();
        let cleanup = |host_id, context| load.queue.ctx_host_cleanup(host_id, context);
        crate::ctxload::load_host_contexts(
            host,
            &crate::ctxload::Sources {
                meta: &w.meta,
                context_db: context_db.as_ref(),
                meta_thread,
                context_thread,
                cleanup: &cleanup,
            },
        );
    }
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Contexts for host {} loaded in {}",
        host.hostname(),
        duration(now_ut().saturating_sub(started))
    );
    host.clear_pending_context_load();
    host.pulse_status(0);
    // aclk_queue_node_info(): the host gets its ACLK sync configuration
    host.set_aclk_sync_config();
    if host.is_virtual_host_os() {
        let _ = load.vnodes.send(());
    }
}

/// `ctx_hosts_load()`, on a pool thread: the pending vnodes first, then the other pending hosts, most recently
/// connected first; each on a free `CTXLOAD` slot (one per CPU, when there is more than one), or here when none is.
fn ctx_hosts_load(hosts: &Hosts, cpus: usize, stack_size: usize, load: &Arc<CtxLoad>) {
    let started = now_ut();
    let max_threads = cpus.max(1);
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "Using {max_threads} threads for context loading"
    );
    let all = hosts.all();
    let mut order: Vec<Arc<Host>> = all
        .iter()
        .filter(|h| h.is_virtual_host_os() && h.is_pending_context_load())
        .cloned()
        .collect();
    let mut others: Vec<Arc<Host>> = all
        .iter()
        .filter(|h| !h.is_virtual_host_os() && h.is_pending_context_load())
        .cloned()
        .collect();
    others.sort_by_key(|h| std::cmp::Reverse(h.last_connected_s()));
    order.extend(others);
    // each slot's thread hands the slot's handles back when it ends, for the next thread on the slot
    let mut slots: Vec<Option<JoinHandle<ThreadDbs>>> =
        (0..if max_threads > 1 { max_threads } else { 0 })
            .map(|_| None)
            .collect();
    let mut own = ThreadDbs::default();
    let (mut delegated, mut direct) = (0, 0);
    let shutting_down = || load.shared.shutdown.load(Ordering::Acquire);
    for host in &order {
        if shutting_down() {
            break;
        }
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Loading context for host {}",
            host.hostname()
        );
        // cleanup_finished_threads(): a slot whose thread has finished is free again; up to 20 passes 10 ms apart
        let mut free = None;
        for pass in 0..20 {
            free = slots.iter().position(|thread| match thread {
                None => true,
                Some(thread) => thread.is_finished(),
            });
            if free.is_some() || slots.is_empty() {
                break;
            }
            if pass < 19 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        let free = free.map(|i| &mut slots[i]);
        let spawned = free.and_then(|slot| {
            let mut handed = slot
                .take()
                .map(|thread| thread.join().unwrap_or_default())
                .unwrap_or_default();
            let (host, load) = (Arc::clone(host), Arc::clone(load));
            let thread = std::thread::Builder::new()
                .name("CTXLOAD".into())
                .stack_size(stack_size)
                .spawn(move || {
                    netdata_agent_log::thread_created();
                    restore_host_context(&host, &load, &mut handed);
                    netdata_agent_log::thread_finished();
                    handed
                })
                .ok()?;
            *slot = Some(thread);
            Some(())
        });
        match spawned {
            Some(()) => delegated += 1,
            None => {
                direct += 1;
                restore_host_context(host, load, &mut own);
            }
        }
    }
    // the slots' handles close with their last thread
    for thread in slots.into_iter().flatten() {
        let _ = thread.join();
    }
    netdata_log_info!(
        "Contexts for {} hosts loaded: {delegated} delegated to {max_threads} threads, {direct} handled directly, in \
         {}.",
        delegated + direct,
        duration(now_ut().saturating_sub(started))
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use netdata_agent_rrd::host::HostInfo;
    use netdata_agent_rrd::mode::DbMode;

    fn info(hostname: &str, os: &str) -> HostInfo {
        HostInfo {
            hostname: hostname.into(),
            registry_hostname: hostname.into(),
            os: os.into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Alloc,
            history_entries: 5,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        }
    }

    fn hosts() -> Arc<Hosts> {
        let hosts = Arc::new(Hosts::new(Host::new(
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
            info("parent", "linux"),
        )));
        for (n, (name, os, last_connected)) in [
            ("old", "linux", 100),
            ("vnode", netdata_agent_rrd::host::VIRTUAL_HOST_OS, 50),
            ("recent", "linux", 300),
        ]
        .into_iter()
        .enumerate()
        {
            let host = hosts.add_archived(
                &format!("5a1e0000-0000-4000-8000-0000000000b{n}"),
                info(name, os),
                |_| {},
            );
            host.set_last_connected_s(last_connected);
        }
        hosts
    }

    /// A health plugin without rules.
    fn health() -> Arc<Health> {
        Health::init(Default::default(), Box::new(|_| {}))
    }

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            check_after: AtomicI64::new(0),
            shutdown: AtomicBool::new(false),
            next_vacuum_run: AtomicI64::new(0),
            next_ctx_cleanup: AtomicI64::new(0),
            next_health_log_cleanup: AtomicI64::new(0),
            cycles: Mutex::new(cleanup_cycles()),
            ctx_load_running: AtomicBool::new(false),
            maintenance: Default::default(),
        })
    }

    /// A context load without a metadata database, whose vnodes report on `vnodes`.
    fn ctx_load(vnodes: mpsc::Sender<()>) -> Arc<CtxLoad> {
        Arc::new(CtxLoad {
            writer: None,
            queue: MetaQueue(mpsc::channel().0),
            shared: shared(),
            vnodes,
        })
    }

    #[test]
    fn vnodes_load_first_then_the_most_recently_connected() {
        let hosts = hosts();
        let (tx, rx) = mpsc::channel();
        let load = ctx_load(tx);
        let (_, records) =
            netdata_agent_log::capture(|| ctx_hosts_load(&hosts, 1, 256 * 1024, &load));
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        let loading: Vec<&str> = messages
            .iter()
            .filter_map(|m| m.strip_prefix("Loading context for host "))
            .collect();
        assert_eq!(loading, ["vnode", "recent", "old"]);
        assert!(messages.last().unwrap().starts_with(
            "Contexts for 3 hosts loaded: 0 delegated to 1 threads, 3 handled directly, in "
        ));
        assert!(hosts.all().iter().all(|h| !h.is_pending_context_load()));
        assert_eq!(rx.try_iter().count(), 1);
    }

    /// Once METASYNC shuts down no host is dispatched, and C's record counts the dispatched ones only.
    #[test]
    fn a_shutdown_stops_the_context_loads() {
        let hosts = hosts();
        let (tx, _rx) = mpsc::channel();
        let load = ctx_load(tx);
        load.shared.shutdown.store(true, Ordering::Release);
        let (_, records) =
            netdata_agent_log::capture(|| ctx_hosts_load(&hosts, 1, 256 * 1024, &load));
        let summary = records
            .into_iter()
            .filter_map(|r| r.message)
            .next_back()
            .unwrap();
        assert!(
            summary.starts_with(
                "Contexts for 0 hosts loaded: 0 delegated to 1 threads, 0 handled directly, in "
            ),
            "{summary}"
        );
    }

    #[test]
    fn hosts_load_on_ctxload_threads_while_slots_last() {
        let hosts = hosts();
        let (tx, rx) = mpsc::channel();
        let load = ctx_load(tx);
        let (_, records) =
            netdata_agent_log::capture(|| ctx_hosts_load(&hosts, 2, 256 * 1024, &load));
        let summary = records
            .into_iter()
            .filter_map(|r| r.message)
            .next_back()
            .unwrap();
        // two slots: a third host waits for a finished thread or runs here
        assert!(
            summary.starts_with("Contexts for 3 hosts loaded: "),
            "{summary}"
        );
        assert!(summary.contains(" delegated to 2 threads, "), "{summary}");
        assert!(hosts.all().iter().all(|h| !h.is_pending_context_load()));
        assert_eq!(rx.try_iter().count(), 1);
    }

    fn meta_with_dimensions(dir: &std::path::Path) -> Arc<MetaDb> {
        let meta = Arc::new(
            MetaDb::open(
                dir,
                &netdata_agent_metadata::open::SqliteSettings::default(),
            )
            .unwrap(),
        );
        meta.lock()
            .execute_batch(
                "INSERT INTO dimension (dim_id, chart_id, id, name) VALUES \
                 (x'01010101010101010101010101010101', x'02', 'a', 'a'), \
                 (x'03030303030303030303030303030303', x'02', 'b', 'b'), \
                 (x'04040404040404040404040404040404', x'02', 'c', 'c')",
            )
            .unwrap();
        meta
    }

    fn dimensions(meta: &MetaDb) -> i64 {
        meta.lock()
            .query_row("SELECT count(*) FROM dimension", [], |r| r.get(0))
            .unwrap()
    }

    /// Hosts over an engine of two empty tiers in these directories.
    fn hosts_with_engine(dirs: &[tempfile::TempDir]) -> Arc<Hosts> {
        use netdata_agent_rrd::storage::StorageLayout;
        use netdata_agent_storage::dbengine::engine::cache::CacheConfig;
        use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
        use netdata_agent_storage::dbengine::engine::mrg::Mrg;
        use netdata_agent_storage::dbengine::engine::query::{Dbengine, EngineConfig};
        let mrg = Mrg::new();
        let tiers = dirs
            .iter()
            .enumerate()
            .map(|(tier, dir)| {
                let cfg = TierConfig::new(tier, dir.path().to_path_buf());
                load(cfg, &mrg, 1_800_000_000).unwrap()
            })
            .collect();
        let engine = Dbengine::new(
            mrg,
            tiers,
            EngineConfig {
                caches: CacheConfig::new(1 << 20, 1 << 20),
                ..EngineConfig::new(|| 1_800_000_000)
            },
        );
        Arc::new(Hosts::with_storage(
            Host::new(
                "5a1e0000-0000-4000-8000-0000000000a0",
                true,
                info("l", "linux"),
            ),
            Arc::new(StorageLayout::new(Some(engine))),
        ))
    }

    /// Freed dimensions' rows go at the next job: without the dbengine unless datafiles were found at start, with it
    /// unless a tier holds a first time for them (`dimension_can_be_deleted()`); none once a shutdown began. C's
    /// record counts them either way.
    #[test]
    fn pending_dimensions_are_deleted_as_c() {
        let dir = tempfile::tempdir().unwrap();
        let meta = meta_with_dimensions(dir.path());
        let shared = shared();
        let mut writer = Writer {
            meta: Arc::clone(&meta),
            context_db: Weak::new(),
            hosts: hosts(),
            datafiles_present: true,
            health: health(),
        };
        delete_pending_dimensions(&writer, &shared, vec![[1; 16]]);
        assert_eq!(dimensions(&meta), 3, "dbengine data on disk keeps the rows");
        writer.datafiles_present = false;
        let ((), records) = netdata_agent_log::capture(|| {
            delete_pending_dimensions(&writer, &shared, vec![[1; 16], [9; 16]])
        });
        assert_eq!(dimensions(&meta), 2);
        let message = records
            .into_iter()
            .filter_map(|r| r.message)
            .next()
            .unwrap();
        assert!(
            message.starts_with("Processed 2 dimension delete items in ")
                && message.ends_with(" ms"),
            "{message}"
        );
        // with the dbengine: a first time on tier 1 keeps the row, a zero first time or no entry does not
        let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
        writer.hosts = hosts_with_engine(&dirs);
        writer.datafiles_present = true;
        let mrg = &writer.hosts.storage().dbengine().unwrap().mrg;
        let _kept = mrg.add_and_acquire(&[3; 16], 1, 100, 200, 1).0;
        let _zero = mrg.add_and_acquire(&[4; 16], 0, 0, 0, 0).0;
        delete_pending_dimensions(&writer, &shared, vec![[3; 16], [4; 16]]);
        assert_eq!(dimensions(&meta), 1);
        let left: Vec<u8> = meta
            .lock()
            .query_row("SELECT dim_id FROM dimension", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, [3; 16]);
        shared.shutdown.store(true, Ordering::Release);
        writer.hosts = hosts();
        writer.datafiles_present = false;
        delete_pending_dimensions(&writer, &shared, vec![[3; 16]]);
        assert_eq!(dimensions(&meta), 1, "nothing goes during a shutdown");
    }

    /// The context cleanup scan checks with C's plain `dimension_can_be_deleted()`: without the dbengine no row goes,
    /// even with no datafiles, where a freed dimension's row does (D75.12).
    #[test]
    fn the_scan_keeps_every_row_without_the_dbengine() {
        let dir = tempfile::tempdir().unwrap();
        let writer = Writer {
            meta: meta_with_dimensions(dir.path()),
            context_db: Weak::new(),
            hosts: hosts(),
            datafiles_present: false,
            health: health(),
        };
        assert!(!dimension_can_be_deleted(&writer, &[1; 16]));
        assert!(freed_dimension_can_be_deleted(&writer, &[1; 16]));
    }

    /// `run_metadata_cleanup()`: the first job arms the scan 5 s later; a due scan consumes the queued context of a
    /// host, its dimensions going only with the dbengine and no retention, and arms the next 300 s after it.
    #[test]
    fn the_context_scan_runs_after_5_s_then_every_300_s() {
        let dirs: Vec<_> = (0..2).map(|_| tempfile::tempdir().unwrap()).collect();
        for engine in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let hosts = if engine {
                hosts_with_engine(&dirs)
            } else {
                hosts()
            };
            let host_id = crate::meta_store::host_id(&hosts.all()[0]).unwrap();
            let meta = meta_with_dimensions(dir.path());
            meta.lock()
                .execute(
                    "INSERT INTO chart (chart_id, host_id, context) VALUES (x'02', ?, 'ctx.a')",
                    [&host_id[..]],
                )
                .unwrap();
            meta.schedule_host_ctx_cleanup(&[(host_id, "ctx.a".into())], || false);
            let writer = Writer {
                meta: Arc::clone(&meta),
                context_db: Weak::new(),
                hosts,
                datafiles_present: false,
                health: health(),
            };
            let queued = |meta: &MetaDb| -> i64 {
                meta.lock()
                    .query_row("SELECT count(*) FROM ctx_metadata_cleanup", [], |r| {
                        r.get(0)
                    })
                    .unwrap()
            };
            let shared = shared();
            let now = now_realtime_s();
            run_metadata_cleanup(&writer, &shared);
            let armed = shared.next_ctx_cleanup.load(Ordering::Acquire);
            assert!((now + 5..=now + 6).contains(&armed), "{armed}");
            assert_eq!((dimensions(&meta), queued(&meta)), (3, 1));
            shared.next_ctx_cleanup.store(1, Ordering::Release);
            run_metadata_cleanup(&writer, &shared);
            let kept = if engine { 0 } else { 3 };
            assert_eq!(
                (dimensions(&meta), queued(&meta)),
                (kept, 0),
                "engine {engine}"
            );
            let next = shared.next_ctx_cleanup.load(Ordering::Acquire);
            assert!((now + 300..=now + 301).contains(&next), "{next}");
        }
    }

    /// The alert log between HEALTH and a store job, on a real metadata database. A host's first pass queues the
    /// save of its alert's link entry and asks for a job; what the pass logs itself (the alert's first status) it
    /// saves at once, with the link entry that status replaces. While a save is pending the next pass is postponed.
    /// The job's step saves what was queued (here an update of a row HEALTH inserted) and records two per save, and
    /// no save is pending afterwards: every entry the host logged has its row, and the alarm its row of the queue
    /// toward the Cloud. A new process on the same database loads the alarm's last entry, and its alert keeps its
    /// alarm id.
    #[test]
    fn a_job_saves_the_alert_entries_health_queued() {
        use netdata_agent_health::pass::Pass;
        use netdata_agent_health::readfile::health_readfile;
        use netdata_agent_health::store::alert_hash_row;
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType, flags};
        use netdata_agent_rrd::host::pending_flags;

        let dir = tempfile::tempdir().unwrap();
        let meta = Arc::new(MetaDb::open(dir.path(), &Default::default()).unwrap());
        let base = info("parent", "linux");
        let ram = HostInfo { health_enabled: true, db_mode: DbMode::Ram, history_entries: 3600, ..base };
        let hosts = Arc::new(Hosts::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, ram)));
        let host = Arc::clone(hosts.localhost());
        host.set_aclk_sync_config();
        let (chart, _) = host.charts().create(&ChartSpec {
            type_: "t",
            id: "c",
            name: None,
            family: Some("f"),
            context: Some("t.ctx"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every: 1,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        });
        chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
        let now = now_realtime_s();
        chart.update_collection(|collection| {
            collection.counter_done = 3;
            collection.last_collected = (now, 0);
        });

        let rules = dir.path().join("t.conf");
        std::fs::write(&rules, "template: t_one\n on: t.ctx\n calc: 1\n every: 1s\n warn: $this > 5\n").unwrap();
        let plugin = || {
            let meta = Arc::clone(&meta);
            let health = Health::init(
                Default::default(),
                Box::new(move |rule| assert!(meta.store_alert_config(&alert_hash_row(rule)))),
            );
            assert!(health_readfile(&health, rules.as_os_str().as_encoded_bytes(), false));
            health
        };
        let health = plugin();
        let (queue, unread) = MetaQueue::unread();
        let env = crate::health::LiveEnv::new(Arc::clone(&hosts), Default::default(), Some(&meta), queue);
        crate::health::mark_health_thread();
        let pass = |health: &Health| {
            let mut next_run = now + 10;
            let ((), records) = netdata_agent_log::capture(|| {
                let pass = Pass { now, apply_hibernation_delay: false, next_run: &mut next_run, gate: &|| true };
                health.host_pass(&host, pass, &env, &now_realtime_s, &|| true);
            });
            records.into_iter().filter_map(|record| record.message).collect::<Vec<String>>()
        };
        let rows = |table: &str| -> i64 {
            meta.lock().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
        };

        // the first pass: the links' saves are queued, a job is asked for, and HEALTH saved its own entries
        pass(&health);
        let alerts = health.host(&host).unwrap();
        let (queued, stores) = unread.alert_commands();
        assert!(!queued.is_empty(), "the link entries' saves");
        assert_eq!((alerts.pending_transitions(), stores), (queued.len() as i32, 1));
        // the alert's first status is saved by HEALTH with the link entry it replaces: the queued save of that link
        // entry will find it saved, and update it
        let logged = alerts.transitions() as i64;
        assert_eq!((logged, rows("health_log_detail")), (2, 2));
        let alarm_id = alerts.alerts()[0].id;

        // saves are pending: the next pass is postponed
        let postponed = pass(&health);
        assert_eq!(postponed, ["Host \"parent\" has pending alert transitions to save, postponing health checks"]);
        assert_eq!(unread.alert_commands().0.len(), 0);

        // the job's step
        let writer = Writer {
            meta: Arc::clone(&meta),
            context_db: Weak::new(),
            hosts: Arc::clone(&hosts),
            datafiles_present: false,
            health: Arc::clone(&health),
        };
        let saves = queued.len();
        let pending = Pending { statements: Some(vec![row(3)]), alerts: Some(queued), ..Pending::default() };
        let ((), records) = netdata_agent_log::capture(|| store_job(&writer, &shared(), pending));
        let stored = records.into_iter().filter_map(|record| record.message).filter(|m| m.starts_with("Stored "));
        let stored: Vec<String> = stored.map(|m| m.split(" in ").next().unwrap().to_owned()).collect();
        let transitions = format!("Stored and processed {} alert transitions", saves * 2);
        assert_eq!(stored, ["Stored and processed 1 sql statements".to_owned(), transitions]);
        assert_eq!(alerts.pending_transitions(), 0);
        assert_eq!((rows("health_log_detail"), rows("health_log"), rows("alert_queue")), (logged, 1, 1));

        // a new process: the host's first pass loads the alarm's last entry, and the alert takes its alarm id again
        drop(alerts);
        let health = plugin();
        host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION);
        chart.flags_set_and_clear(flags::PENDING_HEALTH_INITIALIZATION, 0);
        let records = pass(&health);
        let loaded = "[parent]: Table health_log, loaded 1 alarm entries, errors in 0 entries.";
        assert!(records.iter().any(|record| record == loaded), "{records:?}");
        assert_eq!(health.host(&host).unwrap().alerts()[0].id, alarm_id);
    }

    /// `commit_alert_transitions()`: a store asked for by command starts a job at once, where the timer's first store
    /// comes 5 s after the thread's start.
    #[test]
    fn a_store_command_starts_a_job_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let meta = meta_with_dimensions(dir.path());
        let pool = WorkPool::new(1, 256 * 1024);
        let sync = MetaSync::start(&pool, 1, 256 * 1024).unwrap();
        sync.set_writer(Arc::clone(&meta), Weak::new(), hosts(), false, health());
        let queue = sync.queue();
        queue.execute_store_statement(row(7));
        queue.store();
        let deadline = Instant::now() + Duration::from_secs(3);
        while stored_rows(&meta).is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(stored_rows(&meta), [7]);
        sync.shutdown();
    }

    /// `cleanup_health_log()`: the first job that reaches it arms it 1800 s ahead and does nothing. When it is due,
    /// each host's alert log loses the entries older than the host's retention that a newer entry replaced (a host
    /// whose health never ran has a retention of 0), then the alarms of hosts the table `host` does not have go,
    /// with their entries; and it is armed 3600 s ahead.
    #[test]
    fn the_health_log_cleanup_runs_after_1800_s_then_every_3600_s() {
        use netdata_agent_metadata::health_log::EntryRow;
        let dir = tempfile::tempdir().unwrap();
        let meta = Arc::new(MetaDb::open(dir.path(), &Default::default()).unwrap());
        let hosts = hosts();
        let host = Arc::clone(hosts.localhost());
        let host_id = crate::meta_store::host_id(&host).unwrap();
        meta.lock()
            .execute("INSERT INTO host (host_id, hostname) VALUES (?1, 'parent')", [&host_id[..]])
            .unwrap();
        let (now, hash) = (now_realtime_s(), [0xab; 16]);
        let transitions = [[1u8; 16], [2; 16], [3; 16]];
        let entry = |unique_id: u32, alarm_id: u32, updated_by_id: u32| EntryRow {
            unique_id,
            alarm_id,
            alarm_event_id: unique_id,
            config_hash_id: &hash,
            transition_id: &transitions[unique_id as usize - 1],
            updated_by_id,
            updates_id: 0,
            when: now - 100,
            duration: 0,
            non_clear_duration: 0,
            flags: 0,
            exec_run_timestamp: 0,
            delay_up_to_timestamp: now - 100,
            name: Some(b"an_alarm"),
            chart: Some(b"t.c"),
            chart_context: Some(b"t.ctx"),
            chart_name: Some(b"t.c"),
            exec: None,
            recipient: None,
            units: None,
            info: None,
            summary: None,
            exec_code: 0,
            new_status: 1,
            old_status: 0,
            delay: 0,
            new_value: 1.0,
            old_value: f64::NAN,
            last_repeat: 0,
            global_id: u64::from(unique_id),
        };
        // localhost's alarm 7: entry 1, which entry 2 replaced, and entry 2, its last; alarm 8 of a host the table
        // `host` does not have
        assert!(meta.health_alarm_log_insert("parent", &host_id, &entry(1, 7, 0), false));
        assert!(meta.health_alarm_log_insert("parent", &host_id, &entry(2, 7, 0), false));
        meta.health_alarm_log_update("parent", &entry(1, 7, 2));
        assert!(meta.health_alarm_log_insert("gone", &[0xee; 16], &entry(3, 8, 0), false));
        let rows = |table: &str| -> i64 {
            meta.lock().query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap()
        };
        let writer = Writer {
            meta: Arc::clone(&meta),
            context_db: Weak::new(),
            hosts,
            datafiles_present: false,
            health: health(),
        };
        let shared = shared();

        cleanup_health_log(&writer, &shared);
        let armed = shared.next_health_log_cleanup.load(Ordering::Acquire);
        assert!((now + 1800..=now + 1802).contains(&armed), "{armed} at {now}");
        assert_eq!((rows("health_log"), rows("health_log_detail")), (2, 3));

        shared.next_health_log_cleanup.store(now - 1, Ordering::Release);
        cleanup_health_log(&writer, &shared);
        let armed = shared.next_health_log_cleanup.load(Ordering::Acquire);
        assert!((now + 3600..=now + 3602).contains(&armed), "{armed} at {now}");
        assert_eq!((rows("health_log"), rows("health_log_detail")), (1, 1));
        let left: i64 = meta.lock().query_row("SELECT unique_id FROM health_log_detail", [], |row| row.get(0)).unwrap();
        assert_eq!(left, 2);
    }

    /// A job stores the context cleanups before it deletes the freed dimensions, each list with C's record; the items
    /// met during a shutdown are not stored, and no list gives no record.
    #[test]
    fn a_job_stores_cleanups_before_deletions() {
        let dir = tempfile::tempdir().unwrap();
        let meta = meta_with_dimensions(dir.path());
        let shared = shared();
        let writer = Writer {
            meta: Arc::clone(&meta),
            context_db: Weak::new(),
            hosts: hosts(),
            datafiles_present: false,
            health: health(),
        };
        let cleanups = |meta: &MetaDb| -> i64 {
            meta.lock()
                .query_row("SELECT count(*) FROM ctx_metadata_cleanup", [], |r| {
                    r.get(0)
                })
                .unwrap()
        };
        let debug = |records: Vec<netdata_agent_log::Captured>| -> Vec<String> {
            records
                .into_iter()
                .filter_map(|r| r.message)
                .filter(|m| m.starts_with("Stored ") || m.starts_with("Processed "))
                .map(|m| m.split(" in ").next().unwrap().to_string())
                .collect()
        };
        let ((), records) = netdata_agent_log::capture(|| {
            store_job(
                &writer,
                &shared,
                Pending {
                    ctx_cleanup: Some(vec![([0xaa; 16], "ctx.a".into())]),
                    deletions: Some(vec![[1; 16]]),
                    ..Pending::default()
                },
            )
        });
        assert_eq!(
            debug(records),
            [
                "Stored 1 host context cleanup items",
                "Processed 1 dimension delete items"
            ]
        );
        assert_eq!((cleanups(&meta), dimensions(&meta)), (1, 2));
        let ((), records) =
            netdata_agent_log::capture(|| store_job(&writer, &shared, Pending::default()));
        assert_eq!(debug(records), Vec::<String>::new());
        shared.shutdown.store(true, Ordering::Release);
        store_ctx_cleanup(&writer, &shared, vec![([0xbb; 16], "ctx.b".into())]);
        assert_eq!(cleanups(&meta), 1, "skipped during a shutdown");
    }
    /// An alert configuration's row, told from the others by the byte its hash is made of.
    fn row(n: u8) -> AlertHashRow {
        let mut rule = netdata_agent_health::prototype::Rule::default();
        rule.config.hash_id = [n; 16];
        rule.config.name = Some(format!("alert{n}").into_bytes());
        netdata_agent_health::store::alert_hash_row(&rule)
    }

    /// The hashes' bytes of the `alert_hash` rows, in rowid order.
    fn stored_rows(meta: &MetaDb) -> Vec<u8> {
        let c = meta.lock();
        let mut stmt = c.prepare("SELECT hash_id FROM alert_hash ORDER BY rowid").unwrap();
        let hashes = stmt.query_map([], |r| r.get::<_, Vec<u8>>(0)).unwrap();
        hashes.map(|hash| hash.unwrap()[0]).collect()
    }

    /// `start_metadata_hosts()` steps the queued statements before anything else it stores, each on its own and
    /// in queue order; what a shutdown finds queued is dropped, with the same record.
    #[test]
    fn a_job_steps_the_queued_statements_first_and_a_shutdown_drops_them() {
        let dir = tempfile::tempdir().unwrap();
        let meta = meta_with_dimensions(dir.path());
        let writer = Writer {
            meta: Arc::clone(&meta),
            context_db: Weak::new(),
            hosts: hosts(),
            datafiles_present: false,
            health: health(),
        };
        let stored = |records: Vec<netdata_agent_log::Captured>| -> Vec<String> {
            records
                .into_iter()
                .filter_map(|r| r.message)
                .filter(|m| m.starts_with("Stored "))
                .map(|m| m.split(" in ").next().unwrap().to_string())
                .collect()
        };
        let ((), records) = netdata_agent_log::capture(|| {
            store_job(
                &writer,
                &shared(),
                Pending {
                    ctx_cleanup: Some(vec![([0xaa; 16], "ctx.a".into())]),
                    deletions: None,
                    // the second hash again: its row is replaced and takes a new rowid
                    statements: Some(vec![row(3), row(1), row(2), row(1)]),
                    alerts: None,
                },
            )
        });
        assert_eq!(
            stored(records),
            ["Stored and processed 4 sql statements", "Stored 1 host context cleanup items"]
        );
        assert_eq!(stored_rows(&meta), [3, 2, 1]);

        let ((), records) = netdata_agent_log::capture(|| store_sql_statements(None, vec![row(8), row(9)]));
        assert_eq!(stored(records), ["Stored and processed 2 sql statements"]);
        assert_eq!(stored_rows(&meta), [3, 2, 1]);
    }

    #[test]
    fn a_queued_statement_waits_in_the_queue_for_a_job() {
        let (queue, unread) = MetaQueue::unread();
        queue.execute_store_statement(row(5));
        queue.execute_store_statement(row(4));
        let hashes: Vec<u8> = unread.statements().iter().map(|row| row.hash_id[0]).collect();
        assert_eq!(hashes, [5, 4]);
    }
}
