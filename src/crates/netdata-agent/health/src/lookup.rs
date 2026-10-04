//! An alert's database lookup (`health_event_loop.c`, the first walk of `health_event_loop_for_host()`): the query
//! it asks for and what the alert makes of the answer.

use netdata_agent_query::tables::{TimeGrouping, options};
use netdata_agent_query::value::{Priority, ValueRequest, ValueResult};
use netdata_agent_text::print::print_fixed;

use crate::alert::{Run, run_flags};
use crate::prototype::AlertConfig;

/// C's buffer for the group options holds 100 bytes: 99 and the terminator.
const GROUP_OPTIONS_MAX: usize = 99;

/// The options text of the lookup's time grouping: the rule's value as `%f` for a percentile, a trimmed mean or a
/// trimmed median; the condition and the value for `countif`; none for every other grouping.
pub fn group_options(config: &AlertConfig) -> Option<Vec<u8>> {
    let mut text = Vec::with_capacity(16);
    match config.time_group.unwrap_or(TimeGrouping::Average) {
        TimeGrouping::Percentile | TimeGrouping::TrimmedMean | TimeGrouping::TrimmedMedian => {}
        TimeGrouping::Countif => text.extend_from_slice(config.time_group_condition.name().as_bytes()),
        _ => return None,
    }
    print_fixed(&mut text, config.time_group_value, 6);
    text.truncate(GROUP_OPTIONS_MAX);
    Some(text)
}

/// The call of `rrdset2value_api_v1_with_owa()` for an alert: one point over the rule's window, on the tier the
/// query selects, prepared on the health thread.
pub fn request(config: &AlertConfig) -> ValueRequest {
    ValueRequest {
        // C passes the rule's dimensions as a text: an empty one when the rule names none
        dimensions: Some(config.dimensions.clone().unwrap_or_default()),
        points: 1,
        after: i64::from(config.after),
        before: i64::from(config.before),
        time_group: config.time_group.unwrap_or(TimeGrouping::Average),
        time_group_options: group_options(config),
        resampling_time: 0,
        options: config.options | options::SELECTED_TIER,
        timeout_ms: 0,
        tier: 0,
        priority: Priority::Synchronous,
    }
}

/// What the alert makes of the lookup's answer. Its value and window are written as C's function writes them
/// through its pointers: nothing when there is no result, a zeroed window for a result without rows. Anything but
/// a 200 is a database error, and a null value is marked; either leaves the alert without a value.
pub fn apply(run: &mut Run, result: &ValueResult) {
    if let Some((after, before)) = result.window {
        run.db_after = after;
        run.db_before = before;
    }
    if result.code == 200 {
        run.value = result.value;
        run.run_flags &= !run_flags::DB_ERROR;
    } else {
        run.value = f64::NAN;
        run.run_flags |= run_flags::DB_ERROR;
    }
    if result.value_is_null {
        run.value = f64::NAN;
        run.run_flags |= run_flags::DB_NAN;
    } else {
        run.run_flags &= !run_flags::DB_NAN;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::Status;
    use crate::testing::health_with;

    fn config(lookup: &str) -> AlertConfig {
        let health = health_with(&format!("template: l\n on: t.ctx\n lookup: {lookup}\n every: 1s\n\n"));
        let prototypes = health.prototypes();
        crate::alert::copy_config(&prototypes.get(b"l").unwrap().rules()[0].config).0
    }

    // C's own arguments for eleven lookups are in `tests/vectors/loop.tsv` (scenario `lookup`); these are the shapes
    #[test]
    fn the_group_options_follow_the_grouping() {
        for (lookup, expected) in [
            ("average -10s", None),
            ("percentile95 -1m", Some("95.000000")),
            ("percentile(90) -5m", Some("90.000000")),
            ("trimmed-mean5 -5m", Some("5.000000")),
            ("trimmed-median -5m", Some("5.000000")),
            ("countif(>0.5) -10m", Some(">0.500000")),
            ("countif(!=0.5) -10m", Some("!=0.500000")),
        ] {
            let options = group_options(&config(lookup));
            assert_eq!(options.as_deref(), expected.map(str::as_bytes), "{lookup}");
        }
    }

    #[test]
    fn the_request_is_one_point_on_the_selected_tier() {
        let request = request(&config("sum -2m at -30s unaligned absolute of a,b"));
        assert_eq!((request.points, request.after, request.before), (1, -120, -30));
        assert_eq!(request.dimensions.as_deref(), Some(&b"a,b"[..]));
        assert_eq!(request.options, options::SELECTED_TIER | options::NOT_ALIGNED | options::ABSOLUTE);
        assert_eq!((request.timeout_ms, request.tier, request.priority), (0, 0, Priority::Synchronous));
    }

    fn run() -> Run {
        Run {
            next_event_id: 1,
            value: 7.0,
            old_value: 7.0,
            status: Status::Clear,
            old_status: Status::Uninitialized,
            run_flags: run_flags::RUNNABLE,
            last_status_change: 0,
            last_status_change_value: 0.0,
            last_updated: 0,
            next_update: 0,
            db_after: 11,
            db_before: 21,
            delay_up_to_timestamp: 0,
            delay_up_current: 0,
            delay_down_current: 0,
            delay_last: 0,
            last_repeat: 0,
            times_repeat: 0,
            labels_version: 0,
        }
    }

    #[test]
    fn an_answer_sets_the_value_the_window_and_the_two_flags() {
        let answer = |code, value, window, value_is_null| ValueResult { code, value, window, value_is_null };
        let both = run_flags::DB_ERROR | run_flags::DB_NAN;
        let cases = [
            // a value
            (answer(200, 5.0, Some((1, 2)), false), Some(5.0), (1, 2), 0),
            // no result: the window stays
            (answer(500, f64::NAN, None, true), None, (11, 21), both),
            // no rows: the window is zeroed
            (answer(400, f64::NAN, Some((0, 0)), true), None, (0, 0), both),
            // rows without a column: no value, and no flag
            (answer(200, f64::NAN, Some((1, 2)), false), None, (1, 2), 0),
            // an empty row, whatever its value says
            (answer(200, 0.0, Some((1, 2)), true), None, (1, 2), run_flags::DB_NAN),
        ];
        for (result, value, window, flags) in cases {
            let mut run = run();
            run.run_flags |= both;
            apply(&mut run, &result);
            assert_eq!((run.db_after, run.db_before), window, "{result:?}");
            assert_eq!(run.run_flags, run_flags::RUNNABLE | flags, "{result:?}");
            match value {
                Some(value) => assert_eq!(run.value, value, "{result:?}"),
                None => assert!(run.value.is_nan(), "{result:?}"),
            }
        }
    }
}
