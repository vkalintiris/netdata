//! The variables an alert's expressions name (`health_variable.c`): `alert_variable_lookup_internal()`, and the
//! trace of a lookup that `/api/v1/variable` answers with.
//!
//! A name is looked for in six places, in this order, and the search ends at the first place that has it:
//! the alert's own values and the status constants; a dimension of the alert's chart; a variable of that chart; a
//! variable of the host; the alerts of that name on the host; and `CHART.DIMENSION` or `CONTEXT.DIMENSION` anywhere
//! on the host. Every hit past the first place is a candidate with a score, the number of labels its chart shares
//! with the alert's chart, and the first candidate with the highest score gives the value.

use std::sync::Arc;

use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::labels::Labels;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::Clock;
use crate::alert::{Alert, Run, Status};
use crate::alerts::HostAlerts;

/// What a lookup reads of the alert it resolves for (`rc`).
#[derive(Debug, Clone, Copy)]
pub struct This<'a> {
    /// The alert under evaluation; none for a trace, which C runs with a blank alert on the chart.
    pub alert: Option<&'a Arc<Alert>>,
    /// The alert's chart.
    pub chart: &'a Arc<Chart>,
    pub value: f64,
    pub db_after: i64,
    pub db_before: i64,
    pub status: Status,
}

impl<'a> This<'a> {
    /// The blank alert a trace resolves for (`alert_variable_lookup_trace()`): only its chart is set, so its value,
    /// its query window and its status are zero.
    pub fn blank(chart: &'a Arc<Chart>) -> This<'a> {
        This { alert: None, chart, value: 0.0, db_after: 0, db_before: 0, status: Status::Uninitialized }
    }

    /// An alert with its live fields as they are now.
    pub fn of(alert: &'a Arc<Alert>, run: &Run) -> This<'a> {
        This {
            alert: Some(alert),
            chart: &alert.chart,
            value: run.value,
            db_after: run.db_after,
            db_before: run.db_before,
            status: run.status,
        }
    }
}

/// A resolved variable.
#[derive(Debug, Clone)]
pub struct Found {
    pub value: f64,
    /// What the value is, as the trace's `description` says it.
    pub description: &'static str,
    /// The chart the value came from.
    pub chart: Arc<Chart>,
    /// How many candidates there were; 1 for a name the alert itself answers.
    pub candidates: usize,
}

/// What a dimension's name stands for: the name itself (its last stored value), or the name with `_raw` or
/// `_last_collected_t` taken off its end.
#[derive(Clone, Copy)]
enum Select {
    Stored,
    Raw,
    LastCollected,
}

struct Candidate {
    value: f64,
    score: usize,
    chart: Arc<Chart>,
    description: &'static str,
}

struct Job<'a> {
    host: &'a Host,
    chart: &'a Arc<Chart>,
    /// The whole name: chart variables, host variables and alert names are looked up by it, whatever its end.
    variable: &'a [u8],
    select: Select,
    /// The labels of the alert's chart, read when the first candidate is scored.
    labels: Option<Labels>,
    candidates: Vec<Candidate>,
}

impl Job<'_> {
    /// `variable_lookup_add_result_with_score()`: a candidate, when the host still has a chart of that id (obsolete
    /// or not), which is then the chart the candidate counts for.
    fn add(&mut self, value: f64, chart: &Chart, description: &'static str) {
        let Some(chart) = self.host.charts().find(chart.id(), true) else {
            return;
        };
        let labels = self.labels.get_or_insert_with(|| self.chart.with_meta(|meta| meta.labels.clone()));
        let score = chart.with_meta(|meta| labels.common_count(&meta.labels));
        self.candidates.push(Candidate { value, score, chart, description });
    }

