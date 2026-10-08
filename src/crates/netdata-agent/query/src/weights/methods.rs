//! The queries the weights methods make per metric (`src/web/api/queries/weights.c`): the series of one metric
//! over a window (`rrd2rrdr_ks2()`, which the `ks2` and `volume` methods compare) and what a query adds to the
//! request's statistics.

use std::sync::Arc;
use std::time::Instant;

use netdata_agent_log::netdata_log_error;
use netdata_agent_rrd::contexts::{Instance, Metric};
use netdata_agent_rrd::host::Host;
use netdata_agent_rrd::pulse::{Queries, QuerySource};
use netdata_agent_rrd::stream_control::UserWeightsQuery;
use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
use netdata_agent_storage::dbengine::engine::query::Priority;
use netdata_agent_storage::storage_point::StoragePoint;

use crate::execute::{Control, run_v1};
use crate::grouping::Windows;
use crate::request::{DataRequest, Profile};
use crate::tables::TimeGrouping;
use crate::target::{Source, create, metric_status};
use crate::value::QueryValue;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rrdr::Rrdr;
    use crate::tables::options;
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
