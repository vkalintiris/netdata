//! What the web API shows of a host's alerts and variables: the body of `/api/v1/alarm_variables`
//! (`health_api_v1_chart_variables2json()`), a chart's `alarms` member (`rrdset2json()`), and the counts of
//! `/api/v1/charts` and `/api/v1/info`.
//!
//! None of them looks at whether health is enabled: a host without alerts gives empty members and zero counts.

use std::sync::Arc;

use indexmap::IndexMap;
use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;
use netdata_agent_text::json::{JsonOptions, JsonWriter};

use crate::alert::Status;
use crate::alerts::HostAlerts;

/// `rrdset2json()`'s `alarms` members: the chart's alerts in link order, each with its published status.
pub fn chart_alarms_json(w: &mut JsonWriter, alerts: Option<&HostAlerts>, chart: &Chart) {
    for alert in alerts.map(|alerts| alerts.chart_alerts(chart)).unwrap_or_default() {
        w.member_add_object(alert.name());
        w.member_add_string_or_empty("id", Some(alert.name()));
        w.member_add_string_or_empty("status", Some(alert.snapshot().status.name().as_bytes()));
        w.member_add_string_or_empty("units", alert.config.units.as_deref());
        w.member_add_int64("duration", i64::from(alert.config.update_every));
        w.object_close();
    }
}

/// `rrdvar_to_json_members()`: custom variables in insertion order, each a double.
pub fn variables_json(w: &mut JsonWriter, variables: &[(String, f64)]) {
    for (name, value) in variables {
        w.member_add_double(name, *value);
    }
}

/// `charts2json()`'s `alarms_count`: the host's alerts that have a chart, which every linked alert has.
pub fn linked_count(alerts: Option<&HostAlerts>) -> usize {
    alerts.map_or(0, HostAlerts::count)
}

/// `/api/v1/info`'s `alarms`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatusCounts {
    pub normal: u64,
    pub warning: u64,
    pub critical: u64,
}

/// `web_client_api_request_v1_info_summary_alarm_statuses()`: the alerts whose chart was collected at least once,
/// by published status. Every status that is not WARNING or CRITICAL counts as normal.
pub fn status_counts(alerts: Option<&HostAlerts>) -> StatusCounts {
    let mut counts = StatusCounts::default();
    for alert in alerts.map(HostAlerts::alerts).unwrap_or_default() {
        if alert.chart.collection().last_collected.0 == 0 {
            continue;
        }
        match alert.snapshot().status {
            Status::Warning => counts.warning += 1,
            Status::Critical => counts.critical += 1,
            _ => counts.normal += 1,
        }
    }
    counts
}

/// The alert that answers for its name in `alarm_variables`.
struct Scored {
    chart: Arc<Chart>,
    value: f64,
    score: usize,
}