    /// `variable_lookup_in_chart()`: the first dimension of the chart whose id or name is `dimension`, then the
    /// chart's variable of the whole name. On the alert's own chart the dimension ends the search.
    fn in_chart(&mut self, chart: &Arc<Chart>, dimension: &[u8], stop_on_match: bool) -> bool {
        let mut found = false;
        let dim = chart
            .dims()
            .into_iter()
            .find(|dim| dim.id().as_bytes() == dimension || dim.meta().name.as_bytes() == dimension);
        if let Some(dim) = dim {
            let collection = dim.collection();
            match self.select {
                Select::Stored => self.add(collection.last_stored_value, chart, "last stored value of dimension"),
                Select::Raw => {
                    let value = collection.last_collected_as_double(dim.is_float());
                    self.add(value, chart, "last collected value of dimension");
                }
                Select::LastCollected => {
                    self.add(collection.last_collected_time.0 as f64, chart, "last collected time of dimension");
                }
            }
            found = true;
        }
        if found && stop_on_match {
            return true;
        }

        if let Some(value) = chart.variable(self.variable) {
            self.add(value, chart, "chart variable");
            found = true;
        }
        found
    }

    /// `variable_lookup_context()`: the chart with id `chart_or_context` (obsolete ones excluded), then every
    /// instance of the context of that id that has a chart. A chart found both ways counts twice.
    fn in_context(&mut self, chart_or_context: &[u8], dimension: &[u8]) -> bool {
        let Ok(id) = std::str::from_utf8(chart_or_context) else {
            return false;
        };
        let mut found = false;
        if let Some(chart) = self.host.charts().find(id, false) {
            found |= self.in_chart(&chart, dimension, false);
        }
        if let Some(context) = self.host.contexts().get(id) {
            for instance in context.instances() {
                if let Some(chart) = instance.chart() {
                    found |= self.in_chart(&chart, dimension, false);
                }
            }
        }
        found
    }

    /// `alert_variable_from_running_alerts()`: every linked alert of that name, with its live value, or with its
    /// published one for a trace. The alert under evaluation answers with the value it has so far.
    fn running_alerts(&mut self, alerts: &HostAlerts, this: &This, snapshot_values: bool) -> bool {
        let mut found = false;
        for alert in alerts.by_name(self.variable) {
            if !alerts.is_linked(self.host, &alert) {
                continue;
            }
            let value = if snapshot_values {
                alert.snapshot().value
            } else if this.alert.is_some_and(|own| Arc::ptr_eq(own, &alert)) {
                this.value
            } else {
                alert.run().value
            };
            self.add(value, &alert.chart, "alarm value");
            found = true;
        }
        found
    }
}

