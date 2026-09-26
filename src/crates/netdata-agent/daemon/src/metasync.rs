//! The METASYNC thread of `src/database/sqlite/sqlite_metadata.c` (`metadata_event_loop()`): its lifecycle records;
//! the metadata writer, whose store job it hands to the `UV_WORKER` pool 6 s after it starts and then about every 6 s
//! (a 1 s timer, and 5 s after each job ends), with a final store at shutdown (D61); the context cleanups and the
//! freed dimensions' rows that job writes first; the claim id of an unclaimed start; and the context load of the
//! archived hosts, also on the pool (`ctx_hosts_load()`), one `CTXLOAD` thread per host while slots last (D59.5),
//! reading a dbengine host's contexts from SQL (D64).

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use netdata_agent_evloop::work::WorkPool;
use netdata_agent_log::{Priority, Source, nd_log, netdata_log_info};
use netdata_agent_metadata::open::{ContextDb, MetaDb};
use netdata_agent_metadata::read;
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_text::duration::duration_to_string;

use crate::startup::now_ut;

/// `NETDATA_VIRTUAL_HOST`.
const VIRTUAL_HOST_OS: &str = "Netdata Virtual Host 1.0";

/// `METADATA_HOST_CHECK_FIRST_CHECK`, `METADATA_HOST_CHECK_INTERVAL`: seconds before the next store job may run.
const HOST_CHECK_FIRST_S: i64 = 5;
const HOST_CHECK_INTERVAL_S: i64 = 5;

/// The loop's timer (`TIMER_INITIAL_PERIOD_MS`, `TIMER_REPEAT_PERIOD_MS`).
const TIMER_PERIOD: Duration = Duration::from_secs(1);

/// `MAX_SHUTDOWN_TIMEOUT_SECONDS`, `SHUTDOWN_SLEEP_INTERVAL_MS`: how long the shutdown waits for a running job.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(15);
const SHUTDOWN_POLL: Duration = Duration::from_millis(100);

pub(crate) fn now_realtime_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// What the loop and its store jobs share (`struct meta_config_s`).
struct Shared {
    /// `metadata_check_after`: the timer asks for a store once the wall clock is past it.
    check_after: AtomicI64,
    /// `shutdown_requested`.
    shutdown: AtomicBool,
    /// `next_vacuum_run` of `run_metadata_cleanup()`.
    next_vacuum_run: AtomicI64,
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
}

/// `dimension_can_be_deleted()` or, without the dbengine, whether no datafiles were found at start: a freed
/// dimension's row goes when no tier holds retention for it.
fn dimension_can_be_deleted(writer: &Writer, uuid: &[u8; 16]) -> bool {
    match writer.hosts.storage().dbengine() {
        None => !writer.datafiles_present,
        Some(engine) => (0..engine.tiers.len()).all(|tier| {
            engine
                .mrg
                .retention_by_uuid(uuid, tier)
                .is_none_or(|r| r.first_time_s <= 0)
        }),
    }
}

