//! A weights request from its parsed form to its results (`web_api_v12_weights()`,
//! `src/web/api/queries/weights.c`): the windows, the path (one query over every host, or the walk), what ends a
//! request without an answer, the spreading and the selection. The writers of the formats take it from there.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::pulse::Queries;
use netdata_agent_text::simple_pattern::SimplePattern;
use netdata_agent_text::time_window::relative_window_to_absolute_query;
use netdata_agent_web::progress::Tracker;
use netdata_agent_web::status;

use super::methods::{QueryEnv, Run, Stats};
use super::parse::{Format, WeightsRequest};
use super::results::{Registered, select, spread};
use super::walk::{Scope, Walk};
use super::{Method, timeout_ms, windows};
use crate::grouping::Windows;
use crate::request::Profile;
use crate::tables::{group_by, options};
use crate::target::{MetricFilters, Versions, label_pattern_array};

/// What a weights request needs of the agent: the hosts in the index's order, the CPUs C would spread the hosts
/// over (`netdata_conf_cpus()`), the storage profile, the configured limits of the two exponential smoothings, the
/// pulse counters, the wall clock, the client's interrupt and the request's progress row.
pub struct Env<'a> {
    pub hosts: Vec<Arc<Host>>,
    pub cpus: usize,
    pub profile: &'a Profile,
    pub windows: Windows,
    pub queries: Option<&'a Queries>,
    pub now_s: i64,
    pub interrupted: &'a dyn Fn(&mut i32) -> bool,
    pub progress: Option<Tracker<'a>>,
}

/// A request that ends without results: C's status and text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub code: u16,
    pub text: &'static str,
}

/// A version-1 request whose writer printed no dimension.
pub const NO_RESULTS: Refusal = Refusal { code: status::NOT_FOUND, text: "no results produced." };

impl Refusal {
    /// The body C writes in place of anything written before, with its two spaces.
    pub fn body(&self) -> Vec<u8> {
        format!("{{\"error\": \"{}\" }}", self.text).into_bytes()
    }
}

/// A request's results, ready for its format's writer.
pub struct Finished {
    /// The request as the engine leaves it: the windows absolute, the points settled, the options as they are
    /// printed (`nonzero` back in, the anomaly bit added for the anomaly-rate method), the group-by stripped of
    /// what the weights do not group by.
    pub request: WeightsRequest,
    pub shifts: u32,
    pub results: Vec<Registered>,
    pub stats: Stats,
    /// `examined_dimensions`.
    pub examined: usize,
    /// From the engine's start to the end of its work, before the writer.
    pub duration_us: u64,
    /// Zeros on the one-query path.
    pub versions: Versions,
}

/// The engine's outcome. `cacheable`: both ends of the highlighted window were absolute, which C marks on the
/// reply before anything can refuse the request.
pub struct Outcome {
    pub cacheable: bool,
    pub result: Result<Finished, Refusal>,
}

fn pattern(text: &Option<Vec<u8>>) -> Option<SimplePattern> {
    text.as_deref().and_then(SimplePattern::from_web)
}