/// `alert_variable_lookup_internal()`: the value of `name` for the alert `this` describes, or none. A lookup for an
/// alert that is no longer linked finds nothing. `snapshot_values` is C's `wb != NULL`: a trace reads other alerts'
/// published values. `clock` is read when the name is `now`.
///
/// The caller holds no alert's live fields locked: the alerts of a name are read through the host's store, which
/// the linking side holds while it takes an alert's.
pub fn lookup(
    host: &Host,
    alerts: Option<&HostAlerts>,
    this: &This,
    name: &[u8],
    snapshot_values: bool,
    clock: Clock,
) -> Option<Found> {
    let chart = this.chart;
    let linked = match this.alert {
        Some(alert) => alerts.is_some_and(|alerts| alerts.is_linked(host, alert)),
        None => host.charts().find(chart.id(), true).is_some_and(|found| Arc::ptr_eq(&found, chart)),
    };
    if !linked {
        return None;
    }

    let own = |value: f64, description: &'static str| {
        Some(Found { value, description, chart: Arc::clone(chart), candidates: 1 })
    };
    match name {
        b"this" => return own(this.value, "current alert value"),
        b"after" => return own(this.db_after as f64, "current alert query start time"),
        b"before" => return own(this.db_before as f64, "current alert query end time"),
        b"now" => return own(clock() as f64, "current wall-time clock timestamp"),
        b"status" => return own(f64::from(this.status as i32), "current alert status"),
        b"REMOVED" => return own(f64::from(Status::Removed as i32), "removed status constant"),
        b"UNINITIALIZED" => return own(f64::from(Status::Uninitialized as i32), "uninitialized status constant"),
        b"UNDEFINED" => return own(f64::from(Status::Undefined as i32), "undefined status constant"),
        b"CLEAR" => return own(f64::from(Status::Clear as i32), "clear status constant"),
        b"WARNING" => return own(f64::from(Status::Warning as i32), "warning status constant"),
        b"CRITICAL" => return own(f64::from(Status::Critical as i32), "critical status constant"),
        b"last_collected_t" => {
            return own(chart.collection().last_collected.0 as f64, "current instance last_collected_t");
        }
        b"update_every" => return own(f64::from(chart.update_every()), "current instance update_every"),
        _ => {}
    }

    let (dimension, select) = if let Some(dimension) = name.strip_suffix(b"_raw") {
        (dimension, Select::Raw)
    } else if let Some(dimension) = name.strip_suffix(b"_last_collected_t") {
        (dimension, Select::LastCollected)
    } else {
        (name, Select::Stored)
    };
    let mut job = Job { host, chart, variable: name, select, labels: None, candidates: Vec::new() };

    let found = 'search: {
        if job.in_chart(chart, dimension, true) {
            break 'search true;
        }
        if let Some(value) = host.variable(name) {
            job.add(value, chart, "host variable");
            break 'search true;
        }
        if alerts.is_some_and(|alerts| job.running_alerts(alerts, this, snapshot_values)) {
            break 'search true;
        }

        // CHART.DIMENSION or CONTEXT.DIMENSION: split at the last dot, then at each dot before it, while what is
        // left of the split still holds a dot
        let mut found = false;
        let mut end = dimension.len();
        while let Some(dot) = dimension[..end].iter().rposition(|&byte| byte == b'.') {
            if !dimension[..dot].contains(&b'.') {
                break;
            }
            found |= job.in_context(&dimension[..dot], &dimension[dot + 1..]);
            end = dot;
        }
        found
    };
    if !found {
        return None;
    }

    // the first candidate with the highest score
    let candidates = job.candidates.len();
    let mut best: Option<Candidate> = None;
    for candidate in job.candidates {
        if best.as_ref().is_none_or(|best| candidate.score > best.score) {
            best = Some(candidate);
        }
    }
    best.map(|best| Found { value: best.value, description: best.description, chart: best.chart, candidates })
}

