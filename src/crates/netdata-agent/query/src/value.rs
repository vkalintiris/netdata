//! One value of a chart: `rrdset2value_api_v1_with_owa()` (C: `src/web/api/formatters/rrd2json.c`) over
//! `rrd2rrdr_legacy()` (`src/web/api/queries/query.c`), the query health's lookup makes. And one value of one
//! metric: `rrdmetric2value_with_owa()` (`src/web/api/formatters/value/value.c`), the query the weights endpoints
//! make per metric.

use std::sync::Arc;

use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::contexts::{Instance, Metric};
use netdata_agent_rrd::host::Host;
use netdata_agent_storage::dbengine::RRD_STORAGE_TIERS;
pub use netdata_agent_storage::dbengine::engine::query::Priority;
use netdata_agent_storage::storage_point::StoragePoint;

use crate::execute::{Control, run_v1};
use crate::format::{exposed, rrdr2value, rrdr2value_and_anomaly_rate};
use crate::request::{DataRequest, Profile};
use crate::tables::{TimeGrouping, options};
use crate::target::{Source, create};
use crate::window::calculate;

/// The arguments of `rrdset2value_api_v1_with_owa()` that select and shape the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueRequest {
    /// The dimensions' pattern; `None` is every dimension.
    pub dimensions: Option<Vec<u8>>,
    pub points: u64,
    pub after: i64,
    pub before: i64,
    pub time_group: TimeGrouping,
    pub time_group_options: Option<Vec<u8>>,
    pub resampling_time: i64,
    pub options: u64,
    pub timeout_ms: i32,
    pub tier: u64,
    pub priority: Priority,
}

/// What C's function returns and writes through its pointers.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ValueResult {
    /// 200; 400 for a result without rows; 500 when there is no result at all.
    pub code: u16,
    /// The value, at 200 only (C leaves the caller's variable alone otherwise).
    pub value: f64,
    /// `db_after` and `db_before`: the result's view at 200, zeros at 400, not written (`None`) at 500.
    pub window: Option<(i64, i64)>,
    /// `value_is_null`: set at 400 and 500; at 200 what the row's reduction says, which leaves it unset for a
    /// result without a column.
    pub value_is_null: bool,
    /// At 200: the window was relative to the wall clock (`RRDR_RESULT_FLAG_RELATIVE`), which C writes on a
    /// caller's buffer as "not to be cached" (`rrd2json.c`); an absolute window may be cached. A badge reads it.
    pub relative: bool,
}

impl ValueResult {
    fn failed(code: u16, window: Option<(i64, i64)>) -> Self {
        ValueResult {
            code,
            value: f64::NAN,
            window,
            value_is_null: true,
            relative: false,
        }
    }
}

/// `rrdset2value_api_v1_with_owa()`: a version-1 query of `chart`, reduced to the value of its last row (its first
/// with `reversed`). A request that selects no metric is no failure: the result has the window and no column.
/// `now_s` is the wall clock; the pulse counters and the query's source come with `control`.
pub fn chart_value(
    host: &Arc<Host>,
    chart: &Arc<Chart>,
    request: &ValueRequest,
    profile: &Profile,
    control: &Control,
    now_s: i64,
) -> ValueResult {
    // C's parameter is a uint32_t
    let options = request.options & u64::from(u32::MAX);
    let mut data = DataRequest::new(1, profile);
    data.dimensions = request.dimensions.clone();
    data.points = request.points;
    data.after = request.after;
    data.before = request.before;
    data.time_group = request.time_group;
    data.time_group_options = request.time_group_options.clone();
    data.resampling_time = request.resampling_time;
    data.options = options;
    data.timeout_ms = request.timeout_ms;
    data.tier = request.tier;
    data.priority = request.priority;

    let source = Source::V1 {
        host,
        chart: Some(Arc::clone(chart)),
    };
    let mut qt = create(data, source, now_s);
    // C's `if(!r)`. The window's ends are clamped around the wall clock before the calculation, so no request
    // reaches this from here; the caller's handling of a 500 is what C's is.
    let Some(mut window) = calculate(&qt, now_s) else {
        return ValueResult::failed(500, None);
    };
    let relative = window.relative;
    let r = run_v1(&mut qt, &mut window, control);
    if r.rows == 0 {
        return ValueResult::failed(400, Some((0, 0)));
    }
    let row = if options & options::REVERSED == 0 { r.rows - 1 } else { 0 };
    let (value, value_is_null) = rrdr2value(&r, row, options);
    ValueResult {
        code: 200,
        value,
        window: Some((r.view.after, r.view.before)),
        value_is_null,
        relative,
    }
}

