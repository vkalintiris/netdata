//! Health in the daemon: the plugin's start (`health_plugin_init()`, `src/health/health.c`), which loads the alert
//! configuration, and the `HEALTH` thread, ported from `health_main()` and `health_event_loop()`
//! (`src/health/health_event_loop.c`). A pass visits every host: those health runs for get their charts' alerts
//! linked and evaluated. C runs the loop with health off too, and a disconnected child is archived only after
//! more than 10 of its passes (D93.2). It keeps C's pacing: a pass at most every `[health] run at least every`
//! seconds or when the next alert is due, waited in 1 s sleeps, and none while a backfill or more than one user
//! query runs.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::{Duration, Instant};

use netdata_agent_dyncfg::Dyncfg;
use netdata_agent_dyncfg::inline::{InlineCallback, InlineSpec};
use netdata_agent_dyncfg::model::Status as NodeStatus;
use netdata_agent_health::alert::Status;
use netdata_agent_health::alerts::HostAlerts;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::dyncfg::{Cloud, Ctx, NodeSpec, Nodes};
use netdata_agent_health::entry::Entry;
use netdata_agent_health::notify::{Execution, Waiting};
use netdata_agent_health::pass::{ChartFacts, Env, Pass};
use netdata_agent_health::store::alert_hash_row;
use netdata_agent_health::{Health, StoreSink};
use netdata_agent_inicfg::Config;
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_metadata::health_log::LoadedRow;
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_nrpc::access;
use netdata_agent_query::execute::Control;
use netdata_agent_query::grouping::Windows;
use netdata_agent_query::value::{ValueRequest, ValueResult, chart_value};
use netdata_agent_rrd::chart::{Chart, flags};
use netdata_agent_rrd::clock::{now_realtime_s, now_realtime_ut};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_rrd::pulse::QuerySource;
use netdata_agent_rrd::storage::{AlertClass, AlertView, ChartAlert, HealthEvent};
use netdata_agent_rrd::stream_control;
use netdata_agent_spawn::client::Waited;
use netdata_agent_spawn::popen::Popen;
use netdata_agent_text::print::print_uuid_lower;

use crate::conf::{Conf, health_config_dirs};
use crate::heartbeat::{Phase, Thread};
use crate::metasync::MetaQueue;
use crate::shutdown;

/// `health_plugin_init()`, in `rrd_init()` once localhost has its `config` function: the empty prototype store, and
/// with health on the two `health.d` trees read into it. With health off nothing else happens: no directory key is
/// read, no file opened, no record written, no row queued.
///
/// Each rule C accepts gets its `alert_hash` row: bound on this thread, stepped by METASYNC's next store job.
/// Without a database C's statement cannot be prepared, and says so once per rule.
///
/// The load is `health_reload_prototypes()`: health's DynCfg nodes are registered after it (the template, whose
/// registration brings the saved jobs back, then a job per alert name), on this thread.
pub fn plugin_init(
    conf: &mut Conf,
    config: HealthConfig,
    database: bool,
    queue: MetaQueue,
    dynamic: &Dynamic<'_>,
) -> Arc<Plugin> {
    let store: StoreSink = if database {
        Box::new(move |rule| queue.execute_store_statement(alert_hash_row(rule)))
    } else {
        Box::new(|_| crate::meta_store::no_database("sql_alert_store_config"))
    };
    let health = Health::init(config, store);
    let link = DyncfgLink::new(&health, dynamic);
    if health.config().enabled {
        let dirs = conf.health_config_dirs(health.config().stock_enabled);
        health.reload_prototypes(&dirs, Some(&link.ctx()));
        // with health off the silencers' file is not read: the state stays empty until a request changes it
        health.silencers().init();
    }
    Arc::new(Plugin {
        health,
        link,
        user_config_dir: conf.dirs.user_config.clone(),
        stock_config_dir: conf.dirs.stock_config.clone(),
    })
}

/// Health as the daemon holds it: the plugin's state, its one link to the configuration core, and the two
/// configuration directories of the start, under which the `health.d` trees are by default.
///
/// The link is made with health on or off and lives as long as this: the core's callbacks hold clones of it, a
/// reload's unregistration drops them all, and the registration that follows needs the link again.
pub struct Plugin {
    pub health: Arc<Health>,
    link: Arc<DyncfgLink>,
    user_config_dir: String,
    stock_config_dir: String,
}

impl Plugin {
    /// `health_plugin_reload()`, for `netdatacli reload-health` and SIGUSR2, on the caller's thread: the rules read
    /// again and registered again, then every alert of every host health ran for unlinked and linked again.
    ///
    /// There is no test of `[health] enabled`: with health off this is the first time the two directory keys are
    /// read, the trees loaded and the nodes registered. `[health]` itself and the silencers' file are not read again.
    ///
    /// netdata.conf's lock is held only for the two keys, as C's getter takes it per call.
    pub fn reload(&self, netdata: &Mutex<Config>) {
        let dirs = {
            let mut netdata = netdata.lock().unwrap_or_else(PoisonError::into_inner);
            let stock_enabled = self.health.config().stock_enabled;
            health_config_dirs(&mut netdata, &self.user_config_dir, &self.stock_config_dir, stock_enabled)
        };
        self.health.plugin_reload(&dirs, &self.link.ctx());
    }
}

/// What health's DynCfg nodes need of the daemon: the configuration core, the hosts, and the environment a change
/// of the rules links and unlinks alerts with.
pub struct Dynamic<'a> {
    pub dyncfg: &'a Arc<Dyncfg>,
    pub hosts: &'a Arc<Hosts>,
    pub env: &'a Arc<LiveEnv>,
}

/// Health's side of DynCfg in the daemon: its nodes live on the process's core with localhost as their host
/// (`dyncfg_add()` and its two companions of `dyncfg-inline.c`), their one callback is health's, and the Cloud's
/// copy of a rule is asked for in the metadata database.
///
/// The core's callbacks hold this; this holds the core weakly.
pub struct DyncfgLink {
    me: Weak<DyncfgLink>,
    health: Arc<Health>,
    dyncfg: Weak<Dyncfg>,
    hosts: Arc<Hosts>,
    env: Arc<LiveEnv>,
}

impl DyncfgLink {
    fn new(health: &Arc<Health>, dynamic: &Dynamic<'_>) -> Arc<DyncfgLink> {
        Arc::new_cyclic(|me| DyncfgLink {
            me: me.clone(),
            health: Arc::clone(health),
            dyncfg: Arc::downgrade(dynamic.dyncfg),
            hosts: Arc::clone(dynamic.hosts),
            env: Arc::clone(dynamic.env),
        })
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx { nodes: self, cloud: self, hosts: &*self.hosts, env: &*self.env, clock: &now_realtime_s }
    }
}

impl Nodes for DyncfgLink {
    fn add(&self, node: &NodeSpec<'_>) -> bool {
        let (Some(dyncfg), Some(link)) = (self.dyncfg.upgrade(), self.me.upgrade()) else {
            return false;
        };
        // `dyncfg_health_cb()` for every node: health reads the id, the command, the name and the payload
        let cb: InlineCallback = Arc::new(move |reply, id, cmd, name, payload, _source| {
            link.health.dyncfg_callback(&link.ctx(), reply, id, cmd, name, payload.map(|payload| payload.body.as_slice()))
        });
        dyncfg.add_inline(InlineSpec {
            host: self.hosts.localhost(),
            id: node.id,
            path: node.path,
            status: node.status,
            kind: node.kind,
            source_type: node.source_type,
            source: node.source,
            cmds: node.cmds,
            // none given: the core's defaults
            view_access: access::NONE,
            edit_access: access::NONE,
            cb,
        })
    }

    fn del(&self, id: &[u8]) {
        if let Some(dyncfg) = self.dyncfg.upgrade() {
            dyncfg.del_inline(self.hosts.localhost(), id);
        }
    }

    fn status(&self, id: &[u8], status: NodeStatus) {
        if let Some(dyncfg) = self.dyncfg.upgrade() {
            dyncfg.status_low_level(id, status);
        }
    }
}

