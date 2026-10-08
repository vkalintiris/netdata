//! The numbers of the weights endpoints (`src/web/api/queries/weights.c`): how much each metric of the hosts in
//! scope stands out in a highlighted window. For now the statistics of the `ks2` method (the two-sample
//! Kolmogorov-Smirnov test and the distribution it ends in), the four methods on one metric, their results, the
//! walk over the hosts, contexts and metrics of a request, the request as its handler reads it, the engine that
//! takes it to its results, the two formats of version 1 and the two of the later versions.

pub mod engine;
pub mod ks;
pub mod ks2;
pub mod methods;
pub mod parse;
pub mod results;
pub mod v1;
pub mod v2;
pub mod walk;

use netdata_agent_text::c::double_to_i64;
use netdata_agent_text::json::API_RELATIVE_TIME_MAX;
use netdata_agent_text::time_window::relative_window_to_absolute_query;

/// `WEIGHTS_METHOD`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Ks2,
    Volume,
    AnomalyRate,
    Value,
}

/// `weights_methods[]`.
const METHOD_NAMES: [(&str, Method); 4] = [
    ("ks2", Method::Ks2),
    ("volume", Method::Volume),
    ("anomaly-rate", Method::AnomalyRate),
    ("value", Method::Value),
];

impl Method {
    /// `weights_string_to_method()`: a name that is none of the four is `ks2`.
    pub fn parse(name: &[u8]) -> Self {
        METHOD_NAMES.iter().find(|(known, _)| known.as_bytes() == name).map_or(Method::Ks2, |(_, method)| *method)
    }

    /// `weights_method_to_string()`.
    pub fn name(self) -> &'static str {
        METHOD_NAMES.iter().find(|(_, method)| *method == self).map_or("ks2", |(name, _)| *name)
    }
}
/// `web_api_v12_weights()`: a request without a timeout gets five minutes, and none gets less than a second.
pub fn timeout_ms(given: i64) -> i64 {
    match given {
        0 => 5 * 60 * 1000,
        ms if ms < 1000 => 1000,
        ms => ms,
    }
}

/// Why the prelude refuses a request; each is a 400 with C's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowError {
    Highlight,
    Baseline,
    TooFewPoints,
}

impl WindowError {
    pub fn message(self) -> &'static str {
        match self {
            WindowError::Highlight => "Invalid selected time-range.",
            WindowError::Baseline => "Invalid baseline time-range.",
            WindowError::TooFewPoints => "Too few points available, at least 15 are needed.",
        }
    }
}

/// What the prelude makes of a request's windows and points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestWindows {
    pub after: i64,
    pub before: i64,
    /// Both ends of the highlighted window were absolute: the answer may be cached.
    pub absolute: bool,
    pub baseline_after: i64,
    pub baseline_before: i64,
    pub points: u64,
    /// The baseline holds the highlighted window's points times two to this power.
    pub shifts: u32,
}

