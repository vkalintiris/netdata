//! What the database's events do to a host's alerts, through the hook the daemon installs: a freed chart, a host
//! object that only shares its machine GUID, and all of it at once from four threads.

use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;

use netdata_agent_health::Health;
use netdata_agent_health::pass::Idle;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::readfile::health_readfile;
use netdata_agent_rrd::chart::{Chart, ChartSpec, ChartType};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;

const NOW: i64 = 1_700_000_000;

fn info(hostname: &str) -> HostInfo {
    HostInfo {
        hostname: hostname.into(),
        registry_hostname: hostname.into(),
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
    }
}

fn host() -> Arc<Host> {
    Arc::new(Host::new("11111111-2222-4333-8444-555555555555", true, info("testhost")))
}

fn chart(host: &Host, id: &str, context: &str) -> Arc<Chart> {
    let (type_, id) = id.split_once('.').expect("type.id");
    let (chart, _) = host.charts().create(&ChartSpec {
        type_,
        id,
        name: None,
        family: Some("family"),
        context: Some(context),
        title: "title",
        units: "units",
        plugin: "difftest.plugin",
        module: None,
        priority: 1000,
        update_every: 1,
        chart_type: ChartType::Line,
        mode: DbMode::Ram,
        history_entries: 60,
        page_size: 4096,
    });
    chart
}

fn health(config: HealthConfig, text: &str) -> Arc<Health> {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("test.conf");
    std::fs::write(&path, text).expect("the file");
    let health = Health::init(config, Box::new(|_| {}), false);
    assert!(health_readfile(&health, path.as_os_str().as_bytes(), false));
    health
}

const RULE_A: &str = "template: on_a\n on: ctx.a\n every: 10s\n calc: 1\n\n";

fn linked(health: &Health, host: &Host) -> Vec<(String, String)> {
    let alerts = health.host(host).map(|alerts| alerts.alerts()).unwrap_or_default();
    alerts.iter().map(|a| (String::from_utf8_lossy(a.name()).into_owned(), a.chart.id().to_owned())).collect()
}

/// C: a freed chart's delete callback unlinks the alerts on that RRDSET's own list (`rrdcalc.c:737-761` walks
/// `st->alerts.base`). A new chart of the same id is another RRDSET with a list of its own.
/// Here the old chart's event arrives after the new chart of that id was linked (the hook is called after the
/// index lock is released: `rrd/src/chart.rs` `Charts::free_if`): the alerts are found by the chart object.
#[test]
fn the_free_of_an_old_chart_leaves_the_alerts_of_the_new_chart_of_that_id() {
    let health = health(HealthConfig::default(), RULE_A);
    let host = host();
    chart(&host, "t.keep", "ctx.a");
    let old = chart(&host, "t.x", "ctx.other");
    health.host_link(&host, &|| NOW, &|| true);
    assert_eq!(linked(&health, &host), [("on_a".to_owned(), "t.keep".to_owned())]);

    // the old chart leaves the index; its event to health is still on its way
    assert!(host.charts().free_if(&old, |_| true));
    // the collector defines the chart again, now of the context the rule is for, and a pass links it
    let new = chart(&host, "t.x", "ctx.a");
    assert!(!Arc::ptr_eq(&old, &new));
    health.host_link(&host, &|| NOW + 1, &|| true);
    let both = [("on_a".to_owned(), "t.keep".to_owned()), ("on_a".to_owned(), "t.x".to_owned())];
    assert_eq!(linked(&health, &host), both);

    // the old chart's event arrives
    health.chart_freed(host.machine_guid(), &old, &Idle, &|| NOW + 2);
    assert_eq!(linked(&health, &host), both, "the old chart had no alert: nothing to unlink");
}

/// C: a host that cannot enter the index (its GUID is there already) is freed with `rrdhost_free_unlinked()`, whose
/// `rrdcalc_delete_all(host)` works on that new host's own, empty alert index (`database/rrdhost.c`
/// `rrdhost_cleanup_data_collection_and_health()`); the host that owns the GUID keeps its alerts.
/// The database's events name the host object, not its GUID.
#[test]
fn a_host_that_lost_the_index_collision_leaves_the_indexed_host_s_alerts() {
    use netdata_agent_rrd::host::Hosts;
    use netdata_agent_rrd::storage::HealthEvent;

    let health = health(HealthConfig::default(), RULE_A);
    let hosts = Hosts::new(Host::new("11111111-2222-4333-8444-555555555555", true, info("localhost")));
    // the daemon's hook: daemon/src/main.rs (set_health_hook) and daemon/src/health.rs database_event()
    hosts.storage().set_health_hook({
        let health = Arc::clone(&health);
        move |event| match event {
            HealthEvent::ChartFreed(host, chart) => health.chart_freed(host, chart, &Idle, &|| NOW),
            HealthEvent::HostCleanup(host) => health.host_cleanup(host, &Idle, &|| NOW),
            HealthEvent::HostFreed(host) => health.host_freed(host),
        }
    });

    // 38 characters: the host keeps 37 of them, and a lookup by the whole GUID never finds it
    let long = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeeeXY";
    let child = hosts.find_or_create(long, DbMode::Ram, || info("child"), |_| {}).expect("the child");
    chart(&child, "t.a", "ctx.a");
    health.host_link(&child, &|| NOW, &|| true);
    let one = [("on_a".to_owned(), "t.a".to_owned())];
    assert_eq!(linked(&health, &child), one);

    // the same GUID again: a new host is made, collides in the index and is freed
    assert!(hosts.find_or_create(long, DbMode::Ram, || info("child"), |_| {}).is_err());
    assert_eq!(linked(&health, &child), one, "the indexed host's alerts are its own");
}