impl Cloud for DyncfgLink {
    /// Without a database C's statement cannot be prepared: it says so and answers no.
    fn has(&self, hash: &[u8; 16]) -> bool {
        self.env
            .meta("alert_hash_has_transitioned")
            .is_some_and(|meta| meta.alert_hash_has_transitioned(hash, is_health_thread()))
    }

    /// `aclk_send_alert_configuration()`: nothing without localhost's ACLK sync configuration; else C's record in
    /// the access log. It prints localhost's node id (no text for a host never claimed) where C prints the sync
    /// configuration's own copy; the two differ only once the agent is claimed, and both the copy and the command C
    /// then queues for the ACLK thread come with the Cloud connection.
    fn send_configuration(&self, hash: &[u8; 16]) {
        let localhost = self.hosts.localhost();
        if !localhost.aclk_sync_config() {
            return;
        }
        let text = |uuid: &[u8; 16]| {
            let mut text = Vec::with_capacity(36);
            print_uuid_lower(&mut text, uuid);
            String::from_utf8_lossy(&text).into_owned()
        };
        let node_id = Some(localhost.node_id()).filter(|id| *id != [0; 16]).map(|id| text(&id)).unwrap_or_default();
        nd_log!(
            Source::Access,
            Priority::Debug,
            "ACLK REQ [{node_id} ({})]: Request to send alert config {}.",
            localhost.hostname(),
            text(hash)
        );
    }
}

/// `check_if_resumed_from_suspension()`: the wall clock moved more than twice as far as the monotonic one since the
/// last pass.
struct Suspension {
    last: Option<(u64, Instant)>,
}

impl Suspension {
    fn resumed(&mut self, realtime_ut: u64, monotonic: Instant) -> bool {
        let resumed = self.last.is_some_and(|(last_realtime_ut, last_monotonic)| {
            let realtime_delta = realtime_ut.saturating_sub(last_realtime_ut);
            let monotonic_delta = monotonic.saturating_duration_since(last_monotonic).as_micros() as u64;
            realtime_ut > last_realtime_ut
                && realtime_delta > monotonic_delta
                && realtime_delta - monotonic_delta > monotonic_delta
        });
        self.last = Some((realtime_ut, monotonic));
        resumed
    }
}

thread_local! {
    /// C's `is_health_thread`: set by the HEALTH thread when it starts.
    static IS_HEALTH_THREAD: Cell<bool> = const { Cell::new(false) };
}

/// The calling thread is HEALTH: a save the metadata queue refuses is made by it at once.
pub(crate) fn mark_health_thread() {
    IS_HEALTH_THREAD.with(|flag| flag.set(true));
}

fn is_health_thread() -> bool {
    IS_HEALTH_THREAD.with(Cell::get)
}

/// `sql_health_alarm_log_save()` of a host's entry: an insert, or an update of an entry saved before; with the
/// host's ACLK sync configuration an insert writes the alarm's row of the queue toward the Cloud too. True when a
/// row was inserted.
pub(crate) fn save_entry(meta: &MetaDb, host: &Host, entry: &Entry) -> bool {
    let Some(host_id) = crate::meta_store::host_id(host) else {
        return false;
    };
    let (hostname, queue) = (host.hostname(), host.aclk_sync_config());
    netdata_agent_health::sql::save(meta, &hostname, &host_id, entry, queue, is_health_thread())
}

/// A notification's command on the main spawn server (`POPEN_INSTANCE`). Its waits end early once HEALTH is
/// cancelled, as C's do for a cancelled thread.
struct LivePopen {
    popen: Popen,
    cancel: Arc<AtomicBool>,
}

impl Execution for LivePopen {
    fn pid(&self) -> i32 {
        self.popen.pid()
    }

    fn timedwait(self: Box<Self>, timeout_ms: i32) -> Waiting {
        let LivePopen { popen, cancel } = *self;
        let again = |popen| Box::new(LivePopen { popen, cancel: Arc::clone(&cancel) });
        match popen.timedwait(timeout_ms, &|| cancel.load(Ordering::Acquire)) {
            Waited::Exited(code) => Waiting::Exited(code),
            Waited::Running(popen, errno) => Waiting::Running(again(popen), errno),
            Waited::Error(popen) => Waiting::Error(again(popen)),
        }
    }

    fn kill(self: Box<Self>, timeout_ms: i32) -> i32 {
        let LivePopen { popen, cancel } = *self;
        popen.kill(timeout_ms, &|| cancel.load(Ordering::Acquire))
    }
}

/// What a health pass asks of the daemon: the charts as they are collected, the database for a lookup, the clock,
/// the exit flag, the alert log's tables, the metadata thread's queue, and the spawn server for a notification's
/// command.
pub struct LiveEnv {
    hosts: Arc<Hosts>,
    windows: Windows,
    /// The metadata database; none when the agent has none. Weak: the exit closes it by letting go of it, and what
    /// asks for it afterwards returns as C's statements do on its closed handle.
    meta: Option<Weak<MetaDb>>,
    queue: MetaQueue,
    /// `netdata_configured_user_config_dir`: where a notification's edit command looks for the rule's file.
    user_config_dir: Vec<u8>,
    /// HEALTH's cancel (C's `nd_thread_signaled_to_cancel()` on that thread): set by the exit's step that stops
    /// the health service. A wait for a notification's command returns at its next look, and a kill does not
    /// wait for the command's end.
    cancel: Arc<AtomicBool>,
}

impl LiveEnv {
    pub fn new(hosts: Arc<Hosts>, windows: Windows, meta: Option<&Arc<MetaDb>>, queue: MetaQueue) -> LiveEnv {
        let (meta, cancel) = (meta.map(Arc::downgrade), Arc::default());
        LiveEnv { hosts, windows, meta, queue, user_config_dir: Vec::new(), cancel }
    }

    /// The directory of the user's configuration, for the edit command of a notification.
    pub fn with_user_config_dir(mut self, dir: &str) -> LiveEnv {
        self.user_config_dir = dir.as_bytes().to_vec();
        self
    }

    /// The flag the exit raises to cancel HEALTH's waits.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// The metadata database for a statement of C's `function`: without one C's prepare fails and says so; once
    /// the exit closed it nothing is said.
    fn meta(&self, function: &str) -> Option<Arc<MetaDb>> {
        match &self.meta {
            Some(meta) => meta.upgrade(),
            None => {
                crate::meta_store::no_database(function);
                None
            }
        }
    }
}

impl Env for LiveEnv {
    fn facts(&self, chart: &Chart) -> ChartFacts {
        let collection = chart.collection();
        ChartFacts {
            obsolete: chart.flags() & flags::OBSOLETE != 0,
            last_collected_s: collection.last_collected.0,
            counter_done: collection.counter_done,
            update_every: chart.update_every(),
        }
    }

    fn retention(&self, chart: &Chart) -> (i64, i64) {
        chart.retention()
    }

