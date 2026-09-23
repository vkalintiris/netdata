//! Durations, mirroring `libnetdata/parsers/duration.c`.
//!
//! Units are matched case-insensitively (`strcasecmp()`), so `M` resolves to
//! minutes (`m` comes first in the table) and the months compatibility entry
//! `M` is unreachable, exactly as in C. An empty unit means nanoseconds.

use crate::c::{self, at, eq_ignore_case, is_alpha, skip_spaces};
use crate::parse::str2ndd;

const NSEC_PER_USEC: i64 = 1_000;
const NSEC_PER_MS: i64 = 1_000 * NSEC_PER_USEC;
const NSEC_PER_SEC: i64 = 1_000_000_000;
const NSEC_PER_MIN: i64 = NSEC_PER_SEC * 60;
const NSEC_PER_HOUR: i64 = NSEC_PER_MIN * 60;
const NSEC_PER_DAY: i64 = NSEC_PER_HOUR * 24;
const NSEC_PER_WEEK: i64 = NSEC_PER_DAY * 7;
const NSEC_PER_MONTH: i64 = NSEC_PER_DAY * 30;
const NSEC_PER_QUARTER: i64 = NSEC_PER_MONTH * 3;
const NSEC_PER_YEAR: i64 = NSEC_PER_DAY * 365;

/// One row of the C `units[]` table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationUnit {
    pub name: &'static str,
    /// Used when formatting (the first formatter of each multiplier).
    pub formatter: bool,
    pub nanoseconds: i64,
}

const fn unit(name: &'static str, formatter: bool, nanoseconds: i64) -> DurationUnit {
    DurationUnit {
        name,
        formatter,
        nanoseconds,
    }
}

/// The C `units[]` table; the order (smallest to biggest) is significant.
pub const DURATION_UNITS: &[DurationUnit] = &[
    unit("ns", true, 1),
    unit("nanosecond", false, 1),
    unit("nanoseconds", false, 1),
    unit("us", true, NSEC_PER_USEC),
    unit("microsecond", false, NSEC_PER_USEC),
    unit("microseconds", false, NSEC_PER_USEC),
    unit("ms", true, NSEC_PER_MS),
    unit("millisecond", false, NSEC_PER_MS),
    unit("milliseconds", false, NSEC_PER_MS),
    unit("s", true, NSEC_PER_SEC),
    unit("sec", false, NSEC_PER_SEC),
    unit("secs", false, NSEC_PER_SEC),
    unit("second", false, NSEC_PER_SEC),
    unit("seconds", false, NSEC_PER_SEC),
    unit("m", true, NSEC_PER_MIN),
    unit("min", false, NSEC_PER_MIN),
    unit("minute", false, NSEC_PER_MIN),
    unit("minutes", false, NSEC_PER_MIN),
    unit("h", true, NSEC_PER_HOUR),
    unit("hr", false, NSEC_PER_HOUR),
    unit("hrs", false, NSEC_PER_HOUR),
    unit("hour", false, NSEC_PER_HOUR),
    unit("hours", false, NSEC_PER_HOUR),
    unit("d", true, NSEC_PER_DAY),
    unit("day", false, NSEC_PER_DAY),
    unit("days", false, NSEC_PER_DAY),
    unit("w", false, NSEC_PER_WEEK),
    unit("wk", false, NSEC_PER_WEEK),
    unit("week", false, NSEC_PER_WEEK),
    unit("weeks", false, NSEC_PER_WEEK),
    unit("mo", true, NSEC_PER_MONTH),
    unit("M", false, NSEC_PER_MONTH),
    unit("month", false, NSEC_PER_MONTH),
    unit("months", false, NSEC_PER_MONTH),
    unit("q", false, NSEC_PER_QUARTER),
    unit("quarter", false, NSEC_PER_QUARTER),
    unit("quarters", false, NSEC_PER_QUARTER),
    unit("y", true, NSEC_PER_YEAR),
    unit("Y", false, NSEC_PER_YEAR),
    unit("a", false, NSEC_PER_YEAR),
    unit("year", false, NSEC_PER_YEAR),
    unit("years", false, NSEC_PER_YEAR),
];

