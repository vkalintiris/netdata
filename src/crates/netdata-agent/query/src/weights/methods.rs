//! The queries the weights methods make per metric (`src/web/api/queries/weights.c`): the series of one metric
//! over a window (`rrd2rrdr_ks2()`, which the `ks2` method compares between its two windows) and what a query adds
//! to the request's statistics.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_log::{netdata_log_error, netdata_log_info};
use netdata_agent_rrd::contexts::{Instance, Metric};
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::pulse::{Queries, QuerySource};
use netdata_agent_rrd::stream_control::UserWeightsQuery;
use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
use netdata_agent_storage::dbengine::engine::query::Priority;
use netdata_agent_storage::storage_point::StoragePoint;
use netdata_agent_text::print::print_fixed;

use super::Method;
use super::ks2::kstwo;
use super::results::{Found, Of, Registered, flags, register};
use crate::execute::{Control, run_v1};
use crate::format::exposed;
use crate::grouping::Windows;
use crate::request::{DataRequest, Profile};
use crate::tables::{TimeGrouping, options};
use crate::target::{Source, create, metric_status};
use crate::value::{MetricValueRequest, QueryValue, metric_value};
use crate::window::calculate;

/// `WEIGHTS_STATS`: what the queries of one weights request read and made.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Stats {
    pub max_base_high_ratio: f64,
    pub db_points: usize,
    pub result_points: usize,
    pub db_queries: usize,
    pub db_points_per_tier: [usize; RRD_STORAGE_TIERS],
    pub binary_searches: usize,
}

impl Stats {
    /// `merge_query_value_to_stats()`: the `queries` behind one value.
    pub fn add_value(&mut self, qv: &QueryValue, queries: usize) {
        self.db_queries += queries;
        self.result_points += qv.result_points;
        self.db_points += qv.points_read;
        for (total, points) in self.db_points_per_tier.iter_mut().zip(qv.storage_points_per_tier) {
            *total += points;
        }
    }
}

/// What the queries of one weights request share: the storage profile, the configured limits of the two
/// exponential smoothings, the pulse counters (the queries count for the weights source) and the wall clock.
pub struct QueryEnv<'a> {
    pub profile: &'a Profile,
    pub windows: Windows,
    pub queries: Option<&'a Queries>,
    pub now_s: i64,
}

/// Nothing interrupts a query per metric (C gives these targets no interrupt callback).
static NEVER_INTERRUPTED: fn(&mut i32) -> bool = |_| false;

impl QueryEnv<'_> {
    /// The control of one query per metric: its clock starts now (C sets `received_ut` when it creates the
    /// target), nothing interrupts it and it reports no progress.
    pub fn control(&self) -> Control<'_> {
        Control {
            received: Instant::now(),
            interrupted: &NEVER_INTERRUPTED,
            windows: self.windows,
            pulse: self.queries.map(|queries| (queries, QuerySource::ApiWeights)),
            progress: None,
        }
    }
}

/// The arguments of `rrd2rrdr_ks2()` that shape the series.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesRequest {
    pub after: i64,
    pub before: i64,
    pub points: u64,
    pub options: u64,
    pub time_group: TimeGrouping,
    pub time_group_options: Option<Vec<u8>>,
    pub tier: u64,
}

/// `rrd2rrdr_ks2()`: the rows of one metric over the window, oldest first, with the points the query read. The
/// query counts as a weights query while it runs, and for the statistics whenever it has a result; then nothing
/// is returned for a metric that is not queried (no error), hidden, without a collected point in the window,
/// zero all along, or with fewer than two rows.
pub fn metric_series(
    env: &QueryEnv,
    host: &Arc<Host>,
    instance: &Arc<Instance>,
    metric: &Arc<Metric>,
    request: &SeriesRequest,
    stats: &mut Stats,
) -> Option<(Vec<f64>, StoragePoint)> {
    let mut data = DataRequest::new(1, env.profile);
    data.after = request.after;
    data.before = request.before;
    data.points = request.points;
    data.options = request.options;
    data.time_group = request.time_group;
    data.time_group_options = request.time_group_options.clone();
    data.tier = request.tier;
    data.priority = Priority::SynchronousFirst;

    let mut qt = create(data, Source::Metric { host, instance, metric }, env.now_s);
    let r = {
        let _running = UserWeightsQuery::start();
        // C's `if(!r)`: no window, no result and nothing counted
        let mut window = calculate(&qt, env.now_s)?;
        run_v1(&mut qt, &mut window, &env.control())
    };
    stats.db_queries += 1;
    stats.result_points += r.result_points_generated;
    stats.db_points += r.db_points_read;
    for (total, tier) in stats.db_points_per_tier.iter_mut().zip(&qt.db.tiers) {
        *total += tier.points;
    }

    if r.columns == 0 || qt.query.is_empty() {
        return None;
    }
    if r.columns != 1 || qt.query.len() != 1 {
        netdata_log_error!(
            "WEIGHTS: on query '{}' expected 1 dimension in RRDR but got {} r->d and {} qt->query.used",
            qt.id,
            r.columns,
            qt.query.len()
        );
        return None;
    }
    let od = r.od[0];
    if od & metric_status::HIDDEN != 0 || od & metric_status::QUERIED == 0 || od & metric_status::NONZERO == 0 {
        return None;
    }
    if r.rows < 2 {
        return None;
    }
    // one column: the cells are the rows. An empty one is already zero
    Some((r.v[..r.rows].to_vec(), qt.query[0].query_points))
}