/// `web_api_v12_weights()` up to its writers.
pub fn run(mut req: WeightsRequest, env: &Env) -> Outcome {
    let received = Instant::now();
    let timeout = timeout_ms(req.timeout_ms);
    let labels = |text: &Option<Vec<u8>>| pattern(text).map(|sp| label_pattern_array(&sp));
    let texts = &req.texts;
    let scope = Scope {
        scope_nodes: pattern(&texts.scope_nodes),
        nodes: pattern(&texts.nodes),
        scope_contexts: texts.scope_contexts.clone(),
        scope_contexts_sp: pattern(&texts.scope_contexts),
        contexts_sp: pattern(&texts.contexts),
        metrics: MetricFilters {
            version: req.version,
            match_ids: true,
            match_names: true,
            scope_instances: pattern(&texts.scope_instances),
            scope_labels: labels(&texts.scope_labels),
            scope_dimensions: pattern(&texts.scope_dimensions),
            instances: pattern(&texts.instances),
            chart_label_key: None,
            labels: labels(&texts.labels),
            alerts: pattern(&texts.alerts),
            dimensions: pattern(&texts.dimensions),
        },
    };
    let (_, _, cacheable) = relative_window_to_absolute_query(req.after, req.before, env.now_s);
    let refused = |code: u16, text: &'static str| Outcome { cacheable, result: Err(Refusal { code, text }) };
    let highlighted = (req.after, req.before);
    let baseline = (req.baseline_after, req.baseline_before);
    let w = match windows(req.method, highlighted, baseline, req.points, env.now_s) {
        Ok(w) => w,
        Err(error) => return refused(status::BAD_REQUEST, error.message()),
    };
    (req.after, req.before, req.baseline_after, req.baseline_before) =
        (w.after, w.before, w.baseline_after, w.baseline_before);
    req.points = w.points;

    // the queries run without `nonzero`: zeros are then no results. The echo gets it back
    let register_zero = req.options & options::NONZERO == 0;
    if req.method == Method::AnomalyRate {
        req.options |= options::ANOMALY_BIT;
    }
    let run = Run {
        env: QueryEnv { profile: env.profile, windows: env.windows, queries: env.queries, now_s: env.now_s },
        method: req.method,
        after: w.after,
        before: w.before,
        baseline_after: w.baseline_after,
        baseline_before: w.baseline_before,
        points: w.points,
        options: req.options & !options::NONZERO,
        time_group: req.time_group,
        time_group_options: req.time_group_options.clone(),
        tier: req.tier,
        shifts: w.shifts,
        register_zero,
        stats: Stats::default(),
        results: Vec::new(),
    };
    let mut walk = Walk {
        run,
        received,
        timeout_us: timeout.unsigned_abs() as u128 * 1000,
        interrupted: env.interrupted,
        progress: env.progress,
        examined: 0,
        timed_out: false,
        was_interrupted: false,
        versions: Versions::default(),
    };
    let by_value = matches!(req.method, Method::Value | Method::AnomalyRate);
    if by_value && (scope.contexts_sp.is_some() || scope.scope_contexts_sp.is_some()) {
        let timeout = i32::try_from(timeout).unwrap_or(i32::MAX);
        walk.examined = walk.run.one_query(env.hosts.clone(), &req.texts, timeout);
    } else {
        walk.hosts(&scope, &env.hosts, env.cpus);
    }
    if walk.timed_out {
        return refused(status::GATEWAY_TIMEOUT, "timed out");
    }
    if walk.was_interrupted {
        return refused(status::CLIENT_CLOSED_REQUEST, "interrupted");
    }
    let Walk { run, examined, versions, .. } = walk;
    let Run { mut results, mut stats, .. } = run;

    let raw = req.options & options::RETURN_RAW != 0;
    let normalized = !raw && req.method != Method::Value;
    if normalized && req.format != Format::Mcp {
        spread(&mut results, &mut stats);
    }
    req.group_by.group_by &= !(group_by::LABEL | group_by::SELECTED | group_by::PERCENTAGE_OF_INSTANCE);
    let limited = req.format != Format::Multinode || req.group_by.group_by == group_by::NONE;
    if req.cardinality_limit != 0 && req.format != Format::Mcp && limited {
        let limit = usize::try_from(req.cardinality_limit).unwrap_or(usize::MAX);
        select(&mut results, limit, normalized);
    }
    let duration_us = u64::try_from(received.elapsed().as_micros()).unwrap_or(u64::MAX);
    let finished = Finished { request: req, shifts: w.shifts, results, stats, examined, duration_us, versions };
    Outcome { cacheable, result: Ok(finished) }
}

#[cfg(test)]
mod tests {
    use super::super::parse::parse;
    use super::*;
    use crate::testing::{T0, W_NOW, W_POINTS, weights_host_as};

