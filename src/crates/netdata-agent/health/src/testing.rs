//! Test fixtures: a host with charts, and a `Health` holding the rules of a health.d text.

use std::os::unix::ffi::OsStrExt;
use std::sync::Arc;

use netdata_agent_rrd::chart::{Algorithm, Chart, ChartSpec, ChartType};
use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::labels::SRC_CONFIG;
use netdata_agent_rrd::mode::DbMode;

use netdata_agent_metadata::health_log::LoadedRow;
use netdata_agent_query::value::{ValueRequest, ValueResult};

use crate::Health;
use crate::alert::Alert;
use crate::alerts::HostAlerts;
use crate::config::HealthConfig;
use crate::entry::Entry;
use crate::pass::{ChartFacts, Env, Idle};
use crate::readfile::health_readfile;

/// A localhost with health enabled and the given labels.
pub(crate) fn host(labels: &[(&str, &str)]) -> Arc<Host> {
    host_of("11111111-2222-4333-8444-555555555555", labels)
}

/// A host of that machine GUID with health enabled and the given labels.
pub(crate) fn host_of(guid: &str, labels: &[(&str, &str)]) -> Arc<Host> {
    let info = HostInfo {
        hostname: "testhost".into(),
        registry_hostname: "testhost".into(),
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
    let host = Arc::new(Host::new(guid, true, info));
    host.update_labels(|set| {
        for (name, value) in labels {
            set.add(name.as_bytes(), value.as_bytes(), SRC_CONFIG);
        }
    });
    host
}

/// The chart `type.id` of `context` on `host`, collected every second, with the given labels.
pub(crate) fn chart(host: &Host, id: &str, name: Option<&str>, context: &str, labels: &[(&str, &str)]) -> Arc<Chart> {
    chart_every(host, id, name, Some(context), 1, labels)
}

/// The chart `type.id` on `host`: family `family`, units `units`; without a context its id is its context.
pub(crate) fn chart_every(
    host: &Host,
    id: &str,
    name: Option<&str>,
    context: Option<&str>,
    update_every: i32,
    labels: &[(&str, &str)],
) -> Arc<Chart> {
    let (type_, id) = id.split_once('.').expect("type.id");
    let (chart, _) = host.charts().create(&ChartSpec {
        type_,
        id,
        name,
        family: Some("family"),
        context,
        title: "title",
        units: "units",
        plugin: "difftest.plugin",
        module: None,
        priority: 1000,
        update_every,
        chart_type: ChartType::Line,
        mode: DbMode::Ram,
        history_entries: 60,
        page_size: 4096,
    });
    chart.update_meta(|meta| {
        for (name, value) in labels {
            meta.labels.add(name.as_bytes(), value.as_bytes(), SRC_CONFIG);
        }
    });
    chart
}

/// The host of the recorded case `tests/vectors/variables/` (its `inputs/plugin-lines.txt`): five charts, of
/// which only `hv.a` is collected, with their variables and the host's. The labels are those a plugin's chart
/// gets, and the two the case sets.
pub(crate) fn variables_case_host() -> Arc<Host> {
    const PLUGIN: [(&str, &str); 2] = [("_collect_plugin", "difftest.plugin"), ("_collect_module", "[none]")];
    let absolute = |chart: &Chart, id: &str, name: Option<&str>| {
        chart.dim_add(id, name, 1, 1, Algorithm::Absolute);
    };
    let host = host(&[]);
    host.update_info(|info| info.hostname = "parity-parent".into());

    let b = chart_every(&host, "hv.b", Some("bname"), Some("hv.ctx"), 5, &[PLUGIN[0], PLUGIN[1], ("kind", "x")]);
    absolute(&b, "a", None);
    for (name, value) in [("cv", 7.0), ("cv_raw", 8.0), ("hv.b.dv", 9.0), ("a", 6.0), ("c+v", 4.0)] {
        b.set_variable(name, value);
    }
    let c = chart_every(&host, "hv.c", None, Some("hv.ctx"), 1, &[PLUGIN[0], PLUGIN[1], ("kind", "y")]);
    absolute(&c, "a", Some("aname"));
    absolute(&c, "p.q", None);
    let d = chart_every(&host, "hv.d", None, None, 1, &PLUGIN);
    absolute(&d, "a", None);
    let e = chart_every(&host, "hv.e", None, Some("hvflat"), 1, &PLUGIN);
    absolute(&e, "a", None);
    for (name, value) in [("hvh", 5.0), ("cv", 55.0), ("status", 77.0)] {
        host.set_variable(name, value);
    }
    let a = chart_every(&host, "hv.a", None, Some("hv.ctx"), 1, &PLUGIN);
    absolute(&a, "a", None);
    absolute(&a, "b", None);
    host
}

/// `hv.a` of the recorded case as collected at `second`: `a` is 10 and `b` is 20.
pub(crate) fn variables_case_collect(host: &Host, second: i64) {
    let chart = host.charts().find("hv.a", true).expect("hv.a");
    chart.update_collection(|collection| collection.last_collected = (second, 0));
    for (id, value) in [("a", 10), ("b", 20)] {
        chart.dim(id).expect("the dimension").update_collection(|collection| {
            collection.last_collected_time = (second, 0);
            collection.last_collected_value = value;
            collection.last_stored_value = value as f64;
        });
    }
}

/// The rules of the recorded case with health on.
pub(crate) fn variables_case_rules() -> String {
    let rules = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/vectors/variables/on/inputs/health.d-parity.conf");
    std::fs::read_to_string(rules).expect("the case's rules")
}

/// The chart a request names: by id, then by name.
pub(crate) fn find_chart(host: &Host, chart: &str) -> Arc<Chart> {
    host.charts().find(chart, false).or_else(|| host.charts().find_by_name(chart)).expect("the chart")
}

/// The alerts as (name, chart id), in the given order.
pub(crate) fn named(alerts: &[Arc<Alert>]) -> Vec<(String, String)> {
    alerts
        .iter()
        .map(|alert| (String::from_utf8_lossy(alert.name()).into_owned(), alert.chart.id().to_owned()))
        .collect()
}

pub(crate) fn pair(name: &str, chart: &str) -> (String, String) {
    (name.to_owned(), chart.to_owned())
}

/// The rows of a recorded case's `index.tsv` as (request, status, body).
pub(crate) fn variables_case_rows(case: &str) -> Vec<(String, u16, Vec<u8>)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/variables").join(case);
    let index = std::fs::read_to_string(dir.join("index.tsv")).expect("index.tsv");
    index
        .lines()
        .skip(1)
        .map(|line| {
            let fields: Vec<&str> = line.split('\t').collect();
            let body = std::fs::read(dir.join(fields[0])).expect("a recorded body");
            (fields[3].to_owned(), fields[1].parse().expect("a status"), body)
        })
        .collect()
}