/// The arguments of `rrdmetric2value_with_owa()` that shape the value; the query asks for one point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricValueRequest {
    pub after: i64,
    pub before: i64,
    pub options: u64,
    pub time_group: TimeGrouping,
    pub time_group_options: Option<Vec<u8>>,
    pub tier: u64,
    pub timeout_ms: i32,
    pub priority: Priority,
}

/// `QUERY_VALUE`.
#[derive(Debug, Clone, Copy)]
pub struct QueryValue {
    /// The result's view; zeros without a row.
    pub after: i64,
    pub before: i64,
    pub value: f64,
    pub anomaly_rate: f64,
    pub points_read: usize,
    pub result_points: usize,
    /// The points the exposed metrics read, merged (`sp`).
    pub sp: StoragePoint,
    pub storage_points_per_tier: [usize; RRD_STORAGE_TIERS],
    /// From the request's arrival to the end of the execution; 0 when there is no result.
    pub duration_us: u64,
}

impl QueryValue {
    /// What C answers without a result or without a row: no value, no anomaly rate, a point of no numbers.
    pub fn without_rows(duration_us: u64) -> Self {
        QueryValue {
            after: 0,
            before: 0,
            value: f64::NAN,
            anomaly_rate: f64::NAN,
            points_read: 0,
            result_points: 0,
            sp: StoragePoint::UNSET,
            storage_points_per_tier: [0; RRD_STORAGE_TIERS],
            duration_us,
        }
    }
}