    const NEVER: &dyn Fn(&mut i32) -> bool = &|_| false;

    fn hosts() -> Vec<Arc<Host>> {
        vec![weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two")]
    }

    fn env(profile: &Profile, hosts: Vec<Arc<Host>>) -> Env<'_> {
        let windows = Windows::default();
        Env { hosts, cpus: 16, profile, windows, queries: None, now_s: W_NOW, interrupted: NEVER, progress: None }
    }

    /// A request of the route for the fixture's last minute, absolute, with `more` after it.
    fn request(version: u8, method: Method, format: Format, more: &str) -> WeightsRequest {
        let last = T0 + W_POINTS;
        let query = format!("after={}&before={last}&{more}", last - 60);
        parse(query.as_bytes(), version, method, format, 1).expect("a request")
    }

    fn finished(req: WeightsRequest) -> Finished {
        let profile = Profile::default();
        match run(req, &env(&profile, hosts())).result {
            Ok(finished) => finished,
            Err(refusal) => panic!("refused: {refusal:?}"),
        }
    }

    fn named(finished: &Finished) -> Vec<(String, String, f64)> {
        finished.results.iter().map(|t| (t.hostname.clone(), t.metric.id().to_string(), t.value)).collect()
    }

    /// The two paths of the value method: the walk looks at every metric of the hosts and sums their versions;
    /// with a contexts pattern one query does it, the hidden metric is no column, and the versions stay zero.
    /// The values are the same. By default zeros are no results (`nonzero`), and the echo keeps the option.
    #[test]
    fn a_value_request_walks_or_asks_once() {
        let walked = finished(request(2, Method::Value, Format::Multinode, ""));
        assert_eq!((walked.examined, walked.results.len()), (10, 6));
        assert!(walked.versions.contexts_hard_hash > 0);
        assert_ne!(walked.request.options & options::NONZERO, 0);
        let metrics: Vec<String> =
            named(&walked).into_iter().map(|(host, metric, _)| format!("{host}:{metric}")).collect();
        assert_eq!(metrics, ["one:a", "one:b", "one:step", "two:a", "two:b", "two:step"]);
        // raw values: no spreading for the method value
        assert_eq!((walked.results[1].value, walked.results[2].value), (5.0, 10.0));

        let asked = finished(request(2, Method::Value, Format::Multinode, "contexts=ctx.w"));
        assert_eq!((asked.examined, asked.versions), (8, Versions::default()));
        assert_eq!(named(&asked), named(&walked));
        // a pattern of no word is no pattern: the walk
        let starred = finished(request(2, Method::Value, Format::Multinode, "contexts=*"));
        assert_eq!((starred.examined, starred.versions), (10, walked.versions));
        // zeros count once an option is given without `nonzero`
        let zeros = finished(request(2, Method::Value, Format::Multinode, "options=raw"));
        assert_eq!((zeros.results.len(), zeros.request.options & options::NONZERO), (8, 0));
        // the windows are absolute in the echo, and the clock ran
        let last = T0 + W_POINTS;
        assert_eq!((walked.request.after, walked.request.before, walked.shifts), (last - 60, last, 0));
    }