    /// `rrdset2value_api_v1_with_owa()` as health calls it: a query of the health source, never interrupted.
    fn lookup(&self, host: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult {
        let storage = self.hosts.storage();
        let control = Control {
            received: Instant::now(),
            interrupted: &|_| false,
            windows: self.windows,
            pulse: Some((&storage.pulse().queries, QuerySource::Health)),
            progress: None,
        };
        chart_value(host, chart, request, &crate::data::profile_of(storage), &control, now_realtime_s())
    }

    fn now_usec(&self) -> u64 {
        now_realtime_ut()
    }

    fn transition_id(&self) -> [u8; 16] {
        *uuid::Uuid::new_v4().as_bytes()
    }

    fn exiting(&self) -> bool {
        netdata_agent_sys::exit::initiated()
    }

    fn is_health_thread(&self) -> bool {
        is_health_thread()
    }

    fn service_running(&self) -> bool {
        !shutdown::exiting()
    }

    /// `sql_health_alarm_log_load()`'s statements: the REMOVED rows for the alarms whose last saved entry is none,
    /// then the last entry of each alarm. Without a database C returns at once (silently: the agent runs without
    /// one only when it has no dbengine).
    fn load(&self, host: &Host) -> Option<Vec<LoadedRow>> {
        let meta = self.meta.as_ref()?.upgrade()?;
        let host_id = crate::meta_store::host_id(host)?;
        let (hostname, queue) = (host.hostname(), host.aclk_sync_config());
        let running = || self.service_running();
        let mut transition_id = || self.transition_id();
        let mut now_ut = now_realtime_ut;
        let health = is_health_thread();
        meta.check_removed_alerts_state(&hostname, &host_id, &running, queue, health, &mut now_ut, &mut transition_id);
        let mut rows = Vec::new();
        let prepared = meta.load_health_log(&host_id, |row| {
            rows.push(row);
            true
        });
        prepared.then_some(rows)
    }

    fn sql_alarm_id(&self, host: &Host, chart: &[u8], name: Option<&[u8]>) -> Option<(u32, u32)> {
        let meta = self.meta("sql_get_alarm_id")?;
        meta.get_alarm_id(&crate::meta_store::host_id(host)?, chart, name)
    }

    /// Without a database the queue refuses: the save is then made, and fails, at once.
    fn queue_save(&self, alerts: &Arc<HostAlerts>, unique_id: u32) -> bool {
        self.meta.is_some() && self.queue.ae_save(alerts, unique_id)
    }

    fn sql_save(&self, host: &Host, entry: &Entry) -> bool {
        self.meta("sql_health_alarm_log_insert").is_some_and(|meta| save_entry(&meta, host, entry))
    }

    fn commit_transitions(&self) {
        self.queue.store();
    }

    /// `process_alert_pending_queue()`: the host's due rows of `alert_queue` move toward the Cloud's queue (or are
    /// dropped, for a host without its ACLK sync configuration), with C's record in the access log when any was
    /// due. True when a row was queued.
    fn process_pending_queue(&self, host: &Host) -> bool {
        let (Some(meta), Some(host_id)) = (self.meta("process_alert_pending_queue"), crate::meta_store::host_id(host))
        else {
            return false;
        };
        let (queue, now) = (host.aclk_sync_config(), now_realtime_s());
        let Some((count, added)) = meta.process_alert_pending_queue(&host_id, queue, now, is_health_thread()) else {
            return false;
        };
        if count != 0 {
            let hostname = host.hostname();
            nd_log!(
                Source::Access,
                Priority::Notice,
                "ACLK STA [{hostname} (N/A)]: Processed {count} entries, queued {added}"
            );
        }
        added > 0
    }

    /// Without a database C's statement cannot be prepared, and says so; once the exit closed it nothing is said.
    fn last_executed_event(&self, host: &Host, alarm_id: u32, unique_id: u32) -> Option<i32> {
        let meta = self.meta("sql_health_get_last_executed_event")?;
        let host_id = crate::meta_store::host_id(host)?;
        meta.get_last_executed_event(&host_id, alarm_id, unique_id, is_health_thread()).flatten()
    }

    /// `spawn_popen_run()`: `/bin/sh -c <command>` on the main spawn server.
    fn exec(&self, command: &[u8]) -> Option<Box<dyn Execution>> {
        let popen = Popen::run_argv(&[b"/bin/sh".as_slice(), b"-c", command])?;
        Some(Box::new(LivePopen { popen, cancel: Arc::clone(&self.cancel) }))
    }

    fn monotonic_usec(&self) -> u64 {
        netdata_agent_sys::now_monotonic_usec()
    }

    fn edit_context(&self) -> (Vec<u8>, Vec<u8>) {
        (self.user_config_dir.clone(), self.hosts.localhost().info().registry_hostname.into_bytes())
    }
}

/// What a query sees of health (C's query target reads the host's alert dictionary and each chart's alert list):
/// the versions a data answer prints, and a chart's alerts with their published statuses.
pub struct View(pub Arc<Health>);

impl AlertView for View {
    fn versions(&self, host: &Host) -> (u64, u64) {
        self.0.host(host).map_or((0, 0), |alerts| (alerts.version(), alerts.transitions()))
    }

    fn chart_alerts(&self, host: &Host, chart: &Chart) -> Vec<ChartAlert> {
        let alerts = self.0.host(host).map(|alerts| alerts.chart_alerts(chart)).unwrap_or_default();
        alerts
            .iter()
            .map(|alert| {
                let (status, value) = {
                    let snapshot = alert.snapshot();
                    (snapshot.status, snapshot.value)
                };
                let class = match status {
                    Status::Clear => AlertClass::Clear,
                    Status::Warning => AlertClass::Warning,
                    Status::Critical => AlertClass::Critical,
                    _ => AlertClass::Other,
                };
                ChartAlert {
                    name: alert.name().to_vec(),
                    class,
                    status_name: status.name(),
                    at_least_clear: status as i32 >= Status::Clear as i32,
                    value,
                    units: alert.config.units.clone().unwrap_or_default(),
                }
            })
            .collect()
    }
}

/// What the database tells health as it lets go of a chart or a host, and as a child's receiver leaves its host:
/// C calls `rrdcalc.c` from the chart's delete callback, from the host's cleanup and from the receiver's detach.
/// An unlink is logged unless the agent is exiting.
pub fn database_event(health: &Health, env: &LiveEnv, event: HealthEvent<'_>) {
    match event {
        HealthEvent::ChartFreed(host, chart) => health.chart_freed(host, chart, env, &now_realtime_s),
        HealthEvent::HostCleanup(host) => health.host_cleanup(host, env, &now_realtime_s),
        HealthEvent::HostChartsFlushed(host) => health.host_charts_flushed(host),
        HealthEvent::HostFreed(host) => health.host_freed(host),
        HealthEvent::ChildDisconnected(host) => health.child_disconnected(host, env, &now_realtime_s),
    }
}

/// Starts `HEALTH`, which passes over `hosts`; `run_at_least_every_s` and `postpone_s` are `[health]`'s.
pub fn spawn(
    hosts: Arc<Hosts>,
    health: Arc<Health>,
    env: Arc<LiveEnv>,
    stack_size: usize,
    run_at_least_every_s: i64,
    postpone_s: i64,
) -> std::io::Result<Thread> {
    Thread::spawn(
        "HEALTH",
        stack_size,
        Duration::from_secs(1),
        Phase::OnTheTick,
        move |ticker| {
            mark_health_thread();
            let mut suspension = Suspension { last: None };
            // service_running(SERVICE_HEALTH): false once the exit starts (D110)
            let running = || ticker.running() && !shutdown::exiting();
            // C's `static int logged`: once per process
            let mut skipping_logged = false;
            while running() {
                if !stream_control::health_should_be_running() {
                    ticker.sleep(stream_control::throttle_wait());
                    continue;
                }
                let now = now_realtime_s();
                let mut next_run = now.saturating_add(run_at_least_every_s);
                let apply_hibernation_delay = suspension.resumed(now_realtime_ut(), Instant::now());
                if apply_hibernation_delay {
                    nd_log!(
                        Source::Daemon,
                        Priority::Notice,
                        "Postponing alarm checks for {postpone_s} seconds, because it seems that the system was \
                         just resumed from suspension."
                    );
                    // schedule_node_state_update(localhost, 10)
                    hosts.localhost().set_aclk_sync_config();
                }
                // C's record says so and skips nothing: the hosts' passes run, and each alert is found disabled
                if !skipping_logged && health.silencers().all_alarms_disabled() {
                    nd_log!(
                        Source::Daemon,
                        Priority::Debug,
                        "Skipping health checks, because all alarms are disabled via API command."
                    );
                    skipping_logged = true;
                }
                hosts.storage().next_health_iteration();
                for host in hosts.all() {
                    if !running() {
                        break;
                    }
                    // health_event_loop_for_host(): a host health does not run for is not stamped either
                    let pass = Pass {
                        now,
                        apply_hibernation_delay,
                        next_run: &mut next_run,
                        gate: &|| host.should_run_health(now),
                    };
                    health.host_pass(&host, pass, &*env, &now_realtime_s, &running);
                }
                if !running() {
                    break;
                }
                // the notifications the hosts' passes started are waited for before the next iteration
                health.wait_for_notifications(&*env);
                // health_sleep(): until the next run, which an alert that is due earlier brought forward
                while now_realtime_s() < next_run && !shutdown::exiting() && ticker.sleep(Duration::from_secs(1)) {}
            }
            nd_log!(Source::Daemon, Priority::Debug, "Health thread ended.");
        },
    )
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// A configuration whose user and stock configuration directories are under `root`, with `[health]` as given.
    fn conf(root: &Path, health: &str) -> Conf {
        let mut conf = Conf::default();
        conf.dirs.user_config = root.join("user").to_string_lossy().into_owned();
        conf.dirs.stock_config = root.join("stock").to_string_lossy().into_owned();
        conf.dirs.varlib = root.join("lib").to_string_lossy().into_owned();
        let path = root.join("netdata.conf");
        std::fs::write(&path, format!("[health]\n{health}")).unwrap();
        assert!(conf.netdata.load(&path, false, None).is_ok());
        // the "silencers" step of the start, which comes before health's
        conf.health_silencers_filename();
        conf
    }

    /// What health's DynCfg nodes live on in the daemon: a configuration core whose saved files are under `root`
    /// (as `conf()` lays the directories), a localhost with its `config` function, the live environment.
    struct Core {
        calls: Arc<netdata_agent_nrpc::call::Calls>,
        dyncfg: Arc<Dyncfg>,
        hosts: Arc<Hosts>,
        env: Arc<LiveEnv>,
    }

    /// A caller as the web server describes one.
    const SOURCE: &str = "method=api-bearer,role=admin,permissions=0x7ff,user=tester,ip=127.0.0.1";

    impl Core {
        fn dynamic(&self) -> Dynamic<'_> {
            Dynamic { dyncfg: &self.dyncfg, hosts: &self.hosts, env: &self.env }
        }

        fn node(&self, id: &str) -> Option<netdata_agent_dyncfg::nodes::Node> {
            self.dyncfg.nodes().lock().get(id.as_bytes()).cloned()
        }

        /// A user's `config ...` call with every permission, as `/api/v1/config` makes it: the code and the body.
        fn call(&self, cmd: &str, payload: Option<&str>) -> (u16, String) {
            use netdata_agent_nrpc::call::CallSpec;
            use netdata_agent_nrpc::reply::{ContentType, Payload, Reply};
            let localhost = self.hosts.localhost();
            let hostname = localhost.hostname();
            let called = self.calls.call(CallSpec {
                owner: Some((localhost.functions(), &hostname)),
                cmd: cmd.as_bytes(),
                source: SOURCE.as_bytes(),
                user_access: access::ALL,
                timeout_s: 10,
                wait: true,
                allow_restricted: false,
                call_id: None,
                payload: payload.map(|p| Payload { body: p.as_bytes().to_vec(), content_type: ContentType::ApplicationJson }),
                reply: Reply::new(ContentType::ApplicationJson),
                done: None,
                progress: None,
                is_cancelled: None,
                tag: None,
            });
            (called.code, called.reply.map(|r| String::from_utf8_lossy(&r.body).into_owned()).unwrap_or_default())
        }
    }