/// One weights request at work on its metrics (`struct query_weights_data` and its request, as
/// `weights_for_rrdmetric()` reads them): what the prelude settled, the statistics and the results so far.
pub struct Run<'a> {
    pub env: QueryEnv<'a>,
    pub method: Method,
    /// The highlighted window and, for `ks2` and `volume`, the baseline it is compared with.
    pub after: i64,
    pub before: i64,
    pub baseline_after: i64,
    pub baseline_before: i64,
    /// The points asked of the highlighted window (`ks2`).
    pub points: u64,
    /// The request's options, without `nonzero`.
    pub options: u64,
    pub time_group: TimeGrouping,
    pub time_group_options: Option<Vec<u8>>,
    pub tier: u64,
    /// The baseline is asked for the highlighted window's rows times two to this power (`ks2`).
    pub shifts: u32,
    /// Zeros are results too (the request had no `nonzero`).
    pub register_zero: bool,
    pub stats: Stats,
    pub results: Vec<Registered>,
}

impl Run<'_> {
    /// The request's method on one metric (the switch of `weights_for_rrdmetric()`).
    pub fn metric(&mut self, of: &Of) {
        match self.method {
            Method::Value | Method::AnomalyRate => self.value(of),
            Method::Volume => self.volume(of),
            Method::Ks2 => self.ks2(of),
        }
    }

    /// One value of the metric over a window, as the methods ask for it: no timeout, the weights' priority.
    fn query_value(
        &self,
        of: &Of,
        (after, before): (i64, i64),
        options: u64,
        time_group: TimeGrouping,
        time_group_options: Option<&[u8]>,
    ) -> QueryValue {
        let request = MetricValueRequest {
            after,
            before,
            options,
            time_group,
            time_group_options: time_group_options.map(<[u8]>::to_vec),
            tier: self.tier,
            timeout_ms: 0,
            priority: Priority::SynchronousFirst,
        };
        let (env, control) = (&self.env, self.env.control());
        metric_value(of.host, of.instance, of.metric, &request, env.profile, &control, env.now_s)
    }

    /// `rrdset_weights_value()`, the methods `value` and `anomaly-rate`: the metric's value over the highlighted
    /// window, when it is a number. The query counts whether it is or not.
    fn value(&mut self, of: &Of) {
        let options = self.options | options::MATCH_IDS | options::NATURAL_POINTS;
        let group = self.time_group_options.as_deref();
        let qv = self.query_value(of, (self.after, self.before), options, self.time_group, group);
        self.stats.add_value(&qv, 1);
        if qv.value.is_finite() {
            let (highlighted, duration_us) = (Some(qv.sp), qv.duration_us);
            let found = Found { value: qv.value, flags: 0, highlighted, baseline: None, duration_us };
            register(&mut self.results, &mut self.stats, self.register_zero, of, found);
        }
    }

    /// `rrdset_metric_correlations_volume()`: how far the highlighted window's average is from the baseline's, as
    /// a ratio of the baseline's, times the share of the highlighted window spent on that side of the baseline's
    /// average; that share alone when the baseline's average is zero (or the baseline has no data).
    fn volume(&mut self, of: &Of) {
        let options = self.options | options::MATCH_IDS | options::ABSOLUTE | options::NATURAL_POINTS;
        let group = self.time_group_options.as_deref();
        let mut baseline =
            self.query_value(of, (self.baseline_after, self.baseline_before), options, self.time_group, group);
        self.stats.add_value(&baseline, 1);
        if !baseline.value.is_finite() {
            // no data in the baseline window: the highlighted one may have some
            baseline.value = 0.0;
        }
        let highlight = self.query_value(of, (self.after, self.before), options, self.time_group, group);
        self.stats.add_value(&highlight, 1);
        if !highlight.value.is_finite() || baseline.value == highlight.value {
            return;
        }
        // on anomaly bits, only a rise of the anomaly rate counts
        if options & options::ANOMALY_BIT != 0 && highlight.value < baseline.value {
            return;
        }
        // "%s%0.7f" into C's buffer, which holds 49 bytes of it
        let mut condition = vec![if highlight.value < baseline.value { b'<' } else { b'>' }];
        print_fixed(&mut condition, baseline.value, 7);
        condition.truncate(49);
        let countif =
            self.query_value(of, (self.after, self.before), options, TimeGrouping::Countif, Some(&condition));
        self.stats.add_value(&countif, 1);
        if !countif.value.is_finite() {
            netdata_log_info!("WEIGHTS: highlighted countif query failed, but highlighted average worked - strange...");
            return;
        }
        // countif gives 0 to 100
        let share = countif.value / 100.0;
        // C's `isgreater(b, 0.0) || isless(b, 0.0)`: the baseline's average is a number here
        let (flags, value) = if baseline.value != 0.0 {
            (flags::BASE_HIGH_RATIO, (highlight.value - baseline.value) / baseline.value * share)
        } else {
            (flags::PERCENTAGE_OF_TIME, share)
        };
        let found = Found {
            value,
            flags,
            highlighted: Some(highlight.sp),
            baseline: Some(baseline.sp),
            duration_us: baseline.duration_us + highlight.duration_us + countif.duration_us,
        };
        register(&mut self.results, &mut self.stats, self.register_zero, of, found);
    }

    /// `rrdset_metric_correlations_ks2()`: the two-sample Kolmogorov-Smirnov test of the metric's changes in the
    /// baseline against its changes in the highlighted window; the result is one minus the probability that both
    /// come from one distribution. The baseline is asked for the highlighted window's rows times two to the
    /// power of `shifts`, and only when the highlighted window gave a series.
    fn ks2(&mut self, of: &Of) {
        let options = self.options | options::NATURAL_POINTS;
        let started = Instant::now();
        let request = |after, before, points| SeriesRequest {
            after,
            before,
            points,
            options,
            time_group: self.time_group,
            time_group_options: self.time_group_options.clone(),
            tier: self.tier,
        };
        let highlighted = request(self.after, self.before, self.points);
        let Some((highlight, highlighted_sp)) =
            metric_series(&self.env, of.host, of.instance, of.metric, &highlighted, &mut self.stats)
        else {
            return;
        };
        let base_points = (highlight.len() as u64).checked_shl(self.shifts).unwrap_or(0);
        let baseline = request(self.baseline_after, self.baseline_before, base_points);
        let Some((baseline, baseline_sp)) =
            metric_series(&self.env, of.host, of.instance, of.metric, &baseline, &mut self.stats)
        else {
            return;
        };
        let mut prob = kstwo(&baseline, &highlight, self.shifts);
        if !prob.is_finite() {
            return;
        }
        // C: "these conditions should never happen, but still let's check"
        if prob < 0.0 {
            netdata_log_error!("Metric correlations: kstwo() returned a negative number: {prob:.6}");
            prob = -prob;
        }
        if prob > 1.0 {
            netdata_log_error!("Metric correlations: kstwo() returned a number above 1.0: {prob:.6}");
            prob = 1.0;
        }
        let duration_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
        // 0 is the least correlated and 1 the most, so the probability is flipped
        let found = Found {
            value: 1.0 - prob,
            flags: flags::BASE_HIGH_RATIO,
            highlighted: Some(highlighted_sp),
            baseline: Some(baseline_sp),
            duration_us,
        };
        register(&mut self.results, &mut self.stats, self.register_zero, of, found);
    }
}

