//! Health in the daemon: the plugin's start (`health_plugin_init()`, `src/health/health.c`), which loads the alert
//! configuration, and the `HEALTH` thread, ported from `health_main()` and `health_event_loop()`
//! (`src/health/health_event_loop.c`). A pass visits every host: those health runs for get their charts' alerts
//! linked and evaluated. C runs the loop with health off too, and a disconnected child is archived only after
//! more than 10 of its passes (D93.2). It keeps C's pacing: a pass at most every `[health] run at least every`
//! seconds or when the next alert is due, waited in 1 s sleeps, and none while a backfill or more than one user
//! query runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::entry::Entry;
use netdata_agent_health::pass::{ChartFacts, Env, Pass};
use netdata_agent_health::store::alert_hash_row;
use netdata_agent_health::{Health, StoreSink};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_query::execute::Control;
use netdata_agent_query::grouping::Windows;
use netdata_agent_query::value::{ValueRequest, ValueResult, chart_value};
use netdata_agent_rrd::chart::{Chart, flags};
use netdata_agent_rrd::clock::{now_realtime_s, now_realtime_ut};
use netdata_agent_rrd::host::{Host, Hosts};
use netdata_agent_rrd::pulse::QuerySource;
use netdata_agent_rrd::storage::HealthEvent;
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
    let health = Health::init(config, store, database);
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

/// What a health pass asks of the daemon: the charts as they are collected, the database for a lookup, the clock,
/// the exit flag. Saving an entry and notifying about one do nothing yet: they come with the alert log's tables
/// and with the notifications.
pub struct LiveEnv {
    hosts: Arc<Hosts>,
    windows: Windows,
}

impl LiveEnv {
    pub fn new(hosts: Arc<Hosts>, windows: Windows) -> LiveEnv {
        LiveEnv { hosts, windows }
    }
}

impl Env for LiveEnv {
    fn facts(&self, chart: &Chart) -> ChartFacts {
        let collection = chart.collection();
        let (first_entry_s, last_entry_s) = chart.retention();
        ChartFacts {
            obsolete: chart.flags() & flags::OBSOLETE != 0,
            last_collected_s: collection.last_collected.0,
            counter_done: collection.counter_done,
            update_every: chart.update_every(),
            first_entry_s,
            last_entry_s,
        }
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

    fn save(&self, _: &mut Entry, _: bool) {}

    fn notify(&self, _: &mut Entry) {}
}

/// What the database tells health as it lets go of a chart or a host: C calls `rrdcalc.c` from the chart's delete
/// callback and from the host's cleanup. An unlink is logged unless the agent is exiting.
pub fn database_event(health: &Health, env: &LiveEnv, event: HealthEvent<'_>) {
    match event {
        HealthEvent::ChartFreed(host, chart) => health.chart_freed(host, chart, env, &now_realtime_s),
        HealthEvent::HostCleanup(host) => health.host_cleanup(host, env, &now_realtime_s),
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
                        "Postponing alarm checks for {postpone_s} seconds, because it seems that the system was just resumed from suspension."
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