    fn core(root: &Path) -> Core {
        use netdata_agent_nrpc::call::{Calls, SystemClock};
        use netdata_agent_rrd::host::HostInfo;
        use netdata_agent_rrd::mode::DbMode;
        let calls = Calls::new(Box::new(SystemClock));
        // the core makes its `config` directory in a varlib that exists
        std::fs::create_dir_all(root.join("lib")).unwrap();
        let dyncfg = Dyncfg::new(netdata_agent_dyncfg::Init {
            varlib: &root.join("lib"),
            user_config_dir: &root.join("user"),
            stock_config_dir: &root.join("stock"),
            load_saved: true,
            calls: Arc::clone(&calls),
        });
        let info = HostInfo {
            hostname: "dc-host".into(),
            registry_hostname: "dc-host".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 60,
            health_enabled: true,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        let hosts = Arc::new(Hosts::new(Host::new("11ee0000-0000-4000-8000-0000000000dc", true, info)));
        dyncfg.set_hosts(Arc::clone(&hosts));
        dyncfg.host_init(hosts.localhost());
        let env = Arc::new(LiveEnv::new(Arc::clone(&hosts), Windows::default(), None, MetaQueue::unread().0));
        Core { calls, dyncfg, hosts, env }
    }

    /// A payload of one template rule, as the dashboard sends one.
    fn payload(on: &str, warn: &str) -> String {
        format!(
            "{{\"format_version\":1,\"rules\":[{{\"enabled\":true,\"type\":\"template\",\"config\":{{\
             \"match\":{{\"on\":\"{on}\",\"host_labels\":\"*\",\"instance_labels\":\"*\"}},\
             \"value\":{{\"database_lookup\":{{\"after\":0,\"before\":0,\"time_group\":\"average\",\
             \"time_group_condition\":\"=\",\"time_group_value\":0,\"dims_group\":\"sum\",\
             \"data_source\":\"samples\",\"options\":[],\"dimensions\":\"\"}},\
             \"calculation\":\"1\",\"update_every\":10}},\
             \"conditions\":{{\"warning_condition\":\"{warn}\"}}}}}}]}}"
        )
    }

    /// With health on the start registers health's nodes on the core: the template, then a job per alert name in
    /// the store's order, each told to enable itself at once (C's first echo), so each reads `running`. A user
    /// reaches a job's rules through the `config` function.
    #[test]
    fn health_on_registers_the_template_and_a_job_per_name() {
        use netdata_agent_dyncfg::model::{Cmds, SourceType, Status as NodeStatus, Type};
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, MetaQueue::unread().0, &core.dynamic());
        let health = Arc::clone(&plugin.health);

        let template = core.node("health:alert:prototype").expect("the template");
        assert_eq!((template.kind, template.current.status), (Type::Template, NodeStatus::Accepted));
        assert_eq!(template.cmds, Cmds::parse(b"schema add enable disable userconfig"));
        assert_eq!(template.path, b"/health/alerts/prototypes");
        assert!(template.sync);
        for name in ["user_a", "stock_b"] {
            let job = core.node(&format!("health:alert:prototype:{name}")).expect("a job");
            assert_eq!((job.kind, job.current.status), (Type::Job, NodeStatus::Running), "{name}");
            assert_eq!(job.cmds, Cmds::parse(b"schema get enable disable update userconfig"), "{name}");
            assert_eq!(job.template.as_deref(), Some(b"health:alert:prototype".as_slice()));
        }
        let user = core.node("health:alert:prototype:user_a").unwrap();
        assert_eq!(user.current.source_type, SourceType::User);
        assert!(String::from_utf8_lossy(&user.current.source).starts_with("line=1,file="));
        assert_eq!(core.node("health:alert:prototype:stock_b").unwrap().current.source_type, SourceType::Stock);
        // a stock file a user file shadows gave no rule, so it has no job
        assert!(core.node("health:alert:prototype:stock_a").is_none());

        let (code, body) = core.call("config health:alert:prototype:user_a get", None);
        assert_eq!(code, 200, "{body}");
        assert!(body.starts_with("{\"format_version\":1,\"name\":\"user_a\",\"rules\":[{\"enabled\":true"), "{body}");
        assert!(body.contains("\"source_type\":\"user\""), "{body}");
        drop(health);
    }