/// `health_api_v1_chart_variables2json()`: the body of `/api/v1/alarm_variables` for `chart`: what an alert on it
/// could name. `now` is the wall clock's seconds.
pub fn alarm_variables_json(host: &Host, alerts: Option<&HostAlerts>, chart: &Chart, now: i64) -> Vec<u8> {
    let meta = chart.meta();
    let mut w = JsonWriter::new(JsonOptions::DEFAULT);
    w.member_add_string("chart", chart.id());
    w.member_add_string("chart_name", meta.name.as_deref().unwrap_or(chart.id()));
    w.member_add_string("chart_context", &meta.context);
    w.member_add_string("family", &meta.family);
    w.member_add_string("host", host.hostname());

    w.member_add_object("current_alert_values");
    w.member_add_double("this", f64::NAN);
    w.member_add_double("after", now as f64 - 1.0);
    w.member_add_double("before", now as f64);
    w.member_add_double("now", now as f64);
    w.member_add_double("status", f64::from(Status::Removed as i32));
    let constants =
        [Status::Removed, Status::Undefined, Status::Uninitialized, Status::Clear, Status::Warning, Status::Critical];
    for status in constants {
        w.member_add_double(status.name(), f64::from(status as i32));
    }
    w.member_add_double("green", f64::NAN);
    w.member_add_double("red", f64::NAN);
    w.object_close();

    // each dimension by its id, and by its name when that differs
    let dims: Vec<_> = chart
        .dims()
        .iter()
        .map(|dim| {
            let name = dim.meta().name;
            let names = if name == dim.id() { vec![name] } else { vec![dim.id().to_owned(), name] };
            (names, dim.collection(), dim.is_float())
        })
        .collect();

    w.member_add_object("dimensions_last_stored_values");
    for (names, collection, _) in &dims {
        for name in names {
            w.member_add_double(name, collection.last_stored_value);
        }
    }
    w.object_close();

    w.member_add_object("dimensions_last_collected_values");
    for (names, collection, is_float) in &dims {
        for name in names {
            let name = format!("{name}_raw");
            if *is_float {
                w.member_add_double(&name, collection.last_collected_as_double(true));
            } else {
                w.member_add_int64(&name, collection.last_collected_value);
            }
        }
    }
    w.object_close();

    w.member_add_object("dimensions_last_collected_time");
    for (names, collection, _) in &dims {
        for name in names {
            w.member_add_int64(format!("{name}_last_collected_t"), collection.last_collected_time.0);
        }
    }
    w.object_close();

    w.member_add_object("chart_variables");
    w.member_add_int64("update_every", i64::from(meta.update_every));
    w.member_add_uint64("last_collected_t", chart.collection().last_collected.0 as u64);
    variables_json(&mut w, &chart.variables());
    w.object_close();

    w.member_add_object("host_variables");
    variables_json(&mut w, &host.variables());
    w.object_close();

    // per name, the alert whose chart shares the most labels with this chart; the first on a tie
    w.member_add_object("alerts");
    let mut by_name: IndexMap<Vec<u8>, Scored> = IndexMap::new();
    for alert in alerts.map(HostAlerts::alerts).unwrap_or_default() {
        let scored = Scored {
            value: alert.snapshot().value,
            score: alert.chart.with_meta(|alert_meta| alert_meta.labels.common_count(&meta.labels)),
            chart: Arc::clone(&alert.chart),
        };
        match by_name.get_mut(alert.name()) {
            Some(best) if scored.score > best.score => *best = scored,
            Some(_) => {}
            None => {
                by_name.insert(alert.name().to_vec(), scored);
            }
        }
    }
    for (name, scored) in &by_name {
        w.member_add_object(name);
        w.member_add_double("value", scored.value);
        w.member_add_string("instance", scored.chart.id());
        scored.chart.with_meta(|meta| w.member_add_string("context", &meta.context));
        w.member_add_uint64("score", scored.score as u64);
        w.object_close();
    }
    w.object_close();

    w.finalize();
    w.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{
        find_chart, health_with, recorded_integer, variables_case_collect, variables_case_host, variables_case_rows,
        variables_case_rules,
    };

    const NOW: i64 = 1_700_000_000;

    /// The bodies the C agent answered `/api/v1/alarm_variables` with, with health off and with two linked rules:
    /// the same bytes, at the wall clock and the collection second each recorded answer shows.
    #[test]
    fn alarm_variables_match_c() {
        for (case, bodies) in [("off", 4), ("on", 3)] {
            let health = health_with(&if case == "on" { variables_case_rules() } else { String::new() });
            let host = variables_case_host();
            variables_case_collect(&host, NOW);
            if case == "on" {
                health.host_pass(&host, &|| NOW, &|| true);
            }
            let alerts = health.host(&host);

            let mut compared = 0;
            for (request, status, body) in variables_case_rows(case) {
                let Some(chart) = request.strip_prefix("/api/v1/alarm_variables?chart=").filter(|_| status == 200) else {
                    continue;
                };
                let chart = find_chart(&host, chart);
                let now = recorded_integer(&body, "now").expect("the recorded clock");
                if chart.id() == "hv.a" {
                    let second = recorded_integer(&body, "last_collected_t").expect("the recorded second");
                    variables_case_collect(&host, second);
                }
                let json = alarm_variables_json(&host, alerts.as_deref(), &chart, now);
                assert_eq!(String::from_utf8_lossy(&json), String::from_utf8_lossy(&body), "{case}: {request}");
                compared += 1;
            }
            assert_eq!(compared, bodies, "{case}");
        }
    }

    /// A float dimension's last collected value is printed from its float lane, as a double; an integer one as an
    /// integer.
    #[test]
    fn a_float_dimension_s_raw_value_is_a_double() {
        use netdata_agent_rrd::chart::dim_flags;
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        let a = find_chart(&host, "hv.a");
        let b = a.dim("b").expect("the dimension");
        b.update_meta(|meta| meta.flags |= dim_flags::FLOAT);
        b.update_collection(|collection| collection.last_collected_value_float = 1.5);
        let body = String::from_utf8(alarm_variables_json(&host, None, &a, NOW)).expect("text");
        assert!(body.contains("\"a_raw\":10,\n        \"b_raw\":1.5\n"), "{body}");
    }

    /// A chart's `alarms` member and the two counts, before the host's first pass and after it.
    #[test]
    fn a_chart_shows_its_alerts_and_the_host_counts_them() {
        let health = health_with(&variables_case_rules());
        let host = variables_case_host();
        variables_case_collect(&host, NOW);
        let chart = |id: &str| host.charts().find(id, true).expect("the chart");
        let alarms = |id: &str| {
            let mut w = JsonWriter::new(JsonOptions::MINIFY);
            w.member_add_object("alarms");
            chart_alarms_json(&mut w, health.host(&host).as_deref(), &chart(id));
            w.object_close();
            w.finalize();
            String::from_utf8(w.into_bytes()).expect("text")
        };
        let counts = || status_counts(health.host(&host).as_deref());

        assert_eq!(alarms("hv.a"), r#"{"alarms":{}}"#);
        assert_eq!(linked_count(health.host(&host).as_deref()), 0);
        assert_eq!(counts(), StatusCounts::default());

        health.host_pass(&host, &|| NOW, &|| true);
        // link order; the duration is the rule's `every`, on a chart collected every 5 seconds too
        assert_eq!(
            alarms("hv.a"),
            concat!(
                r#"{"alarms":{"hv_same":{"id":"hv_same","status":"UNINITIALIZED","units":"things","duration":1},"#,
                r#""hv_one":{"id":"hv_one","status":"UNINITIALIZED","units":"things","duration":1}}}"#
            )
        );
        assert_eq!(
            alarms("hv.b"),
            r#"{"alarms":{"hv_same":{"id":"hv_same","status":"UNINITIALIZED","units":"things","duration":1}}}"#
        );
        assert_eq!(alarms("hv.d"), r#"{"alarms":{}}"#);
        assert_eq!(linked_count(health.host(&host).as_deref()), 4);
        // the two alerts of the chart that was collected; any status but WARNING and CRITICAL is normal
        assert_eq!(counts(), StatusCounts { normal: 2, warning: 0, critical: 0 });

        let alerts = health.host(&host).expect("the host's alerts");
        let publish = |chart_id: &str, name: &[u8], status: Status| {
            let linked = alerts.chart_alerts(&chart(chart_id));
            let alert = linked.iter().find(|alert| alert.name() == name).expect("the alert");
            let mut run = alert.run();
            run.status = status;
            alert.publish(&run);
        };
        publish("hv.a", b"hv_same", Status::Warning);
        assert_eq!(counts(), StatusCounts { normal: 1, warning: 1, critical: 0 });
        publish("hv.a", b"hv_one", Status::Critical);
        assert_eq!(counts(), StatusCounts { normal: 0, warning: 1, critical: 1 });
        publish("hv.a", b"hv_one", Status::Removed);
        assert_eq!(counts(), StatusCounts { normal: 1, warning: 1, critical: 0 });
        // an alert of a chart that was never collected does not count, whatever its status
        publish("hv.b", b"hv_same", Status::Critical);
        assert_eq!(counts(), StatusCounts { normal: 1, warning: 1, critical: 0 });
        assert!(alarms("hv.a").starts_with(r#"{"alarms":{"hv_same":{"id":"hv_same","status":"WARNING","#));
    }
}
