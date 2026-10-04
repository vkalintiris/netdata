//! One value of a chart: `rrdset2value_api_v1_with_owa()` (C: `src/web/api/formatters/rrd2json.c`) over
//! `rrd2rrdr_legacy()` (`src/web/api/queries/query.c`), the query health's lookup makes.

use std::sync::Arc;

use netdata_agent_rrd::chart::Chart;
use netdata_agent_rrd::host::Host;
pub use netdata_agent_storage::dbengine::engine::query::Priority;

use crate::execute::{Control, run_v1};
use crate::format::rrdr2value;
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
}

impl ValueResult {
    fn failed(code: u16, window: Option<(i64, i64)>) -> Self {
        ValueResult {
            code,
            value: f64::NAN,
            window,
            value_is_null: true,
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