    /// The anomaly-rate method adds the anomaly bit and its results are spread (the strongest is 0) unless raw;
    /// ks2 settles the baseline, the points and the shifts; a limit selects; the group-by loses what the weights
    /// do not group by.
    #[test]
    fn the_engine_settles_the_request_and_ranks_the_results() {
        let rates = finished(request(1, Method::AnomalyRate, Format::Contexts, ""));
        assert_ne!(rates.request.options & options::ANOMALY_BIT, 0);
        // the last minute is anomalous for every metric of the fixture: one distinct value, spread to 0
        assert!(!rates.results.is_empty() && rates.results.iter().all(|t| t.value == 0.0), "{:?}", named(&rates));
        // raw: the rates themselves. The window holds both of its ends, 61 points, and the last 60 are anomalous
        let raw = finished(request(1, Method::AnomalyRate, Format::Contexts, "options=raw"));
        let rate = 6000.0 / 61.0;
        assert!(raw.results.iter().all(|t| (t.value - rate).abs() < 1e-9), "{:?}", named(&raw));

        let more = "baseline_after=-240&baseline_before=0&points=20";
        let ks2 = finished(request(1, Method::Ks2, Format::Charts, more));
        let last = T0 + W_POINTS;
        // a baseline end inside the relative range counts from the highlighted window's start
        assert_eq!((ks2.request.baseline_before, ks2.request.points), (last - 60, 20));
        assert_eq!(ks2.request.baseline_after, last - 60 - (60 << ks2.shifts));
        assert!(ks2.shifts >= 1 && ks2.examined == 10, "{} {}", ks2.shifts, ks2.examined);

        // a limit: the strongest raw value is selected (`a`, the largest; the first host's, as the two hosts'
        // results are equal in everything the order looks at), with its parents
        let limited = finished(request(2, Method::Value, Format::Multinode, "limit=1"));
        let selected: Vec<_> =
            limited.results.iter().filter(|t| t.selected).map(|t| (t.hostname.as_str(), t.metric.id())).collect();
        assert_eq!(selected, [("one", "a")]);
        assert_eq!(limited.results.iter().filter(|t| t.node_selected).count(), 3);
        let unlimited = finished(request(2, Method::Value, Format::Multinode, ""));
        assert!(unlimited.results.iter().all(|t| !t.selected));
        // grouped multinode results are not selected here: their groups are ranked by the writer
        let grouped = finished(request(2, Method::Value, Format::Multinode, "limit=1&group_by=node"));
        assert!(grouped.results.iter().all(|t| !t.selected) && grouped.request.group_by.group_by != group_by::NONE);
        let stripped = finished(request(2, Method::Value, Format::Multinode, "limit=1&group_by=label"));
        assert_eq!(stripped.request.group_by.group_by, group_by::NONE);
        assert_eq!(stripped.results.iter().filter(|t| t.selected).count(), 1);
    }

    /// The refusals: C's status and text for a window that is none, a baseline that is none, too few points, a
    /// client that went away; the body has C's two spaces; an absolute window makes the reply cacheable, a
    /// relative one does not, whatever the outcome.
    #[test]
    fn the_engine_refuses_as_c_refuses() {
        let profile = Profile::default();
        let refusal = |req: WeightsRequest| {
            let outcome = run(req, &env(&profile, hosts()));
            (outcome.result.err().map(|refusal| (refusal.code, refusal.text)), outcome.cacheable)
        };
        let parsed = |method: Method, query: &str| parse(query.as_bytes(), 1, method, Format::Contexts, 1).unwrap();
        let none = format!("after={T0}&before={T0}");
        assert_eq!(refusal(parsed(Method::Value, &none)), (Some((400, "Invalid selected time-range.")), true));
        let last = T0 + W_POINTS;
        let no_baseline = format!("after={}&before={last}&baseline_after={T0}&baseline_before={T0}", last - 60);
        assert_eq!(refusal(parsed(Method::Ks2, &no_baseline)), (Some((400, "Invalid baseline time-range.")), true));
        let few = format!("after={}&before={last}&baseline_after=-120&points=10", last - 60);
        let too_few = "Too few points available, at least 15 are needed.";
        assert_eq!(refusal(parsed(Method::Volume, &few)), (Some((400, too_few)), true));
        // a relative window is not cacheable; this one is answered
        assert_eq!(refusal(parsed(Method::Value, "after=-60")), (None, false));

        let gone = |_: &mut i32| true;
        let away = Env { interrupted: &gone, ..env(&profile, hosts()) };
        let outcome = run(request(2, Method::Value, Format::Multinode, ""), &away);
        assert_eq!(outcome.result.err(), Some(Refusal { code: 499, text: "interrupted" }));
        assert_eq!(NO_RESULTS.body(), br#"{"error": "no results produced." }"#);
    }
}