    /// A user's add and a user's update of a file alert are saved by the core, and the next start brings both back
    /// through the template's registration, before the file's jobs: the added name is in the store again, the
    /// updated one holds the payload's rule in place of the file's, and a job the user disabled is disabled.
    #[test]
    fn a_start_replays_what_the_user_saved() {
        use netdata_agent_dyncfg::model::{Cmds, SourceType, Status as NodeStatus};
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        {
            let mut conf = conf(root.path(), "");
            let config = conf.health_load_config_defaults();
            let core = core(root.path());
            let plugin = plugin_init(&mut conf, config, true, MetaQueue::unread().0, &core.dynamic());
            let health = Arc::clone(&plugin.health);
            let added = core.call("config health:alert:prototype add d_new", Some(&payload("d.ctx", "$this > 1")));
            assert_eq!(added, (202, "{\"status\":202,\"message\":\"accepted\"}".into()));
            let updated =
                core.call("config health:alert:prototype:user_a update", Some(&payload("u.ctx", "$this > 2")));
            assert_eq!(updated, (202, "{\"status\":202,\"message\":\"updated\"}".into()));
            let disabled = core.call("config health:alert:prototype:stock_b disable", None);
            assert_eq!(disabled, (200, "{\"status\":200,\"message\":\"disabled\"}".into()));
            assert_eq!(names(&health), ["user_a", "stock_b", "d_new"]);
            // the added job is the core's own node of the user's call, with `test` among its commands
            let job = core.node("health:alert:prototype:d_new").unwrap();
            assert!(job.cmds.contains(Cmds::TEST | Cmds::REMOVE));
            assert_eq!(job.stored.saves, 1);
        }

        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, MetaQueue::unread().0, &core.dynamic());
        let health = Arc::clone(&plugin.health);
        assert_eq!(names(&health), ["user_a", "stock_b", "d_new"]);
        {
            let prototypes = health.prototypes();
            let rule = |name: &[u8]| &prototypes.get(name).unwrap().rules()[0];
            assert_eq!(rule(b"d_new").config.source_type, SourceType::Dyncfg);
            assert_eq!(rule(b"d_new").r#match.on.as_deref(), Some(b"d.ctx".as_slice()));
            // the file's rule is replaced by the saved payload's
            assert_eq!(rule(b"user_a").config.source_type, SourceType::Dyncfg);
            assert_eq!(rule(b"user_a").r#match.on.as_deref(), Some(b"u.ctx".as_slice()));
            assert_eq!(prototypes.get(b"user_a").unwrap().rules().len(), 1);
            assert!(prototypes.get(b"user_a").unwrap().enabled());
            // the user's disable came back with the job's first echo
            assert!(!prototypes.get(b"stock_b").unwrap().enabled());
            assert_eq!(rule(b"stock_b").config.source_type, SourceType::Stock);
        }
        let status = |name: &str| core.node(&format!("health:alert:prototype:{name}")).unwrap().current.status;
        assert_eq!(status("stock_b"), NodeStatus::Disabled);
        // a replayed job is registered by health's `add`: accepted, as the running C agent shows it
        assert_eq!(status("d_new"), NodeStatus::Accepted);
        assert_eq!(status("user_a"), NodeStatus::Accepted);
        let job = core.node("health:alert:prototype:user_a").unwrap();
        assert!(job.cmds.contains(Cmds::REMOVE), "an updated file alert can be removed");

        // and a user's remove takes the name out of the store and the node out of the core
        let removed = core.call("config health:alert:prototype:d_new remove", None);
        assert_eq!(removed, (200, "{\"status\":200,\"message\":\"deleted\"}".into()));
        assert_eq!(names(&health), ["user_a", "stock_b"]);
        assert!(core.node("health:alert:prototype:d_new").is_none());
    }

    /// The Cloud's copy of a rule: the table is asked through the metadata database (without one C's statement
    /// cannot be prepared: its record, and the answer no), and the push writes C's DEBUG record to the access log
    /// only once localhost has its ACLK sync configuration, with no node id for a host never claimed.
    #[test]
    fn the_cloud_s_table_is_asked_and_the_push_is_recorded() {
        use netdata_agent_metadata::open::SqliteSettings;
        let root = tempfile::tempdir().unwrap();
        let core = core(root.path());
        let health = Health::init(Default::default(), Box::new(|_| {}));
        let hash = [0x5a; 16];

        let link = DyncfgLink::new(&health, &core.dynamic());
        let (found, records) = netdata_agent_log::capture(|| link.has(&hash));
        assert!(!found);
        let messages: Vec<_> = records.into_iter().filter_map(|record| record.message).collect();
        assert_eq!(messages, ["Failed to prepare statement, rc=21 in alert_hash_has_transitioned"]);

        let meta = Arc::new(MetaDb::open(root.path(), &SqliteSettings::default()).expect("the metadata database"));
        let env = Arc::new(LiveEnv::new(Arc::clone(&core.hosts), Windows::default(), Some(&meta), MetaQueue::unread().0));
        let link = DyncfgLink::new(&health, &Dynamic { dyncfg: &core.dyncfg, hosts: &core.hosts, env: &env });
        let (found, records) = netdata_agent_log::capture(|| link.has(&hash));
        assert!(!found && records.is_empty(), "{records:?}");
        meta.lock().execute("INSERT INTO alert_hash_cloud (hash_id) VALUES (?1)", [&hash[..]]).unwrap();
        assert!(link.has(&hash));
        assert!(!link.has(&[0x5b; 16]));

        let ((), records) = netdata_agent_log::capture(|| link.send_configuration(&hash));
        assert!(records.is_empty(), "no ACLK sync configuration: {records:?}");
        core.hosts.localhost().set_aclk_sync_config();
        let ((), records) = netdata_agent_log::capture(|| link.send_configuration(&hash));
        let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
        assert_eq!(
            records,
            [(
                Source::Access,
                Priority::Debug,
                "ACLK REQ [ (dc-host)]: Request to send alert config 5a5a5a5a-5a5a-5a5a-5a5a-5a5a5a5a5a5a.".to_owned()
            )]
        );
    }

    /// Health's nodes in the core's order, without the template's id in front: the template itself reads ``.
    fn ids(core: &Core) -> Vec<String> {
        let nodes = core.dyncfg.nodes().lock();
        let ids = nodes.keys().filter_map(|id| id.strip_prefix(b"health:alert:prototype".as_slice()));
        ids.map(|id| String::from_utf8_lossy(id).into_owned()).collect()
    }

    /// netdata.conf as the daemon holds it after its start: with the web server, under its lock.
    fn shared(conf: &mut Conf) -> Mutex<Config> {
        Mutex::new(std::mem::take(&mut conf.netdata))
    }

    /// A reload on the core the start registered on: the rules are read again, the nodes of the first registration
    /// are deleted (none was saved) and registered anew, each with its first echo, and a user still reaches a job.
    /// The plugin's own link is what the second registration is made through: every callback's clone of it went
    /// with the unregistration.
    #[test]
    fn a_reload_registers_again_on_the_real_core() {
        use netdata_agent_dyncfg::model::{SourceType, Status as NodeStatus, Type};
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, queue, &core.dynamic());
        let netdata = shared(&mut conf);
        assert_eq!(ids(&core), ["", ":user_a", ":stock_b"]);
        assert_eq!(templates(&unread.statements()), ["user_a", "stock_b"]);

        // the user's file now holds another name; its stock twin stays shadowed
        rule_file(root.path(), "user/health.d/a.conf", "user_a2");
        for round in 0..2 {
            let ((), records) = netdata_agent_log::capture(|| plugin.reload(&netdata));
            assert!(records.is_empty(), "round {round}: {records:?}");
            assert_eq!(names(&plugin.health), ["user_a2", "stock_b"], "round {round}");
            assert_eq!(ids(&core), ["", ":user_a2", ":stock_b"], "round {round}");
            let template = core.node("health:alert:prototype").expect("the template");
            assert_eq!((template.kind, template.current.status), (Type::Template, NodeStatus::Accepted));
            for (name, source_type) in [("user_a2", SourceType::User), ("stock_b", SourceType::Stock)] {
                let job = core.node(&format!("health:alert:prototype:{name}")).expect("a job");
                assert_eq!((job.kind, job.current.status), (Type::Job, NodeStatus::Running), "{name}");
                assert_eq!(job.current.source_type, source_type, "{name}");
            }
            let (code, body) = core.call("config health:alert:prototype:user_a2 get", None);
            assert_eq!(code, 200, "{body}");
            assert!(body.starts_with("{\"format_version\":1,\"name\":\"user_a2\","), "{body}");
            assert!(core.node("health:alert:prototype:user_a").is_none(), "the name the file no longer holds");
        }
        // every rule's `alert_hash` row is made again at each load
        assert_eq!(templates(&unread.statements()), ["user_a2", "stock_b", "user_a2", "stock_b"]);
        // the keys were read at the start; a reload reads them where they are
        assert_eq!(directory_keys(&mut netdata.lock().unwrap()), ["stock health config", "health config"]);
    }

