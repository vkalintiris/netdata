//! Query windows, ported from `rrdr_relative_window_to_absolute()` and `rrdr_relative_window_to_absolute_query()`
//! in `src/libnetdata/libnetdata.c`: `after`/`before` values within three years of zero are relative to now.
//!
//! C computes in a 128-bit "wide" time and clamps back to `time_t`; `i128` is that wide time.

use crate::json::API_RELATIVE_TIME_MAX;

/// `rrdr_relative_window_value_is_relative()`.
pub fn is_relative(value: i64) -> bool {
    (-API_RELATIVE_TIME_MAX..=API_RELATIVE_TIME_MAX).contains(&value)
}

/// `rrdr_time_wide_to_time_t()`.
fn clamp(value: i128) -> i64 {
    value.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64
}

/// `rrdr_relative_window_to_absolute()`: `(after, before, relative)` where `relative` says either end was relative.
/// A relative `before` counts back from `now`; a relative `after` from `before` (0 means -600), plus one second so
/// `after=-5` gives five points; flipped ends swap; a window in the future shifts back to end at `now`.
pub fn relative_window_to_absolute(after: i64, before: i64, now: i64) -> (i64, i64, bool) {
    let now_w = i128::from(now);
    let mut before_w = i128::from(before);
    let mut after_w = i128::from(after);
    let mut relative = false;
    if is_relative(before) {
        if before_w > 0 {
            before_w = -before_w;
        }
        before_w += now_w;
        relative = true;
    }
    if is_relative(after) {
        if after_w > 0 {
            after_w = -after_w;
        }
        if after_w == 0 {
            after_w = -600;
        }
        after_w = before_w + after_w + 1;
        relative = true;
    }
    if after_w > before_w {
        std::mem::swap(&mut after_w, &mut before_w);
    }
    if before_w > now_w {
        let delta = before_w - now_w;
        before_w -= delta;
        after_w -= delta;
    }
    (clamp(after_w), clamp(before_w), relative)
}

/// `rrdr_relative_window_to_absolute_query()` with `unittest = false`: `now` is the wall clock minus one second, and
/// both ends are kept within ten years before and one year after it. `(after, before, absolute)` where `absolute`
/// says both ends were absolute.
pub fn relative_window_to_absolute_query(
    after: i64,
    before: i64,
    wall_clock_s: i64,
) -> (i64, i64, bool) {
    let now = clamp(i128::from(wall_clock_s) - 1);
    let (after, before, relative) = relative_window_to_absolute(after, before, now);
    let minimum = clamp(i128::from(now) - 10 * 365 * 86400);
    let maximum = clamp(i128::from(now) + 365 * 86400);
    (
        after.clamp(minimum, maximum),
        before.clamp(minimum, maximum),
        !relative,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_match_c() {
        let now = 1_700_000_000;
        let cases = [
            // relative after from now
            ((-60, 0), (now - 59, now, true)),
            // a positive relative before flips to negative
            ((0, 10), (now - 10 - 599, now - 10, true)),
            // absolute and flipped
            ((now, now - 100), (now - 100, now, false)),
            // absolute in the future shifts back
            ((now + 100, now + 200), (now - 100, now, false)),
        ];
        for ((after, before), expected) in cases {
            assert_eq!(
                relative_window_to_absolute(after, before, now),
                expected,
                "{after} {before}"
            );
        }
        // The query form: now is one second behind the wall clock, and far ends are clamped.
        assert_eq!(
            relative_window_to_absolute_query(-60, -1, now),
            (now - 1 - 1 - 59, now - 2, false)
        );
        assert_eq!(
            relative_window_to_absolute_query(i64::MIN, now - 10, now).0,
            now - 1 - 10 * 365 * 86400
        );
    }
}