/// `do_pending_uuid_deletion()`: the rows of the dimensions freed since the last job.
fn delete_pending_dimensions(writer: &Writer, shared: &Shared, pending: Vec<[u8; 16]>) {
    let started = now_ut();
    for uuid in &pending {
        if !shared.shutdown.load(Ordering::Acquire) && dimension_can_be_deleted(writer, uuid) {
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
}

/// `start_metadata_hosts()`, on a pool thread: the context cleanups, the freed dimensions, the hosts' pending
/// metadata, then the database's upkeep, and the next store no sooner than 5 s from now. `run_maintenace()` (the
/// service thread's host cleanup) is not ported yet.
fn store_job(writer: &Writer, shared: &Shared, pending: Pending) {
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
    // run_metadata_cleanup(): the context cleanup and the long cycles come with S5 (D61.5)
    if !shared.shutdown.load(Ordering::Acquire) {
        let mut next = shared.next_vacuum_run.load(Ordering::Acquire);
        writer.meta.vacuum(&mut next, now_realtime_s());
        shared.next_vacuum_run.store(next, Ordering::Release);
        writer.meta.wal_checkpoint();
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

fn is_vnode(host: &Host) -> bool {
    host.info().os == VIRTUAL_HOST_OS
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
                });
                let _ = done_tx.send(());
                let mut writer: Option<Writer> = None;
                // pending_ctx_cleanup_list, pending_uuid_deletion: handed to the next job; dropped at shutdown, as C
                // frees them
                let mut pending = Pending::default();
                let (mut store_metadata, mut running) = (false, false);
                let mut next_tick = Instant::now() + TIMER_PERIOD;
                loop {
                    let cmd = match rx.recv_timeout(next_tick.saturating_duration_since(Instant::now())) {
                        Ok(cmd) => Some(cmd),
                        Err(RecvTimeoutError::Timeout) => None,
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
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
                            let _ = pool.queue(move || ctx_hosts_load(&hosts, cpus, stack_size, &load));
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
                        Some(Cmd::Shutdown) => {
                            shared.shutdown.store(true, Ordering::Release);
                            break;
                        }
                        None => {}
                    }
                    // METADATA_STORE: one job at a time; without a database the writer stays off (D61.8)
                    if store_metadata && !running && let Some(w) = &writer {
                        store_metadata = false;
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
                while running && Instant::now() < deadline {
                    if let Ok(Cmd::StoreDone) = rx.recv_timeout(SHUTDOWN_POLL) {
                        running = false;
                    }
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
    ) {
        let _ = self.tx.send(Cmd::Writer(Writer {
            meta,
            context_db,
            hosts,
            datafiles_present,
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

/// `restore_host_context()`: nothing once the exit started; else the host's contexts, read on read-only handles of
/// its own when they open (the shared ones otherwise), then the host no longer waits for them.
fn restore_host_context(host: &Host, load: &CtxLoad) {
    if crate::shutdown::exiting() {
        return;
    }
    let started = now_ut();
    if let Some(w) = &load.writer {
        let cache_dir = w.meta.cache_dir();
        let meta_thread = read::read_only(&MetaDb::path(cache_dir));
        let context_thread = read::read_only(&ContextDb::path(cache_dir));
        let context_db = w.context_db.upgrade();
        let cleanup = |host_id, context| load.queue.ctx_host_cleanup(host_id, context);
        crate::ctxload::load_host_contexts(
            host,
            &crate::ctxload::Sources {
                meta: &w.meta,
                context_db: context_db.as_ref(),
                meta_thread: meta_thread.as_ref(),
                context_thread: context_thread.as_ref(),
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
    if is_vnode(host) {
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
        .filter(|h| is_vnode(h) && h.is_pending_context_load())
        .cloned()
        .collect();
    let mut others: Vec<Arc<Host>> = all
        .iter()
        .filter(|h| !is_vnode(h) && h.is_pending_context_load())
        .cloned()
        .collect();
    others.sort_by_key(|h| std::cmp::Reverse(h.last_connected_s()));
    order.extend(others);
    let mut slots: Vec<Option<JoinHandle<()>>> = (0..if max_threads > 1 { max_threads } else { 0 })
        .map(|_| None)
        .collect();
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
        // cleanup_finished_threads(): a slot whose thread has finished is free again
        let free = slots.iter_mut().find(|slot| match slot {
            None => true,
            Some(thread) => thread.is_finished(),
        });
        let spawned = free.and_then(|slot| {
            if let Some(thread) = slot.take() {
                let _ = thread.join();
            }
            let (host, load) = (Arc::clone(host), Arc::clone(load));
            let thread = std::thread::Builder::new()
                .name("CTXLOAD".into())
                .stack_size(stack_size)
                .spawn(move || {
                    netdata_agent_log::thread_created();
                    restore_host_context(&host, &load);
                    netdata_agent_log::thread_finished();
                })
                .ok()?;
            *slot = Some(thread);
            Some(())
        });
        match spawned {
            Some(()) => delegated += 1,
            None => {
                direct += 1;
                restore_host_context(host, load);
            }
        }
    }
    for thread in slots.into_iter().flatten() {
        let _ = thread.join();
    }
    netdata_log_info!(
        "Contexts for {} hosts loaded: {delegated} delegated to {max_threads} threads, {direct} handled directly, in \
         {}.",
        order.len(),
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
            ("vnode", VIRTUAL_HOST_OS, 50),
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

    fn shared() -> Arc<Shared> {
        Arc::new(Shared {
            check_after: AtomicI64::new(0),
            shutdown: AtomicBool::new(false),
            next_vacuum_run: AtomicI64::new(0),
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
        use netdata_agent_storage::dbengine::engine::cache::{ExtentCache, MainCache};
        use netdata_agent_storage::dbengine::engine::load::{TierConfig, load};
        use netdata_agent_storage::dbengine::engine::mrg::Mrg;
        use netdata_agent_storage::dbengine::engine::query::{Dbengine, TierData};
        let mrg = Mrg::new();
        let tiers = dirs
            .iter()
            .enumerate()
            .map(|(tier, dir)| {
                let cfg = TierConfig {
                    tier,
                    path: dir.path().to_path_buf(),
                    direct_io: false,
                    max_disk_space: 0,
                    journal_check: false,
                };
                TierData::new(load(cfg, &mrg, 1_800_000_000).unwrap())
            })
            .collect();
        let engine = Arc::new(Dbengine {
            mrg,
            tiers,
            main: MainCache::new(1 << 20),
            extents: ExtentCache::new(1 << 20),
            pool: None,
            update_every_s: 1,
        });
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
}