/// Four threads for a second on one host: one defines charts, one frees them (the database's hook unlinks),
/// one runs health passes, one reads what the API reads. Looks for a deadlock, for an alert left on a freed
/// chart, and for a live chart left without its alert once everything is quiet.
#[test]
fn four_threads_define_free_pass_and_read() {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    use netdata_agent_health::api::{alarm_variables_json, linked_count, status_counts};
    use netdata_agent_health::variable::trace_json;
    use netdata_agent_rrd::host::Hosts;
    use netdata_agent_rrd::storage::HealthEvent;

    const CHARTS: usize = 8;
    let health = health(HealthConfig::default(), RULE_A);
    let hosts = Arc::new(Hosts::new(Host::new("11111111-2222-4333-8444-555555555555", true, info("localhost"))));
    hosts.storage().set_health_hook({
        let health = Arc::clone(&health);
        move |event| match event {
            HealthEvent::ChartFreed(host, chart) => health.chart_freed(host, chart, &Idle, &|| NOW),
            HealthEvent::HostCleanup(host) => health.host_cleanup(host, &Idle, &|| NOW),
            HealthEvent::HostFreed(host) => health.host_freed(host),
        }
    });
    let guid = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
    let child = hosts.find_or_create(guid, DbMode::Ram, || info("child"), |_| {}).expect("the child");

    let stop = Arc::new(AtomicBool::new(false));
    let (done, finished) = mpsc::channel::<&'static str>();
    let (defined, freed, passes, reads) = (
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
        Arc::new(AtomicUsize::new(0)),
    );

    {
        let (child, stop, done, defined) = (Arc::clone(&child), Arc::clone(&stop), done.clone(), Arc::clone(&defined));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for i in 0..CHARTS {
                    chart(&child, &format!("t.c{i}"), "ctx.a");
                    defined.fetch_add(1, Ordering::Relaxed);
                }
            }
            let _ = done.send("define");
        });
    }
    {
        let (child, stop, done, freed) = (Arc::clone(&child), Arc::clone(&stop), done.clone(), Arc::clone(&freed));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                for i in 0..CHARTS {
                    if let Some(c) = child.charts().find(&format!("t.c{i}"), true)
                        && child.charts().free_if(&c, |_| true)
                    {
                        freed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            let _ = done.send("free");
        });
    }
    {
        let (child, stop, done, passes, health) =
            (Arc::clone(&child), Arc::clone(&stop), done.clone(), Arc::clone(&passes), Arc::clone(&health));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if passes.fetch_add(1, Ordering::Relaxed) % 16 == 15 {
                    child.raise_label_recheck();
                }
                health.host_link(&child, &|| NOW, &|| true);
            }
            let _ = done.send("pass");
        });
    }
    {
        let (child, stop, done, reads, health) =
            (Arc::clone(&child), Arc::clone(&stop), done.clone(), Arc::clone(&reads), Arc::clone(&health));
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let alerts = health.host(&child);
                for c in child.charts().all() {
                    let _ = alarm_variables_json(&child, alerts.as_deref(), &c, NOW);
                    let _ = trace_json(&child, alerts.as_deref(), &c, b"on_a", &|| NOW);
                    reads.fetch_add(1, Ordering::Relaxed);
                }
                let _ = (status_counts(alerts.as_deref()), linked_count(alerts.as_deref()));
            }
            let _ = done.send("read");
        });
    }

    std::thread::sleep(Duration::from_secs(1));
    stop.store(true, Ordering::Relaxed);
    for _ in 0..4 {
        let name = finished.recv_timeout(Duration::from_secs(30)).expect("a thread did not stop: a deadlock");
        let _ = name;
    }
    assert!(defined.load(Ordering::Relaxed) > 0 && passes.load(Ordering::Relaxed) > 0);
    assert!(freed.load(Ordering::Relaxed) > 0 && reads.load(Ordering::Relaxed) > 0);

    // quiet now: a pass takes whatever is pending (no host recheck: only the flagged charts are done)
    health.host_link(&child, &|| NOW, &|| true);
    let alerts = health.host(&child).expect("the child's alerts");
    let on_freed: Vec<String> =
        alerts.alerts().iter().filter(|a| a.chart.is_freed()).map(|a| a.chart.id().to_owned()).collect();
    assert!(on_freed.is_empty(), "alerts left on freed charts: {on_freed:?}");
    let stale: Vec<String> = alerts
        .alerts()
        .iter()
        .filter(|a| !child.charts().find(a.chart.id(), true).is_some_and(|c| Arc::ptr_eq(&c, &a.chart)))
        .map(|a| a.chart.id().to_owned())
        .collect();
    assert!(stale.is_empty(), "alerts on charts that are not the host's: {stale:?}");
    let bare: Vec<String> = child
        .charts()
        .all()
        .iter()
        .filter(|c| alerts.chart_alerts(c).len() != 1)
        .map(|c| c.id().to_owned())
        .collect();
    assert!(bare.is_empty(), "live charts without their alert after a quiet pass: {bare:?}");
}