/// The windows of a weights request (`web_api_v12_weights()`): the highlighted window made absolute, which must
/// not be empty. For `ks2` and `volume` also: 500 points when none are asked; a baseline end inside the relative
/// range counts from the highlighted window's start; the baseline made absolute, which must not be empty; the
/// baseline's length as a multiple of the highlighted window's, rounded up to a power of two (in 32 bits, as C
/// computes it), gives the shifts; the shifts and then the points give way until the baseline's points fit; 15
/// points at the least; and the baseline is cut to exactly that multiple, ending where it ended. The other
/// methods keep the request's baseline and points as given. `wall_clock_s` is `now_realtime_sec()`.
pub fn windows(
    method: Method,
    (after, before): (i64, i64),
    (baseline_after, baseline_before): (i64, i64),
    points: u64,
    wall_clock_s: i64,
) -> Result<RequestWindows, WindowError> {
    let (after, before, absolute) = relative_window_to_absolute_query(after, before, wall_clock_s);
    if before <= after {
        return Err(WindowError::Highlight);
    }
    let mut w = RequestWindows { after, before, absolute, baseline_after, baseline_before, points, shifts: 0 };
    if !matches!(method, Method::Ks2 | Method::Volume) {
        return Ok(w);
    }
    if w.points == 0 {
        w.points = 500;
    }
    if w.baseline_before <= API_RELATIVE_TIME_MAX {
        w.baseline_before = w.baseline_before.wrapping_add(after);
    }
    (w.baseline_after, w.baseline_before, _) =
        relative_window_to_absolute_query(w.baseline_after, w.baseline_before, wall_clock_s);
    if w.baseline_before <= w.baseline_after {
        return Err(WindowError::Baseline);
    }
    let base_delta = w.baseline_before - w.baseline_after;
    let high_delta = before - after;
    // `(uint32_t)round(...)` on x86-64: the low 32 bits of the 64-bit conversion
    let mut multiplier = double_to_i64((base_delta as f64 / high_delta as f64).round()) as u32;
    if multiplier & multiplier.wrapping_sub(1) != 0 {
        // not a power of two: the next one up
        multiplier = multiplier.wrapping_sub(1);
        for shift in [1, 2, 4, 8, 16] {
            multiplier |= multiplier >> shift;
        }
        multiplier = multiplier.wrapping_add(1);
    }
    while multiplier > 1 {
        w.shifts += 1;
        multiplier >>= 1;
    }
    let exceed =
        |points: u64, shifts: u32| ks2::points_exceed_max(usize::try_from(points).unwrap_or(usize::MAX), shifts);
    while w.shifts != 0 && exceed(w.points, w.shifts) {
        w.shifts -= 1;
    }
    while exceed(w.points, w.shifts) {
        w.points >>= 1;
    }
    if w.points < 15 {
        return Err(WindowError::TooFewPoints);
    }
    w.baseline_after = w.baseline_before - (high_delta << w.shifts);
    Ok(w)
}

#[cfg(test)]
mod vector;

#[cfg(test)]
mod tests {
    use super::Method;

    /// The engine's timeout: five minutes when none is given, a second at the least.
    #[test]
    fn a_request_s_timeout_has_a_default_and_a_floor() {
        use super::timeout_ms;
        for (given, used) in [(0, 300_000), (-5, 1000), (1, 1000), (999, 1000), (1000, 1000), (5000, 5000)] {
            assert_eq!(timeout_ms(given), used, "{given}");
        }
    }