    /// What the user changed in this session comes back at a reload as at a start, from the core's own nodes: a
    /// job the user saved stays in the core without its method when health unregisters, the template (never saved)
    /// is deleted and so comes back last in the core's order, its registration replays `add` for every job whose
    /// rules are DynCfg's, in the core's order, and a job the user disabled is told to disable itself again.
    #[test]
    fn a_reload_replays_what_the_user_saved_in_this_session() {
        use netdata_agent_dyncfg::model::{Cmds, SourceType, Status as NodeStatus};
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, MetaQueue::unread().0, &core.dynamic());
        let health = Arc::clone(&plugin.health);
        let netdata = shared(&mut conf);

        let added = core.call("config health:alert:prototype add d_new", Some(&payload("d.ctx", "$this > 1")));
        assert_eq!(added.0, 202, "{added:?}");
        let updated = core.call("config health:alert:prototype:user_a update", Some(&payload("u.ctx", "$this > 2")));
        assert_eq!(updated.0, 202, "{updated:?}");
        assert_eq!(core.call("config health:alert:prototype:stock_b disable", None).0, 200);
        // health tells the core the job's new status itself (C's `dyncfg_status()` in its callback)
        let status = |name: &str| core.node(&format!("health:alert:prototype:{name}")).unwrap().current.status;
        assert_eq!(status("stock_b"), NodeStatus::Disabled);
        let gone = core.call("config health:alert:prototype add d_gone", Some(&payload("g.ctx", "$this > 3")));
        assert_eq!(gone.0, 202, "{gone:?}");
        assert_eq!(core.call("config health:alert:prototype:d_gone remove", None).0, 200);
        assert_eq!(names(&health), ["user_a", "stock_b", "d_new"]);
        assert_eq!(ids(&core), ["", ":user_a", ":stock_b", ":d_new"]);

        let state = |core: &Core| {
            let job = |name: &str| core.node(&format!("health:alert:prototype:{name}")).unwrap();
            let of = |name: &str| {
                let job = job(name);
                (job.current.status, job.current.source_type, job.cmds.contains(Cmds::REMOVE), job.stored.saves)
            };
            [of("user_a"), of("stock_b"), of("d_new")]
        };
        let before = state(&core);

        for round in 0..2 {
            plugin.reload(&netdata);
            // the file's `user_a` is replaced in its place by the saved payload's; the added name follows the files'
            assert_eq!(names(&health), ["user_a", "stock_b", "d_new"], "round {round}");
            // the three saved jobs stayed; the template was deleted and inserted anew
            assert_eq!(ids(&core), [":user_a", ":stock_b", ":d_new", ""], "round {round}");
            {
                let prototypes = health.prototypes();
                let rule = |name: &[u8]| &prototypes.get(name).unwrap().rules()[0];
                assert_eq!(rule(b"user_a").config.source_type, SourceType::Dyncfg);
                assert_eq!(rule(b"user_a").r#match.on.as_deref(), Some(b"u.ctx".as_slice()));
                assert_eq!(prototypes.get(b"user_a").unwrap().rules().len(), 1);
                assert_eq!(rule(b"d_new").r#match.on.as_deref(), Some(b"d.ctx".as_slice()));
                assert_eq!(rule(b"stock_b").config.source_type, SourceType::Stock);
                assert!(prototypes.get(b"user_a").unwrap().enabled() && prototypes.get(b"d_new").unwrap().enabled());
                assert!(!prototypes.get(b"stock_b").unwrap().enabled(), "the user's disable came back");
                assert!(prototypes.get(b"d_gone").is_none());
            }
            assert_eq!(
                state(&core),
                [
                    (NodeStatus::Accepted, SourceType::Dyncfg, true, before[0].3),
                    (NodeStatus::Disabled, SourceType::Stock, false, before[1].3),
                    (NodeStatus::Accepted, SourceType::Dyncfg, true, before[2].3),
                ],
                "round {round}"
            );
            assert!(core.node("health:alert:prototype:d_gone").is_none());
        }
        // nothing was saved by a reload, and the user still reaches what came back
        assert!(before.iter().all(|job| job.3 >= 1), "{before:?}");
        let (code, body) = core.call("config health:alert:prototype:d_new get", None);
        assert_eq!(code, 200, "{body}");
        let enabled = core.call("config health:alert:prototype:stock_b enable", None);
        assert_eq!(enabled, (202, "{\"status\":202,\"message\":\"enabled\"}".into()));
        assert!(health.prototypes().get(b"stock_b").unwrap().enabled());
        assert_eq!(status("stock_b"), NodeStatus::Accepted);
    }

    /// With health off the start loads and registers nothing, and a reload does it all, because C's reload has no
    /// test of `[health] enabled`: the two directory keys are read for the first time, both trees are loaded, every
    /// rule gets its row, and the template and the jobs are on the core. The silencers' file is still not read.
    #[test]
    fn a_reload_with_health_off_registers_and_reads_the_keys() {
        use netdata_agent_dyncfg::model::Status as NodeStatus;
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "enabled = no\n");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, queue, &core.dynamic());
        let netdata = shared(&mut conf);
        assert!(ids(&core).is_empty() && names(&plugin.health).is_empty());
        assert!(directory_keys(&mut netdata.lock().unwrap()).is_empty());