/// `duration_find_unit()`: index of the first case-insensitive match (the
/// unit is a C string: it ends at a NUL byte).
fn find_unit(unit: &[u8]) -> Option<usize> {
    let unit = c::c_str(unit);
    let unit = if unit.is_empty() {
        b"ns".as_slice()
    } else {
        unit
    };
    DURATION_UNITS
        .iter()
        .position(|u| eq_ignore_case(unit, u.name.as_bytes()))
}

/// `parser_round_number_to_int64()`: `round()` then a range check.
pub(crate) fn round_to_i64(value: f64) -> Option<i64> {
    let value = value.round();
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    (-LIMIT..LIMIT).contains(&value).then_some(value as i64)
}

/// `parser_round_number_to_uint64()`: `round()` then a range check.
pub(crate) fn round_to_u64(value: f64) -> Option<u64> {
    let value = value.round();
    const LIMIT: f64 = 18_446_744_073_709_551_616.0;
    (0.0..LIMIT).contains(&value).then_some(value as u64)
}

/// `duration_round_to_resolution()`: `(value ± (resolution - 1) / 2) / resolution`,
/// the nearest multiple, with exact halves of an even resolution rounded toward
/// zero (`(15, 10)` is 1, `(-15, 10)` is -1, `(5, 10)` is 0).
///
/// The C code overflows (undefined behaviour) when `value` is within
/// `resolution / 2` of the `i64` limits; this port wraps there.
///
/// # Panics
///
/// When `resolution` is 0 (C raises `SIGFPE`).
pub fn duration_round_to_resolution(value: i64, resolution: i64) -> i64 {
    let half = resolution.wrapping_sub(1) / 2;
    if value > 0 {
        value.wrapping_add(half).wrapping_div(resolution)
    } else if value < 0 {
        value.wrapping_sub(half).wrapping_div(resolution)
    } else {
        0
    }
}