/// The integer a recorded body holds for `"member":`, the first one.
pub(crate) fn recorded_integer(body: &[u8], member: &str) -> Option<i64> {
    let text = std::str::from_utf8(body).ok()?;
    let rest = &text[text.find(&format!("\"{member}\":"))? + member.len() + 3..];
    let end = rest.find(|c: char| c != '-' && !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

/// A `Health` with the rules of `text`, read as a user's health.d file.
pub(crate) fn health_with(text: &str) -> Arc<Health> {
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("test.conf");
    std::fs::write(&path, text).expect("the file");
    let health = Health::init(HealthConfig::default(), Box::new(|_| {}));
    assert!(health_readfile(&health, path.as_os_str().as_bytes(), false));
    health
}

/// A rule's text: a template or an alarm of `name` on `on`, with extra lines.
pub(crate) fn rule_text(kind: &str, name: &str, on: &str, lines: &[&str]) -> String {
    let mut text = format!("{kind}: {name}\n on: {on}\n every: 10s\n calc: 1\n");
    for line in lines {
        text.push_str(&format!(" {line}\n"));
    }
    text.push('\n');
    text
}

/// An [`Env`] a test sets: the exit flag, whether a save marks an entry as saved, the second every chart was last
/// collected at, with data from 100 seconds before it (none: no chart is collected), and the rows the alert log's
/// table gives a host's first pass (none: no database). It counts what it gave out, so every entry has its own
/// ids.
#[derive(Default)]
pub struct Scripted {
    pub exiting: bool,
    pub saves: bool,
    pub collected: Option<i64>,
    pub table: Option<Vec<LoadedRow>>,
    pub ids: std::cell::Cell<u64>,
}

impl Env for Scripted {
    fn facts(&self, chart: &Chart) -> ChartFacts {
        match self.collected {
            Some(second) => ChartFacts { obsolete: false, last_collected_s: second, counter_done: 2, update_every: 1 },
            None => Idle.facts(chart),
        }
    }

    fn retention(&self, chart: &Chart) -> (i64, i64) {
        self.collected.map_or_else(|| Idle.retention(chart), |second| (second - 100, second))
    }

    fn lookup(&self, host: &Arc<Host>, chart: &Arc<Chart>, request: &ValueRequest) -> ValueResult {
        Idle.lookup(host, chart, request)
    }

    fn now_usec(&self) -> u64 {
        self.ids.set(self.ids.get() + 1);
        self.ids.get()
    }

    fn transition_id(&self) -> [u8; 16] {
        u128::from(self.ids.get() + 1).to_be_bytes()
    }

    fn exiting(&self) -> bool {
        self.exiting
    }

    fn is_health_thread(&self) -> bool {
        true
    }

    fn service_running(&self) -> bool {
        true
    }

    fn load(&self, _: &Host) -> Option<Vec<LoadedRow>> {
        self.table.clone()
    }

    fn sql_alarm_id(&self, _: &Host, _: &[u8], _: Option<&[u8]>) -> Option<(u32, u32)> {
        None
    }

    fn queue_save(&self, _: &Arc<HostAlerts>, _: u32) -> bool {
        false
    }

    fn sql_save(&self, _: &Host, _: &Entry) -> bool {
        self.saves
    }

    fn commit_transitions(&self) {}

    fn process_pending_queue(&self, _: &Host) -> bool {
        false
    }

    fn notify(&self, _: &mut Entry) {}
}