/// `alert_variable_lookup_trace()`: the body of `/api/v1/variable`, the lookup of `name` for a blank alert on
/// `chart`, with where the value came from.
pub fn trace_json(
    host: &Host,
    alerts: Option<&HostAlerts>,
    chart: &Arc<Chart>,
    name: &[u8],
    clock: Clock,
) -> Vec<u8> {
    let found = lookup(host, alerts, &This::blank(chart), name, true, clock);
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("variable", name);
    w.member_add_string("instance", chart.id());
    chart.with_meta(|meta| w.member_add_string("context", &meta.context));
    w.member_add_boolean("found", found.is_some());
    if let Some(found) = found {
        w.member_add_double("value", found.value);
        w.member_add_object("source");
        w.member_add_string("description", found.description);
        w.member_add_string("instance", found.chart.id());
        found.chart.with_meta(|meta| w.member_add_string("context", &meta.context));
        w.member_add_uint64("candidates", found.candidates as u64);
        w.object_close();
    }
    w.finalize();
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Health;
    use crate::testing::{
        find_chart, health_with, recorded_integer, variables_case_collect, variables_case_host, variables_case_rows,
        variables_case_rules,
    };

    const NOW: i64 = 1_700_000_000;

    /// The recorded case with health on: its two rules linked, as the host's first pass links them.
    fn linked_case() -> (Arc<Health>, Arc<Host>) {
        let health = health_with(&variables_case_rules());
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        health.host_pass(&host, &|| NOW, &|| true);
        (health, host)
    }

    /// The traces the C agent answered, with health off and with two linked rules: the same bytes. The wall clock
    /// and the second `hv.a` was last collected at are the ones each recorded answer shows.
    #[test]
    fn traces_match_c() {
        for (case, traces) in [("off", 50), ("on", 51)] {
            let (health, host) = match case {
                "on" => linked_case(),
                _ => {
                    let host = variables_case_host();
                    variables_case_collect(&host, NOW);
                    (health_with(""), host)
                }
            };
            let alerts = health.host(&host);
            assert_eq!(alerts.is_some(), case == "on");

            let mut compared = 0;
            for (request, status, body) in variables_case_rows(case) {
                let query = ["/api/v1/variable?", "/api/v3/variable?"].iter().find_map(|path| request.strip_prefix(path));
                let Some(query) = query.filter(|_| status == 200) else {
                    continue;
                };
                let parameter = |name: &str| {
                    query.split('&').find_map(|pair| pair.strip_prefix(name)?.strip_prefix('=')).expect("the parameter")
                };
                let (chart, variable) = (parameter("chart"), parameter("variable"));
                let chart = find_chart(&host, chart);

                let value = recorded_integer(&body, "value");
                let clock = if variable == "now" { value.expect("the recorded clock") } else { NOW };
                if let Some(second) = value.filter(|&second| second != 0 && variable.ends_with("last_collected_t")) {
                    variables_case_collect(&host, second);
                }
                let json = trace_json(&host, alerts.as_deref(), &chart, variable.as_bytes(), &|| clock);
                assert_eq!(String::from_utf8_lossy(&json), String::from_utf8_lossy(&body), "{case}: {request}");
                compared += 1;
            }
            assert_eq!(compared, traces, "{case}");
        }
    }

    /// In the loop an alert reads its own live fields, and other alerts' live values; a trace reads what they
    /// published.
    #[test]
    fn an_alert_reads_live_values() {
        let (health, host) = linked_case();
        let alerts = health.host(&host).expect("the host's alerts");
        let chart = |id: &str| host.charts().find(id, true).expect("the chart");
        let alert = |chart_id: &str, name: &[u8]| {
            let linked = alerts.chart_alerts(&chart(chart_id));
            Arc::clone(linked.iter().find(|alert| alert.name() == name).expect("the alert"))
        };
        for (chart_id, value) in [("hv.b", 1.0), ("hv.c", 2.0), ("hv.a", 3.0)] {
            alert(chart_id, b"hv_same").run().value = value;
        }
        let one = alert("hv.a", b"hv_one");
        {
            let mut run = one.run();
            (run.value, run.db_after, run.db_before, run.status) = (42.0, 100, 200, Status::Warning);
        }
        let value = |this: &This, name: &[u8], snapshot_values: bool| {
            let found = lookup(&host, Some(&alerts), this, name, snapshot_values, &|| NOW);
            found.map(|found| (found.value, found.chart.id().to_owned()))
        };
        let hv = |value: f64, chart: &str| Some((value, chart.to_owned()));

        let this = This::of(&one, &one.run());
        assert_eq!(value(&this, b"this", false), hv(42.0, "hv.a"));
        assert_eq!(value(&this, b"after", false), hv(100.0, "hv.a"));
        assert_eq!(value(&this, b"before", false), hv(200.0, "hv.a"));
        assert_eq!(value(&this, b"status", false), hv(3.0, "hv.a"));
        assert_eq!(value(&this, b"now", false), hv(NOW as f64, "hv.a"));
        // three alerts of the name with one score: the first linked, with its live value
        assert_eq!(value(&this, b"hv_same", false), hv(1.0, "hv.b"));
        // what a trace reads: nothing was published
        assert!(value(&this, b"hv_same", true).is_some_and(|(value, chart)| value.is_nan() && chart == "hv.b"));

        // an alert that names itself: the value it has so far in this evaluation, not the one of its last
        let same = alert("hv.c", b"hv_same");
        let mut this = This::of(&same, &same.run());
        this.value = 7.0;
        assert_eq!(value(&this, b"hv_same", false), hv(7.0, "hv.c"), "its own chart shares the most labels");
        // and another alert's is read from that alert
        assert_eq!(value(&this, b"hv_one", false), hv(42.0, "hv.a"));

        // an alert whose chart is gone is not linked, whether its free reached health or not: it finds nothing, not
        // even its own values, and is no candidate for the others
        let candidates = |this: &This| {
            lookup(&host, Some(&alerts), this, b"hv_same", false, &|| NOW).map(|found| found.candidates)
        };
        assert_eq!(candidates(&This::of(&one, &one.run())), Some(3));
        assert!(host.charts().free_if(&chart("hv.c"), |_| true));
        assert_eq!(alerts.by_name(b"hv_same").len(), 3, "still in the name index");
        assert_eq!(value(&this, b"this", false), None);
        assert_eq!(value(&this, b"hv_one", false), None);
        assert_eq!(candidates(&This::of(&one, &one.run())), Some(2));
        // nor is it one when a chart of that id exists again: the alert hangs on the old chart object
        let again = crate::testing::chart_every(&host, "hv.c", None, Some("hv.ctx"), 1, &[]);
        assert!(!Arc::ptr_eq(&again, &same.chart));
        assert_eq!(candidates(&This::of(&one, &one.run())), Some(2));
        health.chart_freed(host.machine_guid(), &same.chart, &|| NOW, false);
        assert_eq!(alerts.by_name(b"hv_same").len(), 2);
        assert_eq!(candidates(&This::of(&one, &one.run())), Some(2));
    }

    /// What the recorded case has no object for: a float dimension's raw value is its float lane; a dimension's
    /// time is its own, not its chart's; `CHART.DIM` does not find an obsolete chart by its id.
    #[test]
    fn a_float_dimension_its_own_time_and_an_obsolete_chart() {
        use netdata_agent_rrd::chart::dim_flags;
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        let a = find_chart(&host, "hv.a");
        let found = |name: &[u8]| {
            lookup(&host, None, &This::blank(&a), name, true, &|| NOW).map(|found| (found.value, found.candidates))
        };

        let b = a.dim("b").expect("the dimension");
        b.update_meta(|meta| meta.flags |= dim_flags::FLOAT);
        b.update_collection(|collection| {
            collection.last_collected_value_float = 1.5;
            collection.last_collected_time = (NOW - 7, 0);
        });
        assert_eq!(found(b"b_raw"), Some((1.5, 1)));
        assert_eq!(found(b"a_raw"), Some((10.0, 1)), "an integer dimension keeps its integer lane");
        assert_eq!(found(b"b_last_collected_t"), Some(((NOW - 7) as f64, 1)));
        assert_eq!(found(b"last_collected_t"), Some((NOW as f64, 1)));

        assert_eq!(found(b"hv.e.a"), Some((0.0, 1)));
        find_chart(&host, "hv.e").is_obsolete(&host);
        assert_eq!(found(b"hv.e.a"), None);
    }

    /// A blank alert is linked while the host's chart of that id is the chart it names.
    #[test]
    fn a_trace_of_a_freed_chart_finds_nothing() {
        let host = variables_case_host();
        let chart = host.charts().find("hv.b", true).expect("hv.b");
        let found = |name: &[u8]| lookup(&host, None, &This::blank(&chart), name, true, &|| NOW).map(|found| found.value);
        assert_eq!(found(b"cv"), Some(7.0));
        assert_eq!(found(b"update_every"), Some(5.0));
        assert!(host.charts().free_if(&chart, |_| true));
        assert_eq!(found(b"cv"), None);
        assert_eq!(found(b"update_every"), None);
    }
}