/// `duration_parse()`: parses `duration` (e.g. `1d 12h`, `-5m`, `7 days ago`,
/// `never`, `off`) into an integer count of `output_unit`, using
/// `default_unit` for bare numbers. Returns `None` where C returns `false`.
pub fn duration_parse(duration: &[u8], default_unit: &str, output_unit: &str) -> Option<i64> {
    let s = c::c_str(duration);
    if s.is_empty() {
        return None;
    }
    let du_def = find_unit(default_unit.as_bytes())?;
    let du_out = find_unit(output_unit.as_bytes())?;

    let mut i = skip_spaces(s, 0);
    let mut sign: i64 = 1;
    if at(s, i) == b'-' {
        i += 1;
        sign = -1;
    }

    let mut v: i64 = 0;
    let mut found_ago = false;
    let mut parsed_any_duration = false;

    while i < s.len() {
        i = skip_spaces(s, i);
        if i >= s.len() {
            break;
        }

        let rest = &s[i..];
        if matches!(rest[0], b'n' | b'N') && eq_ignore_case(rest, b"never") {
            return Some(0);
        }
        if matches!(rest[0], b'o' | b'O') && eq_ignore_case(rest, b"off") {
            return Some(0);
        }

        let (value, used) = str2ndd(rest);
        if used == 0 {
            if eq_ignore_case(rest, b"ago") {
                found_ago = true;
                i += 3;
                break;
            }
            return None;
        }
        i = skip_spaces(s, i + used);

        let unit_start = i;
        while is_alpha(at(s, i)) {
            i += 1;
        }
        let mut unit_len = i - unit_start;

        let mut du = None;
        if unit_len == 0 {
            du = Some(du_def);
        } else {
            if unit_len >= 3 && s[i - 3..i].eq_ignore_ascii_case(b"ago") {
                // a unit glued to "ago", like "daysago"
                unit_len -= 3;
                if unit_len > 0 && unit_len < 16 {
                    du = find_unit(&s[unit_start..unit_start + unit_len]);
                    if du.is_some() {
                        found_ago = true;
                        i -= 3;
                    }
                }
            }
            if du.is_none() {
                // the C unit buffer holds 15 characters
                let unit = &s[unit_start..unit_start + (i - unit_start).min(15)];
                if eq_ignore_case(unit, b"ago") {
                    found_ago = true;
                    break;
                }
                du = Some(find_unit(unit)?);
            }
        }

        let multiplier = DURATION_UNITS[du?].nanoseconds;
        let nanoseconds = round_to_i64(value * multiplier as f64)?;
        if (nanoseconds > 0 && v > i64::MAX - nanoseconds)
            || (nanoseconds < 0 && v < i64::MIN - nanoseconds)
        {
            return None;
        }
        v += nanoseconds;
        parsed_any_duration = true;
    }

    if v == i64::MIN {
        return None;
    }
    v *= sign;

    if !found_ago {
        i = skip_spaces(s, i);
        if i < s.len() {
            if !eq_ignore_case(&s[i..], b"ago") {
                return None;
            }
            found_ago = true;
            i += 3;
        }
    }

    if found_ago {
        if !parsed_any_duration {
            return None;
        }
        // "-7 days ago" stays negative
        if sign > 0 {
            v = -v;
        }
        if skip_spaces(s, i) < s.len() {
            return None;
        }
    }

    let multiplier = DURATION_UNITS[du_out].nanoseconds;
    if multiplier == 1 {
        Some(v)
    } else {
        Some((v as f64 / multiplier as f64).round() as i64)
    }
}

/// `duration_parse_seconds()`: seconds that fit an `int`.
pub fn duration_parse_seconds(duration: &[u8]) -> Option<i32> {
    duration_parse(duration, "s", "s").and_then(|v| i32::try_from(v).ok())
}

/// `duration_snprintf()`: renders `value` (a count of `unit`) with the largest
/// units first (`1d12h`, `-2h30m`), optionally space separated, `off` for 0.
///
/// Returns `None` where C returns `-3` (unknown unit). C truncates to its
/// destination buffer; this returns the complete text.
pub fn duration_to_string(value: i64, unit: &str, add_spaces: bool) -> Option<String> {
    if value == 0 {
        return Some("off".to_string());
    }
    let (mut sign, magnitude) = if value < 0 {
        ("-", value.unsigned_abs())
    } else {
        ("", value as u64)
    };
    let du_min = find_unit(unit.as_bytes())?;

    let mut out = String::new();
    let mut nsec = u128::from(magnitude) * DURATION_UNITS[du_min].nanoseconds as u128;

    for (i, du) in DURATION_UNITS.iter().enumerate().rev() {
        if nsec == 0 {
            break;
        }
        if !du.formatter && i != du_min {
            continue;
        }

        // rounding happens per unit (only at the smallest one), so the text parses back exactly
        let multiplier = du.nanoseconds as u128;
        let rounded = if i == du_min {
            (nsec / multiplier + u128::from(nsec % multiplier > multiplier / 2)) * multiplier
        } else {
            nsec
        };

        let unit_count = rounded / multiplier;
        if unit_count > 0 {
            let count = u64::try_from(unit_count).ok()?;
            if add_spaces && !out.is_empty() {
                out.push(' ');
            }
            out.push_str(sign);
            out.push_str(&count.to_string());
            out.push_str(du.name);
            sign = "";

            let unit_nsec = unit_count * multiplier;
            if unit_nsec >= nsec {
                break;
            }
            nsec -= unit_nsec;
        }

        if i == du_min {
            break;
        }
    }

    if out.is_empty() {
        out.push_str("off");
    }
    Some(out)
}