    /// The windows of a request. T is a wall clock; C's "now" for a relative end is the second before it.
    #[test]
    fn the_prelude_settles_the_windows_the_points_and_the_shifts() {
        use super::{RequestWindows, WindowError, windows};
        const T: i64 = 1_700_000_000;
        // absolute windows: a highlighted window of 100 s, a baseline of `times` that before it
        let absolute = |method: Method, times: i64, points: u64| {
            windows(method, (T - 100, T), (T - 100 - 100 * times, T - 100), points, T + 10)
        };
        let settled = |times: i64, points: u64| {
            let w = absolute(Method::Ks2, times, points).expect("windows");
            assert_eq!((w.after, w.before, w.absolute, w.baseline_before), (T - 100, T, true, T - 100), "{times}");
            // the baseline is cut to the multiple, ending where it ended
            assert_eq!(w.baseline_after, T - 100 - (100 << w.shifts), "{times}");
            (w.shifts, w.points)
        };
        // the multiple, rounded up to a power of two: 1, 2, 3 (4), 5 (8), 1000 (1024, which the limit trims)
        assert_eq!(settled(1, 500), (0, 500));
        assert_eq!(settled(2, 500), (1, 500));
        assert_eq!(settled(3, 500), (2, 500));
        assert_eq!(settled(5, 500), (3, 500));
        // 10000 points at the most in the baseline: the shifts give way first (500 << 4 is the most), ...
        assert_eq!(settled(1000, 500), (4, 500));
        assert_eq!(settled(1000, 0), (4, 500));
        // ... then the points are halved
        assert_eq!(settled(1, 30_000), (0, 7500));
        assert_eq!(settled(4, 30_000), (0, 7500));
        assert_eq!(settled(4, 5000), (1, 5000));
        // volume has the same prelude; the value methods keep what the request gave
        assert_eq!(absolute(Method::Volume, 3, 0).map(|w| (w.shifts, w.points)), Ok((2, 500)));
        for method in [Method::Value, Method::AnomalyRate] {
            let kept = RequestWindows {
                after: T - 100,
                before: T,
                absolute: true,
                baseline_after: T - 400,
                baseline_before: T - 100,
                points: 0,
                shifts: 0,
            };
            assert_eq!(absolute(method, 3, 0), Ok(kept), "{method:?}");
            // not even a baseline that is no window is looked at
            assert!(windows(method, (T - 100, T), (T, T), 0, T + 10).is_ok());
        }

        // the three refusals, with C's texts
        let refused = |highlighted: (i64, i64), baseline: (i64, i64), points: u64| {
            windows(Method::Ks2, highlighted, baseline, points, T + 10).map_err(WindowError::message)
        };
        assert_eq!(refused((T, T), (T - 200, T - 100), 0), Err("Invalid selected time-range."));
        assert_eq!(refused((T - 100, T), (T - 100, T - 100), 0), Err("Invalid baseline time-range."));
        let too_few = Err("Too few points available, at least 15 are needed.");
        assert_eq!(refused((T - 100, T), (T - 200, T - 100), 14), too_few);
        assert!(refused((T - 100, T), (T - 200, T - 100), 15).is_ok());
        // flipped ends are swapped, not refused
        assert!(refused((T, T - 100), (T - 100, T - 200), 0).is_ok());
        // volume needs its 15 points too
        let volume = windows(Method::Volume, (T - 100, T), (T - 200, T - 100), 14, T + 10);
        assert_eq!(volume, Err(WindowError::TooFewPoints));

        // the multiple is rounded, not cut: a baseline of 250 s against 100 s is 2.5 times, rounded to 3, so 4
        // times; 150 s is 1.5 times, rounded to 2; 40 s rounds to 0 times, which stays one time
        let rounded = |baseline: i64| {
            let w = windows(Method::Ks2, (T - 100, T), (T - 100 - baseline, T - 100), 0, T + 10).expect("windows");
            (w.shifts, w.baseline_after)
        };
        assert_eq!(rounded(250), (2, T - 500));
        assert_eq!(rounded(150), (1, T - 300));
        assert_eq!(rounded(40), (0, T - 200));
        // a baseline end counts from the highlighted window's start when it is not above the relative range,
        // however far below it is
        let far = windows(Method::Ks2, (T - 100, T), (-2400, -94_608_001), 0, T + 10).expect("windows");
        assert_eq!(far.baseline_before, T - 100 - 94_608_001);

        // relative ends: the highlighted window counts back from the second before the wall clock, and a
        // baseline end inside the relative range counts from the highlighted window's start
        let relative = windows(Method::Ks2, (-100, 0), (-400, 0), 0, T + 1).expect("windows");
        assert_eq!((relative.after, relative.before, relative.absolute), (T - 99, T, false));
        assert_eq!((relative.baseline_before, relative.shifts), (T - 99, 2));
        assert_eq!(relative.baseline_after, T - 99 - (99 << 2));
    }

    /// C's table of the methods' names: the name is exact, and anything else is `ks2`.
    #[test]
    fn a_method_is_known_by_c_s_name() {
        let named = [("ks2", Method::Ks2), ("volume", Method::Volume), ("anomaly-rate", Method::AnomalyRate)];
        for (name, method) in named.into_iter().chain([("value", Method::Value)]) {
            assert_eq!((Method::parse(name.as_bytes()), method.name()), (method, name));
        }
        for name in ["", "KS2", "anomaly_rate", "value ", "nope"] {
            assert_eq!(Method::parse(name.as_bytes()), Method::Ks2, "{name:?}");
        }
    }
}
