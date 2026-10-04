//! Health in the daemon: the plugin's start (`health_plugin_init()`, `src/health/health.c`), which loads the alert
//! configuration, and the `HEALTH` thread, ported from `health_main()` and `health_event_loop()`
//! (`src/health/health_event_loop.c`) without alerts yet (M9): C runs the loop with health off too, and a
//! disconnected child is archived only after more than 10 of its passes (D93.2). It keeps C's pacing: a pass at
//! most every `[health] run at least every` seconds, waited in 1 s sleeps, and none while a backfill or more than
//! one user query runs.

use std::sync::Arc;
use std::time::{Duration, Instant};

use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::store::alert_hash_row;
use netdata_agent_health::{Health, StoreSink};
use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_rrd::clock::{now_realtime_s, now_realtime_ut};
use netdata_agent_rrd::storage::StorageLayout;
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

/// Starts `HEALTH`, whose passes `storage` counts; `run_at_least_every_s` and `postpone_s` are `[health]`'s.
pub fn spawn(
    storage: Arc<StorageLayout>,
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
            while ticker.running() && !shutdown::exiting() {
                if !stream_control::health_should_be_running() {
                    ticker.sleep(stream_control::throttle_wait());
                    continue;
                }
                let next_run = now_realtime_s().saturating_add(run_at_least_every_s);
                if suspension.resumed(now_realtime_ut(), Instant::now()) {
                    nd_log!(
                        Source::Daemon,
                        Priority::Notice,
                        "Postponing alarm checks for {postpone_s} seconds, because it seems that the system was just resumed from suspension."
                    );
                }
                // health_event_loop_for_host(): with health off no host runs its alerts
                storage.next_health_iteration();
                // health_sleep()
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