/// `rrdmetric2value_with_owa()`: a version-1 query of one metric for one point, reduced to the value and the
/// anomaly rate of its last row (its first with `reversed`), with what the query read. The request names nothing
/// but the metric, so the dimension rules alone decide whether it is queried: a hidden metric gives a result
/// without a column, which is no failure (no value, an anomaly rate of 0, nothing read).
pub fn metric_value(
    host: &Arc<Host>,
    instance: &Arc<Instance>,
    metric: &Arc<Metric>,
    request: &MetricValueRequest,
    profile: &Profile,
    control: &Control,
    now_s: i64,
) -> QueryValue {
    let options = request.options;
    let mut data = DataRequest::new(1, profile);
    data.after = request.after;
    data.before = request.before;
    data.points = 1;
    data.options = options;
    data.time_group = request.time_group;
    data.time_group_options = request.time_group_options.clone();
    data.tier = request.tier;
    data.timeout_ms = request.timeout_ms;
    data.priority = request.priority;

    let mut qt = create(data, Source::Metric { host, instance, metric }, now_s);
    // C's `if(!r)`
    let Some(mut window) = calculate(&qt, now_s) else {
        return QueryValue::without_rows(0);
    };
    let r = run_v1(&mut qt, &mut window, control);
    let duration_us = qt.executed.map_or(0, |executed| {
        u64::try_from(executed.saturating_duration_since(control.received).as_micros()).unwrap_or(u64::MAX)
    });
    if r.rows == 0 {
        return QueryValue::without_rows(duration_us);
    }
    let mut sp = StoragePoint::default();
    for qm in &qt.query {
        if exposed(qm.status, options) {
            sp.merge_to(&qm.query_points);
        }
    }
    let row = if options & options::REVERSED == 0 { r.rows - 1 } else { 0 };
    let (value, all_null, anomaly_rate) = rrdr2value_and_anomaly_rate(&r, row, options);
    // C's struct starts zeroed, and the reduction of a result without a column writes nothing
    let (value, anomaly_rate) = if all_null { (f64::NAN, f64::NAN) } else { (value, anomaly_rate.unwrap_or(0.0)) };
    QueryValue {
        after: r.view.after,
        before: r.view.before,
        value,
        anomaly_rate,
        points_read: r.db_points_read,
        result_points: r.result_points_generated,
        sp,
        storage_points_per_tier: qt.db.tiers.map(|tier| tier.points),
        duration_us,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use netdata_agent_rrd::pulse::{Queries, QuerySource};

    use super::*;
    use crate::testing::{T0, host};

    /// Health's request, with a window that is not aligned to its own length.
    fn request(after: i64, before: i64) -> ValueRequest {
        ValueRequest {
            dimensions: None,
            points: 1,
            after,
            before,
            time_group: TimeGrouping::Average,
            time_group_options: None,
            resampling_time: 0,
            options: options::SELECTED_TIER | options::NOT_ALIGNED,
            timeout_ms: 0,
            tier: 0,
            priority: Priority::Synchronous,
        }
    }

    fn value(request: &ValueRequest, queries: Option<&Queries>) -> ValueResult {
        let h = host();
        let chart = h.charts().find("t.a", false).expect("the chart");
        let control = Control {
            received: Instant::now(),
            interrupted: &|_| false,
            windows: crate::grouping::Windows::default(),
            pulse: queries.map(|queries| (queries, QuerySource::Health)),
            progress: None,
        };
        chart_value(&h, &chart, request, &Profile::default(), &control, T0 + 7)
    }

    /// A window with an end that is zero or counted back from now is relative; one with two absolute ends is not.
    /// C writes that on a caller's buffer as "not to be cached" or "may be cached"; a badge reads it.
    #[test]
    fn a_value_says_whether_its_window_is_relative() {
        for (after, before, relative) in [(-6, 0, true), (-6, -1, true), (T0 + 1, 0, true), (T0 + 1, T0 + 6, false)] {
            let got = value(&request(after, before), None);
            assert_eq!((got.code, got.relative), (200, relative), "{after} {before}");
        }
    }

    // the host holds 10, -, 20, 30, -, 40 at T0+1..=T0+6
    #[test]
    fn a_value_is_the_window_s_one_row() {
        let got = value(&request(-6, 0), None);
        assert_eq!((got.code, got.value, got.value_is_null), (200, 25.0, false));
        assert_eq!(got.window, Some((T0 + 1, T0 + 6)));

        let mut max = request(-3, 0);
        max.time_group = TimeGrouping::Max;
        let got = value(&max, None);
        assert_eq!((got.code, got.value, got.window), (200, 40.0, Some((T0 + 4, T0 + 6))));

        // an aligned window of 6 seconds ends at a multiple of 6: T0+4, and holds 10, -, 20, 30
        let mut aligned = request(-6, 0);
        aligned.options = options::SELECTED_TIER;
        let got = value(&aligned, None);
        assert_eq!((got.code, got.value, got.window), (200, 20.0, Some((T0 - 1, T0 + 4))));

        // reversed: the first row is the one. Health asks for one point, so both are the same row here: which row
        // of several C would take is not drawn
        let mut reversed = request(-6, 0);
        reversed.options |= options::REVERSED;
        assert_eq!(value(&reversed, None).value, 25.0);
    }

    #[test]
    fn a_window_without_a_collected_point_is_null() {
        let got = value(&request(-1, -1), None);
        assert_eq!((got.code, got.value_is_null), (200, true));
        assert!(got.value.is_nan());
        assert_eq!(got.window, Some((T0 + 5, T0 + 5)));
    }

    #[test]
    fn a_request_that_selects_no_metric_has_a_window_and_no_value() {
        let mut nope = request(-6, 0);
        nope.dimensions = Some(b"nope".to_vec());
        let got = value(&nope, None);
        // not null: C's reduction returns before it writes the flag
        assert_eq!((got.code, got.value_is_null), (200, false));
        assert!(got.value.is_nan());
        assert!(got.window.is_some_and(|(after, before)| after != 0 && before >= after));
    }

    fn metric_request(after: i64, before: i64) -> MetricValueRequest {
        MetricValueRequest {
            after,
            before,
            options: options::SELECTED_TIER | options::NOT_ALIGNED,
            time_group: TimeGrouping::Average,
            time_group_options: None,
            tier: 0,
            timeout_ms: 0,
            priority: Priority::SynchronousFirst,
        }
    }

    fn metric(dimension: &str, request: &MetricValueRequest, queries: Option<&Queries>) -> QueryValue {
        use crate::testing::{W_NOW, weights_host};
        let h = weights_host();
        let rc = h.contexts().get("ctx.w").expect("the fixture's context");
        let ri = rc.instances().into_iter().next().expect("its instance");
        let rm = ri.metric(dimension).expect("the metric");
        let control = Control {
            received: Instant::now(),
            interrupted: &|_| false,
            windows: crate::grouping::Windows::default(),
            pulse: queries.map(|queries| (queries, QuerySource::ApiWeights)),
            progress: None,
        };
        metric_value(&h, &ri, &rm, request, &Profile::default(), &control, W_NOW)
    }

    /// `rrdmetric2value_with_owa()`: one point over the window: the value, the anomaly rate of its points, the
    /// view, what was read and the points merged. `a` holds 1 to 240, its last 60 points anomalous.
    #[test]
    fn a_metric_s_value_comes_with_what_the_query_read() {
        use crate::testing::W_POINTS;
        let queries = Queries::default();
        let qv = metric("a", &metric_request(-W_POINTS, 0), Some(&queries));
        assert_eq!((qv.value, qv.anomaly_rate), (120.5, 25.0));
        assert_eq!((qv.after, qv.before), (T0 + 1, T0 + W_POINTS));
        let sp = qv.sp;
        assert_eq!((sp.count, sp.sum, sp.min, sp.max, sp.anomaly_count), (240, 28920.0, 1.0, 240.0, 60));
        assert_eq!(qv.result_points, 1);
        assert!(qv.points_read >= 240, "{}", qv.points_read);
        assert_eq!(qv.storage_points_per_tier[0], qv.points_read);
        assert!(qv.storage_points_per_tier[1..].iter().all(|points| *points == 0));
        let weights = queries.source(QuerySource::ApiWeights);
        assert_eq!((weights.queries, weights.points_read, weights.points_generated), (1, qv.points_read as u64, 1));

        // the last quarter alone: every point anomalous
        let last = metric("a", &metric_request(-60, 0), None);
        assert_eq!((last.value, last.anomaly_rate, last.sp.count), (210.5, 100.0, 60));
        assert_eq!((last.after, last.before), (T0 + W_POINTS - 59, T0 + W_POINTS));
        // another time grouping, and a metric that never moves
        let mut max = metric_request(-W_POINTS, 0);
        max.time_group = TimeGrouping::Max;
        assert_eq!(metric("a", &max, None).value, 240.0);
        assert_eq!(metric("b", &metric_request(-W_POINTS, 0), None).value, 5.0);
    }

    /// What is no value: a hidden metric is not queried, so the result has no column and C's reduction returns
    /// before it writes anything (no value, an anomaly rate of 0 from the zeroed struct, a point of zeros); a
    /// window before the ring's retention admits no metric, the same way; a metric at zero has a value of 0, and
    /// none (with no anomaly rate) once only nonzero dimensions are wanted.
    #[test]
    fn a_metric_without_a_value() {
        use crate::testing::W_POINTS;
        let whole = metric_request(-W_POINTS, 0);
        let unqueried = |qv: &QueryValue| {
            assert!(qv.value.is_nan(), "{qv:?}");
            assert_eq!((qv.anomaly_rate, qv.points_read, qv.result_points), (0.0, 0, 0), "{qv:?}");
            assert_eq!((qv.sp.count, qv.sp.sum, qv.sp.min, qv.sp.max), (0, 0.0, 0.0, 0.0), "{qv:?}");
        };
        let hidden = metric("hid", &whole, None);
        unqueried(&hidden);
        // the result still has its window
        assert!(hidden.after != 0 && hidden.before >= hidden.after, "{hidden:?}");
        // far in the past, outside the ring: the metric is not admitted
        unqueried(&metric("a", &metric_request(T0 - 100_000, T0 - 90_000), None));

        let zero = metric("z", &whole, None);
        assert_eq!((zero.value, zero.sp.count, zero.sp.sum), (0.0, 240, 0.0));
        let mut nonzero = whole.clone();
        nonzero.options |= options::NONZERO;
        let none = metric("z", &nonzero, None);
        assert!(none.value.is_nan() && none.anomaly_rate.is_nan(), "{none:?}");
        // not exposed, so its points are not merged either
        assert_eq!((none.sp.count, none.result_points), (0, 1));

        // C's answer without a result or without a row
        let nothing = QueryValue::without_rows(7);
        assert!(nothing.value.is_nan() && nothing.anomaly_rate.is_nan());
        assert!(nothing.sp.min.is_nan() && nothing.sp.max.is_nan() && nothing.sp.sum.is_nan());
        assert_eq!((nothing.sp.count, nothing.sp.anomaly_count, nothing.duration_us), (0, 0, 7));
    }

    #[test]
    fn the_query_counts_for_its_source() {
        let queries = Queries::default();
        value(&request(-6, 0), Some(&queries));
        let health = queries.source(QuerySource::Health);
        assert_eq!(health.queries, 1);
        assert!(health.points_read > 0 && health.points_generated > 0);
        // no metric executed, nothing counted
        let mut nope = request(-6, 0);
        nope.dimensions = Some(b"nope".to_vec());
        let queries = Queries::default();
        value(&nope, Some(&queries));
        assert_eq!(queries.source(QuerySource::Health).queries, 0);
    }
}