        let ((), records) = netdata_agent_log::capture(|| plugin.reload(&netdata));
        assert!(records.is_empty(), "{records:?}");
        assert_eq!(names(&plugin.health), ["user_a", "stock_b"]);
        assert_eq!(templates(&unread.statements()), ["user_a", "stock_b"]);
        assert_eq!(ids(&core), ["", ":user_a", ":stock_b"]);
        for name in ["user_a", "stock_b"] {
            let job = core.node(&format!("health:alert:prototype:{name}")).unwrap();
            assert_eq!(job.current.status, NodeStatus::Running, "{name}");
        }
        assert_eq!(directory_keys(&mut netdata.lock().unwrap()), ["stock health config", "health config"]);
        assert!(!plugin.health.config().enabled);
    }

    /// Each load reads the two `[directories]` keys again, as C's getters do, so a reload loads the trees the
    /// configuration names then (for an agent with health off that is the keys' first read). Here a key is changed
    /// between two loads to show the read. A tree that is not there leaves C's record and no rule.
    #[test]
    fn a_reload_reads_the_directories_again() {
        use netdata_agent_inicfg::SECTION_DIRECTORIES;
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, MetaQueue::unread().0, &core.dynamic());
        let netdata = shared(&mut conf);
        assert_eq!(names(&plugin.health), ["user_a", "stock_b"]);

        rule_file(root.path(), "other/a.conf", "other_a");
        rule_file(root.path(), "shipped/z.conf", "shipped_z");
        let path = |dir: &str| root.path().join(dir).to_string_lossy().into_owned();
        netdata.lock().unwrap().set(SECTION_DIRECTORIES, "health config", &path("other"));
        netdata.lock().unwrap().set(SECTION_DIRECTORIES, "stock health config", &path("shipped"));
        plugin.reload(&netdata);
        assert_eq!(names(&plugin.health), ["other_a", "shipped_z"]);
        assert_eq!(ids(&core), ["", ":other_a", ":shipped_z"]);

        netdata.lock().unwrap().set(SECTION_DIRECTORIES, "health config", &path("missing"));
        let ((), records) = netdata_agent_log::capture(|| plugin.reload(&netdata));
        let messages: Vec<_> = records.into_iter().filter_map(|record| record.message).collect();
        assert_eq!(messages, [format!("CONFIG cannot open user-config directory '{}'.", path("missing"))]);
        assert_eq!(names(&plugin.health), ["shipped_z"]);
    }

    /// The record of health's start when there is no silencers file, which no test here lays.
    fn no_silencers(root: &Path) -> String {
        let file = root.join("lib").join("health.silencers.json");
        format!("Cannot open the file {}, so Netdata will work with the default health configuration.", file.display())
    }

    fn rule_file(root: &Path, path: &str, name: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("template: {name}\non: system.cpu\nevery: 10s\ncalc: 1\n")).unwrap();
    }

    /// Three rules: one in the user tree, its stock twin (shadowed), one in the stock tree only.
    fn trees(root: &Path) {
        rule_file(root, "user/health.d/a.conf", "user_a");
        rule_file(root, "stock/health.d/a.conf", "stock_a");
        rule_file(root, "stock/health.d/b.conf", "stock_b");
    }

    /// The names of the health keys under `[directories]`, in the order they were first read.
    fn directory_keys(netdata: &mut Config) -> Vec<String> {
        let dump = String::from_utf8(netdata.generate(false, true)).unwrap();
        let lines = dump.lines().filter(|line| line.contains("health config = "));
        lines.map(|line| line.trim_start_matches(['#', ' ', '\t']).split(" = ").next().unwrap().to_owned()).collect()
    }

    fn names(health: &Health) -> Vec<String> {
        health.prototypes().iter().map(|(name, _)| String::from_utf8_lossy(name).into_owned()).collect()
    }

    fn templates(rows: &[netdata_agent_metadata::health::AlertHashRow]) -> Vec<String> {
        rows.iter().map(|row| String::from_utf8_lossy(row.template.as_deref().unwrap_or(b"")).into_owned()).collect()
    }

    #[test]
    fn health_on_loads_both_trees_and_queues_a_row_per_rule() {
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let core = core(root.path());
        let (plugin, records) =
            netdata_agent_log::capture(|| plugin_init(&mut conf, config, true, queue, &core.dynamic()));
        let health = Arc::clone(&plugin.health);
        // with health on the silencers' file is read after the rules: there is none, and the record carries the
        // failed open's errno
        let records: Vec<_> = records.into_iter().map(|record| (record.errno, record.message)).collect();
        assert_eq!(records, [(2, Some(no_silencers(root.path())))]);
        // the user tree first, then the stock files nothing shadows
        assert_eq!(names(&health), ["user_a", "stock_b"]);
        assert_eq!(templates(&unread.statements()), ["user_a", "stock_b"]);
        // C reads the stock key first
        assert_eq!(directory_keys(&mut conf.netdata), ["stock health config", "health config"]);
        // the defaults are filled after the row was made
        let prototypes = health.prototypes();
        let rule = &prototypes.get(b"user_a").unwrap().rules()[0];
        assert!(rule.config.exec.as_deref().is_some_and(|exec| exec.ends_with(b"/alarm-notify.sh")));
        assert_eq!(rule.config.recipient.as_deref(), Some(&b"root"[..]));
    }

    #[test]
    fn without_the_stock_rules_only_the_user_key_is_read() {
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "enable stock health configuration = no\n");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let core = core(root.path());
        let plugin = plugin_init(&mut conf, config, true, queue, &core.dynamic());
        let health = Arc::clone(&plugin.health);
        assert_eq!(names(&health), ["user_a"]);
        assert_eq!(templates(&unread.statements()), ["user_a"]);
        assert_eq!(directory_keys(&mut conf.netdata), ["health config"]);
        // nor does a reload read the stock key
        let netdata = shared(&mut conf);
        plugin.reload(&netdata);
        assert_eq!(names(&health), ["user_a"]);
        assert_eq!(directory_keys(&mut netdata.lock().unwrap()), ["health config"]);
    }

    /// The alert log's part of the live environment when the agent has no metadata database, and when the exit
    /// closed it. Without one: the load says nothing; the queue refuses every save, so none is ever pending; a save
    /// the queue refused is made only on the HEALTH thread; a save, an alarm's id and the pending queue each leave
    /// C's record of a statement that cannot be prepared; no entry is marked as saved. With a closed one nothing is
    /// said.
    #[test]
    fn the_live_env_without_a_database_and_after_its_close() {
        use netdata_agent_health::entry::entry_flags;
        use netdata_agent_health::readfile::health_readfile;
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        use netdata_agent_rrd::host::{HostInfo, pending_flags};
        use netdata_agent_rrd::mode::DbMode;

        let dir = tempfile::tempdir().unwrap();
        let rules = dir.path().join("t.conf");
        std::fs::write(&rules, "template: t_one\n on: t.ctx\n calc: 1\n every: 1s\n warn: $this > 5\n").unwrap();
        let health = Health::init(Default::default(), Box::new(|_| {}));
        assert!(health_readfile(&health, rules.as_os_str().as_encoded_bytes(), false));
        let info = HostInfo {
            hostname: "live".into(),
            registry_hostname: "live".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 3600,
            health_enabled: true,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        let hosts = Arc::new(Hosts::new(Host::new("11ee0000-0000-4000-8000-0000000000ab", true, info)));
        let host = Arc::clone(hosts.localhost());
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

        // a host's first pass through `env`, as a new process makes it: what it recorded
        let first_pass = |env: &LiveEnv| -> Vec<String> {
            health.host_freed(&host);
            host.raise_pending_flags(pending_flags::HEALTH_INITIALIZATION);
            chart.flags_set_and_clear(flags::PENDING_HEALTH_INITIALIZATION, 0);
            let mut next_run = now + 10;
            let ((), records) = netdata_agent_log::capture(|| {
                let pass = Pass { now, apply_hibernation_delay: false, next_run: &mut next_run, gate: &|| true };
                health.host_pass(&host, pass, env, &now_realtime_s, &|| true);
            });
            records.into_iter().filter_map(|record| record.message).collect()
        };
        let failed = |records: &[String], function: &str| {
            let record = format!("Failed to prepare statement, rc=21 in {function}");
            records.iter().filter(|message| **message == record).count()
        };

        // no database, on a thread that is not HEALTH: the link's save is not made; what the pass logs itself is
        let (queue, unread) = MetaQueue::unread();
        let env = LiveEnv::new(Arc::clone(&hosts), Windows::default(), None, queue);
        let records = first_pass(&env);
        let elsewhere = failed(&records, "sql_health_alarm_log_insert");
        assert!(elsewhere > 0, "{records:?}");
        assert_eq!(failed(&records, "sql_get_alarm_id"), 1, "{records:?}");
        assert_eq!(failed(&records, "process_alert_pending_queue"), 1, "{records:?}");
        // the alert's first CLEAR: the table is asked for the alarm's last executed event
        assert_eq!(failed(&records, "sql_health_get_last_executed_event"), 1, "{records:?}");
        assert!(!records.iter().any(|message| message.contains("Database has not been initialized")), "{records:?}");
        let alerts = health.host(&host).unwrap();
        assert_eq!((alerts.pending_transitions(), unread.alert_commands().0.len()), (0, 0), "the queue refuses");
        assert!(alerts.log_entries().iter().all(|entry| entry.flags & entry_flags::SAVED == 0));

        // the same on HEALTH: the link's refused save is made at once too
        mark_health_thread();
        let records = first_pass(&env);
        assert_eq!(failed(&records, "sql_health_alarm_log_insert"), elsewhere + 1, "{records:?}");

        // a database the exit closed: nothing is said
        let meta = Arc::new(MetaDb::open(dir.path(), &Default::default()).unwrap());
        let (queue, _unread) = MetaQueue::unread();
        let env = LiveEnv::new(Arc::clone(&hosts), Windows::default(), Some(&meta), queue);
        drop(meta);
        let records = first_pass(&env);
        assert!(!records.iter().any(|message| message.starts_with("Failed to prepare statement")), "{records:?}");
        assert!(health.host(&host).unwrap().log_entries().iter().all(|entry| entry.flags & entry_flags::SAVED == 0));
    }

    /// What a notification's edit command is made of: the user configuration directory the daemon was given and
    /// localhost's registry hostname. The flag that cancels HEALTH's waits is one, shared with the exit.
    #[test]
    fn the_live_env_gives_the_edit_command_its_two_texts() {
        use netdata_agent_rrd::host::HostInfo;
        use netdata_agent_rrd::mode::DbMode;
        let info = HostInfo {
            hostname: "live".into(),
            registry_hostname: "live-registry".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 3600,
            health_enabled: true,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        let hosts = Arc::new(Hosts::new(Host::new("11ee0000-0000-4000-8000-0000000000ac", true, info)));
        let env = LiveEnv::new(hosts, Windows::default(), None, MetaQueue::unread().0);
        assert_eq!(env.edit_context(), (Vec::new(), b"live-registry".to_vec()));
        let env = env.with_user_config_dir("/etc/netdata");
        assert_eq!(env.edit_context(), (b"/etc/netdata".to_vec(), b"live-registry".to_vec()));
        let cancel = env.cancel_flag();
        assert!(!cancel.load(Ordering::Acquire) && Arc::ptr_eq(&cancel, &env.cancel));
    }

    /// What the loop reads of a chart through the daemon: its collection as it stands, the span of its stored
    /// data over its dimensions, and a lookup's value, read from the database at the wall clock and counted as a
    /// query of health.
    #[test]
    fn the_live_env_reads_the_chart_and_its_database() {
        use netdata_agent_query::tables::{TimeGrouping, options};
        use netdata_agent_query::value::Priority as QueryPriority;
        use netdata_agent_rrd::chart::{Algorithm, ChartSpec, ChartType};
        use netdata_agent_rrd::host::HostInfo;
        use netdata_agent_rrd::mode::DbMode;
        use netdata_agent_storage::storage_number::SN_FLAG_NOT_ANOMALOUS;

        let info = HostInfo {
            hostname: "live".into(),
            registry_hostname: "live".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: DbMode::Ram,
            history_entries: 3600,
            health_enabled: true,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        };
        let hosts = Arc::new(Hosts::new(Host::new("11ee0000-0000-4000-8000-0000000000aa", true, info)));
        let host = Arc::clone(hosts.localhost());
        let spec = |id: &'static str, update_every: i32| ChartSpec {
            type_: "t",
            id,
            name: None,
            family: Some("f"),
            context: Some("t.ctx"),
            title: "T",
            units: "u",
            plugin: "p",
            module: None,
            priority: 1000,
            update_every,
            chart_type: ChartType::Line,
            mode: DbMode::Ram,
            history_entries: 3600,
            page_size: 4096,
        };
        let env = LiveEnv::new(Arc::clone(&hosts), Windows::default(), None, MetaQueue::unread().0);

        // a new chart, collected every 3 seconds on a host of 1: never collected, no data
        let (slow, _) = host.charts().create(&spec("slow", 3));
        let never = ChartFacts { obsolete: false, last_collected_s: 0, counter_done: 0, update_every: 3 };
        assert_eq!((env.facts(&slow), env.retention(&slow)), (never, (0, 0)));
        let (chart, _) = host.charts().create(&spec("c", 1));

        // two dimensions: one with data at three seconds that end 3 seconds ago, one without any
        let now = now_realtime_s();
        let (stored, _) = chart.dim_add("stored", None, 1, 1, Algorithm::Absolute);
        chart.dim_add("empty", None, 1, 1, Algorithm::Absolute);
        for (second, value) in [(now - 5, 10.0), (now - 4, 40.0), (now - 3, 20.0)] {
            stored.store_metric(second as u64 * 1_000_000, value, SN_FLAG_NOT_ANOMALOUS);
        }
        chart.update_collection(|collection| {
            collection.counter_done = 3;
            collection.last_collected = (now - 3, 250_000);
            collection.last_updated = (now - 2, 0);
        });
        host.contexts().process_queued();
        let collected = ChartFacts { obsolete: false, last_collected_s: now - 3, counter_done: 3, update_every: 1 };
        assert_eq!(env.facts(&chart), collected);
        let (first, last) = env.retention(&chart);
        assert!(first != 0 && first <= now - 5 && last == now - 3, "{first} {last} at {now}");

        chart.update_meta(|meta| meta.flags |= flags::OBSOLETE);
        assert_eq!(env.facts(&chart), ChartFacts { obsolete: true, ..collected });
        chart.update_meta(|meta| meta.flags &= !flags::OBSOLETE);

        // the highest value of the last 20 seconds, whatever second it is by now
        let request = ValueRequest {
            dimensions: None,
            points: 1,
            after: -20,
            before: 0,
            time_group: TimeGrouping::Max,
            time_group_options: None,
            resampling_time: 0,
            options: options::SELECTED_TIER | options::NOT_ALIGNED,
            timeout_ms: 0,
            tier: 0,
            priority: QueryPriority::Synchronous,
        };
        let health_queries = || hosts.storage().pulse().queries.source(QuerySource::Health).queries;
        assert_eq!(health_queries(), 0);
        let result = env.lookup(&host, &chart, &request);
        assert_eq!((result.code, result.value, result.value_is_null), (200, 40.0, false));
        // the window is the wall clock's, not a pass's second
        let (after, before) = result.window.expect("the window");
        assert!(after < before && (now - 5..=now_realtime_s()).contains(&before), "{after} {before} at {now}");
        assert_eq!(health_queries(), 1);
        // of the dimension without data alone: no value
        let empty = ValueRequest { dimensions: Some(b"empty".to_vec()), ..request };
        let result = env.lookup(&host, &chart, &empty);
        assert!(result.value.is_nan() && result.code == 200, "{result:?}");

        // each entry gets its own transition id
        assert_ne!(env.transition_id(), [0; 16]);
        assert_ne!(env.transition_id(), env.transition_id());
    }

    /// With health off the store exists and nothing else happens.
    #[test]
    fn health_off_reads_no_key_no_file_and_queues_nothing() {
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "enabled = no\n");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let core = core(root.path());
        let (plugin, records) =
            netdata_agent_log::capture(|| plugin_init(&mut conf, config, true, queue, &core.dynamic()));
        let health = Arc::clone(&plugin.health);
        assert!(records.is_empty(), "{records:?}");
        // with health off no node of health's is registered
        assert!(core.node("health:alert:prototype").is_none());
        assert!(names(&health).is_empty());
        assert!(unread.statements().is_empty());
        assert!(directory_keys(&mut conf.netdata).is_empty());
    }

    /// Without `netdata-meta.db` C's statement cannot be prepared: one record per rule, and no row.
    #[test]
    fn without_a_database_every_rule_logs_the_failed_prepare() {
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let core = core(root.path());
        let (plugin, records) =
            netdata_agent_log::capture(|| plugin_init(&mut conf, config, false, queue, &core.dynamic()));
        let health = Arc::clone(&plugin.health);
        assert_eq!(names(&health), ["user_a", "stock_b"]);
        assert!(unread.statements().is_empty());
        let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
        let failed = "Failed to prepare statement, rc=21 in sql_alert_store_config".to_owned();
        assert_eq!(messages, [failed.clone(), failed, no_silencers(root.path())]);
    }

    /// A pass after the wall clock ran more than twice as far as the monotonic one is a resume; the first is not.
    #[test]
    fn a_resume_is_twice_the_monotonic_time() {
        let mut s = Suspension { last: None };
        let t0 = Instant::now();
        assert!(!s.resumed(1_000_000_000, t0));
        let t1 = t0 + Duration::from_secs(10);
        assert!(!s.resumed(1_000_000_000 + 10_000_000, t1), "as far");
        let t2 = t1 + Duration::from_secs(10);
        assert!(!s.resumed(1_000_000_000 + 30_000_000, t2), "twice as far is not more than twice");
        let t3 = t2 + Duration::from_secs(10);
        assert!(s.resumed(1_000_000_000 + 30_000_000 + 20_000_001, t3), "more than twice");
    }
}