/// The request's eleven selector texts, as given (`qwr`): the one-query path hands them to a query target.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Texts {
    pub scope_nodes: Option<Vec<u8>>,
    pub scope_contexts: Option<Vec<u8>>,
    pub scope_instances: Option<Vec<u8>>,
    pub scope_labels: Option<Vec<u8>>,
    pub scope_dimensions: Option<Vec<u8>>,
    pub nodes: Option<Vec<u8>>,
    pub contexts: Option<Vec<u8>>,
    pub instances: Option<Vec<u8>>,
    pub dimensions: Option<Vec<u8>>,
    pub labels: Option<Vec<u8>>,
    pub alerts: Option<Vec<u8>>,
}

impl Run<'_> {
    /// `rrdset_weights_multi_dimensional_value()`, the path of the methods `value` and `anomaly-rate` when the
    /// request names contexts: one version-1 query of one point over every host, with the request's texts and
    /// its timeout, counted as a weights query while it runs. Each column of its one row is a metric looked at;
    /// an exposed column whose cell is a number is a result, with the points and the duration of its metric.
    /// Returns how many metrics were looked at. Nothing is done unless the result has exactly one row and a
    /// column for every query metric; a query cut short by its timeout is not told apart.
    pub fn one_query(&mut self, hosts: Vec<Arc<Host>>, texts: &Texts, timeout_ms: i32) -> usize {
        self.one_query_since(Instant::now(), hosts, texts, timeout_ms)
    }

    /// [`Self::one_query`] of a request whose query was received at `received`: C takes the query's time first
    /// in `query_target_create()`, so building the target over every host counts against the timeout.
    fn one_query_since(
        &mut self,
        received: Instant,
        hosts: Vec<Arc<Host>>,
        texts: &Texts,
        timeout_ms: i32,
    ) -> usize {
        let mut data = DataRequest::new(1, self.env.profile);
        data.scope_nodes = texts.scope_nodes.clone();
        data.scope_contexts = texts.scope_contexts.clone();
        data.scope_instances = texts.scope_instances.clone();
        data.scope_labels = texts.scope_labels.clone();
        data.scope_dimensions = texts.scope_dimensions.clone();
        data.nodes = texts.nodes.clone();
        data.contexts = texts.contexts.clone();
        data.instances = texts.instances.clone();
        data.dimensions = texts.dimensions.clone();
        data.labels = texts.labels.clone();
        data.alerts = texts.alerts.clone();
        data.after = self.after;
        data.before = self.before;
        data.points = 1;
        data.options = self.options | options::NATURAL_POINTS;
        data.time_group = self.time_group;
        data.time_group_options = self.time_group_options.clone();
        data.tier = self.tier;
        data.timeout_ms = timeout_ms;
        data.priority = Priority::SynchronousFirst;

        let mut qt = create(data, Source::V2 { hosts }, self.env.now_s);
        let r = {
            let _running = UserWeightsQuery::start();
            let Some(mut window) = calculate(&qt, self.env.now_s) else {
                return 0;
            };
            run_v1(&mut qt, &mut window, &Control { received, ..self.env.control() })
        };
        if r.rows != 1 || r.columns == 0 || r.columns != qt.query.len() {
            return 0;
        }
        let (mut examined, mut queries) = (0, 0);
        // a host's name is taken when the columns reach it
        let mut named: Option<(usize, String)> = None;
        for (d, qm) in qt.query.iter().enumerate() {
            examined += 1;
            if !exposed(r.od[d], self.options) {
                continue;
            }
            // one row: the cell is the column's
            let value = r.v[d];
            if value.is_finite() {
                let qd = &qt.dimensions[qm.dimension];
                let qi = &qt.instances[qd.instance];
                let qc = &qt.contexts[qi.context];
                let host = &qt.nodes[qc.node].host;
                let hostname = match &named {
                    Some((node, hostname)) if *node == qc.node => hostname.clone(),
                    _ => {
                        let hostname = host.hostname();
                        named = Some((qc.node, hostname.clone()));
                        hostname
                    }
                };
                let of = Of { host, hostname: &hostname, context: &qc.rc, instance: &qi.ri, metric: &qd.rm };
                let found = Found {
                    value,
                    flags: 0,
                    highlighted: Some(qm.query_points),
                    baseline: None,
                    duration_us: qm.duration_ut,
                };
                register(&mut self.results, &mut self.stats, self.register_zero, &of, found);
            }
            queries += 1;
        }
        // C's value of this path has no points per tier: they are not added
        self.stats.db_queries += queries;
        self.stats.result_points += r.result_points_generated;
        self.stats.db_points += r.db_points_read;
        examined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rrdr::Rrdr;
    use crate::target::QueryTarget;
    use crate::testing::{T0, W_NOW, W_POINTS, weights_host};

    fn request(points: u64) -> SeriesRequest {
        SeriesRequest {
            after: -W_POINTS,
            before: 0,
            points,
            options: options::SELECTED_TIER | options::NOT_ALIGNED,
            time_group: TimeGrouping::Average,
            time_group_options: None,
            tier: 0,
        }
    }

    fn env<'a>(profile: &'a Profile, queries: Option<&'a Queries>) -> QueryEnv<'a> {
        QueryEnv { profile, windows: Windows::default(), queries, now_s: W_NOW }
    }

    type Series = Option<(Vec<f64>, StoragePoint)>;

    fn series(dimension: &str, request: &SeriesRequest, queries: Option<&Queries>) -> (Series, Stats) {
        let h = weights_host();
        let rc = h.contexts().get("ctx.w").expect("the fixture's context");
        let ri = rc.instances().into_iter().next().expect("its instance");
        let rm = ri.metric(dimension).expect("the metric");
        let profile = Profile::default();
        let mut stats = Stats::default();
        let got = metric_series(&env(&profile, queries), &h, &ri, &rm, request, &mut stats);
        (got, stats)
    }

    /// The fixture chart's own version-1 query of one dimension: what the engine gives for the same window.
    fn chart_query(dimension: &str, request: &SeriesRequest) -> (Rrdr, QueryTarget) {
        let h = weights_host();
        let chart = h.charts().find("t.w", false).expect("the chart");
        let mut data = DataRequest::new(1, &Profile::default());
        data.dimensions = Some(dimension.as_bytes().to_vec());
        (data.after, data.before, data.points, data.options) =
            (request.after, request.before, request.points, request.options);
        let mut qt = create(data, Source::V1 { host: &h, chart: Some(chart) }, W_NOW);
        let mut window = calculate(&qt, W_NOW).expect("a window");
        let r = run_v1(&mut qt, &mut window, &env(&Profile::default(), None).control());
        (r, qt)
    }

    /// What a result adds to the statistics, as `rrd2rrdr_ks2()` adds it.
    fn counted(r: &Rrdr, qt: &QueryTarget) -> Stats {
        Stats {
            db_points: r.db_points_read,
            result_points: r.result_points_generated,
            db_queries: 1,
            db_points_per_tier: qt.db.tiers.map(|tier| tier.points),
            ..Stats::default()
        }
    }

    /// `rrd2rrdr_ks2()`: the metric's rows, oldest first, and the points it read: the rows and the points of the
    /// chart's own query of that one dimension. The query counts once, for the statistics and for its source.
    #[test]
    fn a_metric_s_series_is_its_rows_oldest_first() {
        let queries = Queries::default();
        let (got, stats) = series("a", &request(24), Some(&queries));
        let (values, sp) = got.expect("a series");
        // `a` rises by one a second from 1: ten seconds a row, each the mean of its ten points (to the engine's
        // rounding: the exact rows are compared with the chart's own query below)
        assert_eq!(values.len(), 24);
        for (row, value) in values.iter().enumerate() {
            let mean = row as f64 * 10.0 + 5.5;
            assert!((value - mean).abs() < 1e-9, "row {row}: {value} against {mean}");
        }
        assert_eq!((sp.count, sp.sum, sp.min, sp.max, sp.anomaly_count), (240, 28920.0, 1.0, 240.0, 60));

        let (r, qt) = chart_query("a", &request(24));
        assert_eq!((r.columns, r.rows), (1, 24));
        assert_eq!(values, r.v);
        let read = qt.query[0].query_points;
        assert_eq!((sp.count, sp.sum, sp.anomaly_count), (read.count, read.sum, read.anomaly_count));
        assert_eq!(stats, counted(&r, &qt));
        assert!(stats.db_points >= 240 && stats.result_points == 24, "{stats:?}");
        let weights = queries.source(QuerySource::ApiWeights);
        assert_eq!(
            (weights.queries, weights.points_read, weights.points_generated),
            (1, stats.db_points as u64, stats.result_points as u64)
        );
    }

    /// The refusals, each after the query counted: a hidden metric is not queried at all; one that is zero all
    /// along; a window before the ring's retention (nothing to query); fewer than two rows. And when every dimension
    /// is wanted (the percentage option), the hidden metric is queried and refused as hidden.
    #[test]
    fn a_series_is_refused_after_it_counted() {
        let (got, stats) = series("hid", &request(24), None);
        assert!(got.is_none());
        assert_eq!((stats.db_queries, stats.result_points, stats.db_points), (1, 0, 0));

        let (got, stats) = series("z", &request(24), None);
        assert!(got.is_none());
        let (r, qt) = chart_query("z", &request(24));
        assert_eq!((r.columns, r.rows), (1, 24));
        assert_eq!(stats, counted(&r, &qt));
        assert!(stats.db_points >= 240, "{stats:?}");

        // far in the past: outside the ring, so the metric is not admitted
        let before_the_data = SeriesRequest { after: T0 - 100_000, before: T0 - 90_000, ..request(24) };
        let (got, stats) = series("a", &before_the_data, None);
        assert!(got.is_none());
        assert_eq!((stats.db_queries, stats.db_points), (1, 0));

        let (got, stats) = series("a", &request(1), None);
        assert!(got.is_none());
        assert_eq!((stats.db_queries, stats.result_points), (1, 1));
        assert!(stats.db_points >= 240, "{stats:?}");
        let two = series("a", &request(2), None).0.expect("two rows are a series");
        assert!(two.0.len() == 2 && (two.0[0] - 60.5).abs() < 1e-9 && (two.0[1] - 180.5).abs() < 1e-9, "{:?}", two.0);

        let all = SeriesRequest { options: request(24).options | options::PERCENTAGE, ..request(24) };
        let (got, stats) = series("hid", &all, None);
        assert!(got.is_none());
        assert_eq!(stats.db_queries, 1);
        assert!(stats.db_points >= 240, "{stats:?}");
    }

    /// The two options the request's parser always adds: without `unaligned` the windows would move to a
    /// multiple of their length.
    const BASE: u64 = options::NOT_ALIGNED | options::NULL2ZERO;

    /// A run of `method` over the fixture: the highlighted window is the last `high` seconds, the baseline the
    /// `base` seconds before it; both absolute, as the engine's prelude leaves them.
    fn run(profile: &Profile, method: Method, high: i64, base: i64) -> Run<'_> {
        let last = T0 + W_POINTS;
        Run {
            env: env(profile, None),
            method,
            after: last - high,
            before: last,
            baseline_after: last - high - base,
            baseline_before: last - high,
            points: 30,
            options: BASE,
            time_group: TimeGrouping::Average,
            time_group_options: None,
            tier: 0,
            shifts: 0,
            register_zero: true,
            stats: Stats::default(),
            results: Vec::new(),
        }
    }

    /// The method of `run` on each of `dimensions` of the fixture, in order: the run as the method leaves it.
    fn on<'a>(mut run: Run<'a>, dimensions: &[&str]) -> Run<'a> {
        let h = weights_host();
        let rc = h.contexts().get("ctx.w").expect("the fixture's context");
        let ri = rc.instances().into_iter().next().expect("its instance");
        for dimension in dimensions {
            let rm = ri.metric(dimension).expect("the metric");
            run.metric(&Of { host: &h, hostname: "weights", context: &rc, instance: &ri, metric: &rm });
        }
        run
    }

    type Group<'a> = (TimeGrouping, Option<&'a [u8]>);

    /// The value the port's own query of one metric gives for a window, with the options a method adds.
    fn reference(dimension: &str, window: (i64, i64), options: u64, group: Group) -> QueryValue {
        let profile = Profile::default();
        let probe = run(&profile, Method::Value, 1, 1);
        let h = weights_host();
        let rc = h.contexts().get("ctx.w").expect("the fixture's context");
        let ri = rc.instances().into_iter().next().expect("its instance");
        let rm = ri.metric(dimension).expect("the metric");
        let of = Of { host: &h, hostname: "weights", context: &rc, instance: &ri, metric: &rm };
        probe.query_value(&of, window, options, group.0, group.1)
    }

    fn values(run: &Run) -> Vec<(String, f64, u32)> {
        run.results.iter().map(|t| (t.metric.id().to_string(), t.value, t.flags)).collect()
    }

    /// `rrdset_weights_value()`: the value of the highlighted window with the two options the method adds, kept
    /// when it is a number; every query counts, the hidden metric's too; a zero is kept only when zeros count;
    /// `anomaly-rate` is the same method (the engine adds the anomaly bit to the options).
    #[test]
    fn the_value_method_registers_the_highlighted_window_s_value() {
        let profile = Profile::default();
        let window = (T0 + W_POINTS - 60, T0 + W_POINTS);
        let added = BASE | options::MATCH_IDS | options::NATURAL_POINTS;
        let average: Group = (TimeGrouping::Average, None);
        let a = reference("a", window, added, average);
        assert!(a.value > 200.0 && a.value < 220.0, "{a:?}");

        let got = on(run(&profile, Method::Value, 60, 60), &["a", "b", "z", "hid"]);
        let kept = [("a".to_string(), a.value, 0), ("b".to_string(), 5.0, 0), ("z".to_string(), 0.0, 0)];
        assert_eq!(values(&got), kept);
        assert_eq!(got.stats.db_queries, 4);
        let first = &got.results[0];
        assert_eq!((first.highlighted, first.baseline), (a.sp, StoragePoint::default()));
        assert_eq!((first.hostname.as_str(), first.instance.id(), first.context.id()), ("weights", "t.w", "ctx.w"));
        // the statistics are the sums of what each value's query read
        let (b, z) = (reference("b", window, added, average), reference("z", window, added, average));
        let mut summed = Stats::default();
        for qv in [&a, &b, &z, &reference("hid", window, added, average)] {
            summed.add_value(qv, 1);
        }
        assert_eq!(got.stats, summed);
        assert!(got.stats.db_points > 0 && got.stats.result_points == 3, "{:?}", got.stats);

        let mut nonzero = run(&profile, Method::Value, 60, 60);
        nonzero.register_zero = false;
        let got = on(nonzero, &["z", "b"]);
        assert_eq!((values(&got), got.stats.db_queries), (vec![("b".to_string(), 5.0, 0)], 2));

        // the anomaly rate: the same method over the anomaly bit; the last 60 points are anomalous
        let mut rate = run(&profile, Method::AnomalyRate, 30, 60);
        rate.options = BASE | options::ANOMALY_BIT;
        assert_eq!(values(&on(rate, &["a"])), [("a".to_string(), 100.0, 0)]);
        let mut calm = run(&profile, Method::AnomalyRate, 30, 60);
        (calm.after, calm.before, calm.options) = (T0 + 10, T0 + 40, BASE | options::ANOMALY_BIT);
        assert_eq!(values(&on(calm, &["a"])), [("a".to_string(), 0.0, 0)]);
    }

    /// `rrdset_metric_correlations_volume()`: three queries (the baseline's average, the highlighted window's,
    /// the share of the highlighted window on its side of the baseline's average), each counted; the ratio and
    /// its flag; the share alone when the baseline's average is zero or the baseline has no data; nothing when
    /// the averages are equal, when the highlighted window has no value, or when an anomaly rate fell.
    #[test]
    fn the_volume_method_weighs_the_change_by_its_share_of_the_time() {
        let profile = Profile::default();
        let added = BASE | options::MATCH_IDS | options::ABSOLUTE | options::NATURAL_POINTS;
        let average: Group = (TimeGrouping::Average, None);
        let last = T0 + W_POINTS;
        let (high, base) = ((last - 60, last), (last - 180, last - 60));

        // `a` rises: the whole highlighted window is above the baseline's average
        let (b, h) = (reference("a", base, added, average), reference("a", high, added, average));
        let condition = format!(">{:.7}", b.value);
        let c = reference("a", high, added, (TimeGrouping::Countif, Some(condition.as_bytes())));
        assert!(b.value > 100.0 && h.value > b.value && c.value == 100.0, "{b:?} {h:?} {c:?}");
        let got = on(run(&profile, Method::Volume, 60, 120), &["a"]);
        let ratio = (h.value - b.value) / b.value * (c.value / 100.0);
        assert_eq!(values(&got), [("a".to_string(), ratio, flags::BASE_HIGH_RATIO)]);
        assert_eq!((got.results[0].highlighted, got.results[0].baseline), (h.sp, b.sp));
        assert_eq!((got.stats.db_queries, got.stats.max_base_high_ratio), (3, ratio));
        let mut summed = Stats::default();
        for qv in [&b, &h, &c] {
            summed.add_value(qv, 1);
        }
        assert_eq!((got.stats.db_points, got.stats.result_points), (summed.db_points, summed.result_points));

        // a fall: the windows swapped. The value keeps no sign; the share is of the time below
        let mut fall = run(&profile, Method::Volume, 60, 120);
        (fall.after, fall.before, fall.baseline_after, fall.baseline_before) = (base.0, base.1, high.0, high.1);
        let below = format!("<{:.7}", h.value);
        let share = reference("a", base, added, (TimeGrouping::Countif, Some(below.as_bytes()))).value / 100.0;
        let fallen = (b.value - h.value) / h.value * share;
        assert!(fallen < 0.0 && share == 1.0, "{fallen} {share}");
        assert_eq!(values(&on(fall, &["a"])), [("a".to_string(), -fallen, flags::BASE_HIGH_RATIO)]);

        // `step`: zero all through a baseline in the fixture's first half, 10 all through the highlighted window:
        // the share alone
        let mut early = run(&profile, Method::Volume, 60, 60);
        (early.baseline_after, early.baseline_before) = (T0 + 30, T0 + 90);
        let got = on(early, &["step", "b", "z", "hid"]);
        assert_eq!(values(&got), [("step".to_string(), 1.0, flags::PERCENTAGE_OF_TIME)]);
        // step: three queries; b and z: equal averages after two; hid: no highlighted value after two
        assert_eq!((got.stats.db_queries, got.stats.max_base_high_ratio), (9, 0.0));

        // no data in the baseline window (it is outside the ring): its average counts as zero
        let mut no_baseline = run(&profile, Method::Volume, 60, 60);
        (no_baseline.baseline_after, no_baseline.baseline_before) = (T0 - 100_000, T0 - 90_000);
        assert_eq!(values(&on(no_baseline, &["b"])), [("b".to_string(), 1.0, flags::PERCENTAGE_OF_TIME)]);

        // on anomaly bits a fall is nothing: `a` is anomalous in its last 60 points alone
        let mut calmer = run(&profile, Method::Volume, 60, 60);
        (calmer.after, calmer.before) = (T0 + 10, T0 + 40);
        (calmer.baseline_after, calmer.baseline_before) = (last - 30, last);
        calmer.options = BASE | options::ANOMALY_BIT;
        let got = on(calmer, &["a"]);
        assert_eq!((values(&got), got.stats.db_queries), (vec![], 2));
        let mut wilder = run(&profile, Method::Volume, 30, 60);
        (wilder.baseline_after, wilder.baseline_before) = (T0 + 10, T0 + 40);
        wilder.options = BASE | options::ANOMALY_BIT;
        assert_eq!(values(&on(wilder, &["a"])), [("a".to_string(), 1.0, flags::PERCENTAGE_OF_TIME)]);
    }

    /// `rrdset_metric_correlations_ks2()`: the highlighted window's series, then the baseline's with as many
    /// points as the highlighted window gave rows, times two to the power of the shifts; the result is one minus
    /// the test's probability, with both windows' points. A metric without a highlighted series costs one query
    /// and the baseline is not asked; one without a baseline series costs two.
    #[test]
    fn the_ks2_method_tests_the_two_windows_changes() {
        let profile = Profile::default();
        let last = T0 + W_POINTS;
        for shifts in [0, 1] {
            let mut asked = run(&profile, Method::Ks2, 60, 120);
            asked.shifts = shifts;
            let got = on(asked, &["a", "step", "b", "z", "hid"]);

            // the same two series, asked for here
            let h = weights_host();
            let rc = h.contexts().get("ctx.w").expect("the fixture's context");
            let ri = rc.instances().into_iter().next().expect("its instance");
            let mut stats = Stats::default();
            let mut series = |dimension: &str, (after, before): (i64, i64), points: u64| {
                let rm = ri.metric(dimension).expect("the metric");
                let options = BASE | options::NATURAL_POINTS;
                let request = SeriesRequest { after, before, points, options, ..request(1) };
                metric_series(&env(&profile, None), &h, &ri, &rm, &request, &mut stats)
            };
            let mut expected = Vec::new();
            for dimension in ["a", "step", "b"] {
                let (high, high_sp) = series(dimension, (last - 60, last), 30).expect("a highlighted series");
                let base_points = (high.len() as u64) << shifts;
                let (base, base_sp) = series(dimension, (last - 180, last - 60), base_points).expect("a baseline series");
                let prob = kstwo(&base, &high, shifts);
                assert!((0.0..=1.0).contains(&prob), "{dimension} {prob}");
                expected.push((dimension.to_string(), 1.0 - prob, flags::BASE_HIGH_RATIO, high_sp, base_sp));
            }
            let registered: Vec<_> = got
                .results
                .iter()
                .map(|t| (t.metric.id().to_string(), t.value, t.flags, t.highlighted, t.baseline))
                .collect();
            assert_eq!(registered, expected, "shifts {shifts}");
            // `b` never changes in either window: both are one distribution
            assert_eq!(expected[2].1, 0.0);
            // `a` rises by one a second. Without a shift a point of the baseline is twice as long as one of the
            // highlighted window, so its changes are twice as large and the two distributions share nothing;
            // with one shift the points are as long and the changes are the same
            let rising = expected[0].1;
            assert!(if shifts == 0 { rising > 0.99 } else { rising < 0.01 }, "shifts {shifts}: {rising}");
            // a, step and b: two queries each; z (zero all along) and hid (not queried): the highlighted one alone
            assert_eq!(got.stats.db_queries, 8, "shifts {shifts}");
            let largest = expected.iter().map(|e| e.1).fold(0.0, f64::max);
            assert_eq!(got.stats.max_base_high_ratio, largest);
        }

        // no baseline series (the baseline is outside the ring): two queries, no result
        let mut no_baseline = run(&profile, Method::Ks2, 60, 120);
        (no_baseline.baseline_after, no_baseline.baseline_before) = (T0 - 100_000, T0 - 90_000);
        let got = on(no_baseline, &["b"]);
        assert_eq!((got.results.len(), got.stats.db_queries), (0, 2));
        // zeros do not count: `b`'s zero is dropped
        let mut nonzero = run(&profile, Method::Ks2, 60, 120);
        nonzero.register_zero = false;
        assert!(on(nonzero, &["b"]).results.is_empty());
    }

    /// `rrdset_weights_multi_dimensional_value()`: one query over the hosts gives the values the method gives
    /// metric by metric, in the same order, for the metrics that are columns: the hidden one is none, so it is
    /// not looked at here. Every exposed column counts as a query; the points per tier are not added; a zero is
    /// a result only when zeros count; the texts select as they select for a data query.
    #[test]
    fn one_query_gives_the_values_of_the_metrics_it_selects() {
        use crate::testing::weights_host_as;
        let profile = Profile::default();
        let hosts = vec![weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two")];
        let named = |run: &Run| -> Vec<(String, String, f64)> {
            run.results.iter().map(|t| (t.hostname.clone(), t.metric.id().to_string(), t.value)).collect()
        };
        let texts = |set: &dyn Fn(&mut Texts)| {
            let mut texts = Texts { contexts: Some(b"ctx.w".to_vec()), ..Texts::default() };
            set(&mut texts);
            texts
        };
        let asked = |texts: &Texts, register_zero: bool| {
            let mut run = run(&profile, Method::Value, 60, 60);
            run.register_zero = register_zero;
            let examined = run.one_query(hosts.clone(), texts, 0);
            (run, examined)
        };

        // the method, metric by metric, on one such host
        let by_metric = on(run(&profile, Method::Value, 60, 60), &["a", "b", "z", "step"]);
        let of_host = |hostname: &str| -> Vec<(String, String, f64)> {
            by_metric.results.iter().map(|t| (hostname.to_string(), t.metric.id().to_string(), t.value)).collect()
        };
        let both: Vec<_> = of_host("one").into_iter().chain(of_host("two")).collect();
        assert_eq!(both.len(), 8);

        let (got, examined) = asked(&texts(&|_| {}), true);
        assert_eq!((named(&got), examined), (both.clone(), 8));
        assert_eq!(got.stats.db_queries, 8);
        assert!(got.stats.db_points > 0 && got.stats.result_points == 8, "{:?}", got.stats);
        assert_eq!(got.stats.db_points_per_tier, [0; RRD_STORAGE_TIERS]);
        let first = &got.results[0];
        let read = by_metric.results[0].highlighted;
        assert_eq!((first.flags, first.baseline, first.highlighted), (0, StoragePoint::default(), read));
        assert!(Arc::ptr_eq(&first.host, &hosts[0]) && first.instance.id() == "t.w" && first.context.id() == "ctx.w");

        // zeros do not count: `z` is still a column and a query, and no result
        let (got, examined) = asked(&texts(&|_| {}), false);
        let without_z: Vec<_> = both.iter().filter(|(_, metric, _)| metric != "z").cloned().collect();
        assert_eq!((named(&got), examined, got.stats.db_queries), (without_z, 8, 8));

        // the texts: a host, a dimension; a context that is not there gives no column and nothing is done
        let (got, examined) = asked(&texts(&|t| t.nodes = Some(b"two".to_vec())), true);
        assert_eq!((named(&got), examined), (of_host("two"), 4));
        let (got, examined) = asked(&texts(&|t| t.dimensions = Some(b"b".to_vec())), true);
        assert_eq!((named(&got).len(), examined, got.stats.db_queries), (2, 2, 2));
        let (got, examined) = asked(&texts(&|t| t.contexts = Some(b"nope".to_vec())), true);
        assert_eq!((got.results.len(), examined, got.stats), (0, 0, Stats::default()));
    }

    /// The one query's time counts from before its target is built: a request already past its timeout when
    /// the query starts is cut at the query's first check, which comes after its first metric: at most that one
    /// is a result.
    #[test]
    fn one_query_is_cut_by_a_timeout_that_ran_out_before_it() {
        use crate::testing::weights_host_as;
        let profile = Profile::default();
        let hosts = vec![weights_host_as("guid-1", "one")];
        let texts = Texts { contexts: Some(b"ctx.w".to_vec()), ..Texts::default() };
        let asked = |received: Instant| {
            let mut run = run(&profile, Method::Value, 60, 60);
            run.register_zero = true;
            run.one_query_since(received, hosts.clone(), &texts, 1000);
            (run.results.len(), run.stats.db_queries)
        };
        assert_eq!(asked(Instant::now()), (4, 4));
        let long_ago = Instant::now().checked_sub(std::time::Duration::from_secs(10)).expect("an uptime of 10 s");
        let (results, queries) = asked(long_ago);
        assert!(results <= 1 && queries <= 1, "{results} results of {queries} queries");
    }

    /// The one query looks at every column and counts a query for each exposed one. A hidden metric is a column
    /// only when every dimension is wanted (`percentage`): it is then looked at, not exposed, and no result;
    /// with `raw` every queried column is exposed, the hidden one too.
    #[test]
    fn the_one_query_counts_a_column_that_is_not_exposed() {
        use crate::testing::weights_host_as;
        let profile = Profile::default();
        let hosts = vec![weights_host_as("guid-1", "one"), weights_host_as("guid-2", "two")];
        let texts = Texts { contexts: Some(b"ctx.w".to_vec()), ..Texts::default() };
        let asked = |more: u64| {
            let mut run = run(&profile, Method::Value, 60, 60);
            run.register_zero = true;
            run.options |= more;
            let examined = run.one_query(hosts.clone(), &texts, 0);
            let hidden = run.results.iter().filter(|t| t.metric.id() == "hid").count();
            (examined, run.stats.db_queries, hidden)
        };
        assert_eq!(asked(0), (8, 8, 0));
        assert_eq!(asked(options::PERCENTAGE), (10, 8, 0));
        assert_eq!(asked(options::PERCENTAGE | options::RETURN_RAW), (10, 10, 2));
    }

    /// A value whose every cell is empty is no value: a window inside the ring and before the data gives a
    /// query and no result, also when zeros count and with `null2zero`, which every request carries.
    #[test]
    fn a_window_without_a_point_gives_no_value() {
        let profile = Profile::default();
        let mut early = run(&profile, Method::Value, 60, 60);
        (early.after, early.before) = (T0 - 1000, T0 - 900);
        early.register_zero = true;
        assert_ne!(early.options & options::NULL2ZERO, 0);
        let got = on(early, &["a"]);
        assert_eq!((got.results.len(), got.stats.db_queries), (0, 1));
    }

    /// `merge_query_value_to_stats()`.
    #[test]
    fn a_value_adds_to_the_statistics() {
        let mut per_tier = [0; RRD_STORAGE_TIERS];
        per_tier[0] = 7;
        let qv = QueryValue {
            points_read: 7,
            result_points: 1,
            storage_points_per_tier: per_tier,
            ..QueryValue::without_rows(0)
        };
        let mut stats = Stats::default();
        stats.add_value(&qv, 1);
        stats.add_value(&qv, 2);
        per_tier[0] = 14;
        let twice =
            Stats { db_points: 14, result_points: 2, db_queries: 3, db_points_per_tier: per_tier, ..Stats::default() };
        assert_eq!(stats, twice);
    }
}
