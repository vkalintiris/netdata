//! Health in the daemon: the plugin's start (`health_plugin_init()`, `src/health/health.c`), which loads the alert
//! configuration, and the `HEALTH` thread, ported from `health_main()` and `health_event_loop()`
//! (`src/health/health_event_loop.c`). A pass visits every host: those health runs for get their charts' alerts
//! linked and evaluated. C runs the loop with health off too, and a disconnected child is archived only after
//! more than 10 of its passes (D93.2). It keeps C's pacing: a pass at most every `[health] run at least every`
//! seconds or when the next alert is due, waited in 1 s sleeps, and none while a backfill or more than one user
//! query runs.

use std::cell::Cell;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use netdata_agent_health::alert::Status;
use netdata_agent_health::alerts::HostAlerts;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::entry::Entry;
use netdata_agent_health::pass::{ChartFacts, Env, Pass};
use netdata_agent_health::store::alert_hash_row;
use netdata_agent_health::{Health, StoreSink};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_metadata::health_log::LoadedRow;
use netdata_agent_metadata::open::MetaDb;
use netdata_agent_query::execute::Control;
use netdata_agent_query::grouping::Windows;
use netdata_agent_query::value::{ValueRequest, ValueResult, chart_value};
use netdata_agent_rrd::chart::{Chart, flags};
use netdata_agent_rrd::clock::{now_realtime_s, now_realtime_ut};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_rrd::pulse::QuerySource;
use netdata_agent_rrd::storage::{AlertClass, AlertView, ChartAlert, HealthEvent};
use netdata_agent_rrd::stream_control;

use crate::conf::Conf;
use crate::heartbeat::{Phase, Thread};
use crate::metasync::MetaQueue;
use crate::shutdown;

/// `health_plugin_init()`, in `rrd_init()` once localhost has its `config` function: the empty prototype store, and
/// with health on the two `health.d` trees read into it. With health off nothing else happens: no directory key is
/// read, no file opened, no record written, no row queued.
///
/// Each rule C accepts gets its `alert_hash` row: bound on this thread, stepped by METASYNC's next store job.
/// Without a database C's statement cannot be prepared, and says so once per rule.
pub fn plugin_init(conf: &mut Conf, config: HealthConfig, database: bool, queue: MetaQueue) -> Arc<Health> {
    let store: StoreSink = if database {
        Box::new(move |rule| queue.execute_store_statement(alert_hash_row(rule)))
    } else {
        Box::new(|_| crate::meta_store::no_database("sql_alert_store_config"))
    };
    let health = Health::init(config, store);
    if health.config().enabled {
        let dirs = conf.health_config_dirs(health.config().stock_enabled);
        health.reload_prototypes(&dirs);
    }
    health
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

/// `sql_health_alarm_log_save()` of a host's entry: an insert, or an update of an entry saved before; with the
/// host's ACLK sync configuration an insert writes the alarm's row of the queue toward the Cloud too. True when a
/// row was inserted.
pub(crate) fn save_entry(meta: &MetaDb, host: &Host, entry: &Entry) -> bool {
    let Some(host_id) = crate::meta_store::host_id(host) else {
        return false;
    };
    netdata_agent_health::sql::save(meta, &host.hostname(), &host_id, entry, host.aclk_sync_config())
}

/// What a health pass asks of the daemon: the charts as they are collected, the database for a lookup, the clock,
/// the exit flag, the alert log's tables and the metadata thread's queue. Notifying about an entry does nothing
/// yet: it comes with the notifications.
pub struct LiveEnv {
    hosts: Arc<Hosts>,
    windows: Windows,
    /// The metadata database; none when the agent has none. Weak: the exit closes it by letting go of it, and what
    /// asks for it afterwards returns as C's statements do on its closed handle.
    meta: Option<Weak<MetaDb>>,
    queue: MetaQueue,
}

impl LiveEnv {
    pub fn new(hosts: Arc<Hosts>, windows: Windows, meta: Option<&Arc<MetaDb>>, queue: MetaQueue) -> LiveEnv {
        LiveEnv { hosts, windows, meta: meta.map(Arc::downgrade), queue }
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
        IS_HEALTH_THREAD.with(Cell::get)
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
        let (now, now_ut) = (now_realtime_s(), now_realtime_ut());
        meta.check_removed_alerts_state(&hostname, &host_id, &running, queue, now, now_ut, &mut transition_id);
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
        let Some((count, added)) = meta.process_alert_pending_queue(&host_id, host.aclk_sync_config(), now_realtime_s())
        else {
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

    fn notify(&self, _: &mut Entry) {}
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

/// What the database tells health as it lets go of a chart or a host: C calls `rrdcalc.c` from the chart's delete
/// callback and from the host's cleanup. An unlink is logged unless the agent is exiting.
pub fn database_event(health: &Health, env: &LiveEnv, event: HealthEvent<'_>) {
    match event {
        HealthEvent::ChartFreed(host, chart) => health.chart_freed(host, chart, env, &now_realtime_s),
        HealthEvent::HostCleanup(host) => health.host_cleanup(host, env, &now_realtime_s),
        HealthEvent::HostChartsFlushed(host) => health.host_charts_flushed(host),
        HealthEvent::HostFreed(host) => health.host_freed(host),
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
        let path = root.join("netdata.conf");
        std::fs::write(&path, format!("[health]\n{health}")).unwrap();
        assert!(conf.netdata.load(&path, false, None).is_ok());
        conf
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
    fn directory_keys(conf: &mut Conf) -> Vec<String> {
        let dump = String::from_utf8(conf.netdata.generate(false, true)).unwrap();
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
        let (health, records) = netdata_agent_log::capture(|| plugin_init(&mut conf, config, true, queue));
        assert!(records.is_empty(), "{records:?}");
        // the user tree first, then the stock files nothing shadows
        assert_eq!(names(&health), ["user_a", "stock_b"]);
        assert_eq!(templates(&unread.statements()), ["user_a", "stock_b"]);
        // C reads the stock key first
        assert_eq!(directory_keys(&mut conf), ["stock health config", "health config"]);
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
        let health = plugin_init(&mut conf, config, true, queue);
        assert_eq!(names(&health), ["user_a"]);
        assert_eq!(templates(&unread.statements()), ["user_a"]);
        assert_eq!(directory_keys(&mut conf), ["health config"]);
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
        let (health, records) = netdata_agent_log::capture(|| plugin_init(&mut conf, config, true, queue));
        assert!(records.is_empty(), "{records:?}");
        assert!(names(&health).is_empty());
        assert!(unread.statements().is_empty());
        assert!(directory_keys(&mut conf).is_empty());
    }

    /// Without `netdata-meta.db` C's statement cannot be prepared: one record per rule, and no row.
    #[test]
    fn without_a_database_every_rule_logs_the_failed_prepare() {
        let root = tempfile::tempdir().unwrap();
        trees(root.path());
        let mut conf = conf(root.path(), "");
        let config = conf.health_load_config_defaults();
        let (queue, unread) = MetaQueue::unread();
        let (health, records) = netdata_agent_log::capture(|| plugin_init(&mut conf, config, false, queue));
        assert_eq!(names(&health), ["user_a", "stock_b"]);
        assert!(unread.statements().is_empty());
        let messages: Vec<String> = records.into_iter().filter_map(|record| record.message).collect();
        assert_eq!(messages, ["Failed to prepare statement, rc=21 in sql_alert_store_config"; 2]);
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
