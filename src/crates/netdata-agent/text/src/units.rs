//! A value with its units as text: `format_value_and_unit()` of C's
//! `src/web/api/v1/api_v1_badge/web_buffer_svg.c`, as the health alert log makes an entry's value texts (C calls it
//! with a 100-byte buffer length and precision -1, `src/health/health_log.c`) and as a badge prints its value (with
//! the request's precision).
//!
//! Some units are not printed but read: a number of seconds, minutes or hours becomes a duration, a number under a
//! two-word unit (`on/off`, `up/down`, `ok/error`, `ok/failed`) becomes one of the two words. Every other value is
//! printed with a precision that follows its size, without trailing zeros.

use crate::print::print_fixed;

/// The longest text C's callers get: they pass a buffer length of 100, and `snprintfz()` keeps the last byte for
/// the terminator.
const VALUE_STRING_MAX: usize = 99;

#[derive(Clone, Copy, PartialEq)]
enum Format {
    None,
    Seconds,
    SecondsAgo,
    Minutes,
    MinutesAgo,
    Hours,
    HoursAgo,
    OnOff,
    UpDown,
    OkError,
    OkFailed,
    Empty,
    Percent,
}

/// C's `badge_units_formatters[]`: matched exactly, case and all.
const FORMATTERS: [(&[u8], Format); 23] = [
    (b"seconds", Format::Seconds),
    (b"seconds ago", Format::SecondsAgo),
    (b"minutes", Format::Minutes),
    (b"minutes ago", Format::MinutesAgo),
    (b"hours", Format::Hours),
    (b"hours ago", Format::HoursAgo),
    (b"on/off", Format::OnOff),
    (b"on-off", Format::OnOff),
    (b"onoff", Format::OnOff),
    (b"up/down", Format::UpDown),
    (b"up-down", Format::UpDown),
    (b"updown", Format::UpDown),
    (b"ok/error", Format::OkError),
    (b"ok-error", Format::OkError),
    (b"okerror", Format::OkError),
    (b"ok/failed", Format::OkFailed),
    (b"ok-failed", Format::OkFailed),
    (b"okfailed", Format::OkFailed),
    (b"empty", Format::Empty),
    (b"null", Format::Empty),
    (b"percentage", Format::Percent),
    (b"percent", Format::Percent),
    (b"pcent", Format::Percent),
];

/// C's `badge_time_value_is_undefined()`: a time that cannot be a count (negative, not a number, or past what
/// `size_t` holds).
fn time_is_undefined(value: f64) -> bool {
    const SIZE_MAX_PLUS_ONE: f64 = 18_446_744_073_709_551_616.0;
    value < 0.0 || !value.is_finite() || value >= SIZE_MAX_PLUS_ONE
}

/// One of the six time units: `now` for zero, `undefined` for what is no count, else the duration `shape` makes of
/// the value cut to a whole number.
fn time(value: f64, shape: impl FnOnce(u64) -> String) -> Vec<u8> {
    if value == 0.0 {
        b"now".to_vec()
    } else if time_is_undefined(value) {
        b"undefined".to_vec()
    } else {
        shape(value as u64).into_bytes()
    }
}

/// C's `format_value_with_precision_and_unit()`. For a negative precision the digits follow the value's size, and
/// trailing zeros (then the point) go, but never the first digit. Any other precision is that many digits, 50 at
/// most, with every zero kept.
fn number(value: f64, units: &[u8], precision: i32) -> Vec<u8> {
    if precision >= 0 {
        let mut out = Vec::with_capacity(64);
        print_fixed(&mut out, value, precision.min(50) as usize);
        if units.first().is_some_and(u8::is_ascii_alphanumeric) {
            out.push(b' ');
        }
        out.extend_from_slice(units);
        out.truncate(VALUE_STRING_MAX);
        return out;
    }
    let abs = value.abs();
    let (precision, trim_zeros) = if abs >= 1000.0 {
        (0, false)
    } else if abs >= 10.0 {
        (1, true)
    } else if abs >= 1.0 || abs >= 0.1 {
        (2, true)
    } else if abs >= 0.01 {
        (4, true)
    } else if abs >= 0.001 {
        (5, true)
    } else if abs >= 0.0001 {
        (6, true)
    } else {
        (7, true)
    };

    let mut out = Vec::with_capacity(32);
    print_fixed(&mut out, value, precision);
    out.truncate(VALUE_STRING_MAX);

    if trim_zeros {
        // C stops above the sign of a value below zero; a negative zero's sign is a digit to it
        let stop = usize::from(value < 0.0);
        let mut len = out.len();
        while len > stop + 1 {
            match out[len - 1] {
                b'0' => len -= 1,
                b'.' => {
                    len -= 1;
                    break;
                }
                _ => break,
            }
        }
        out.truncate(len);
    }

    if units.first().is_some_and(u8::is_ascii_alphanumeric) {
        out.push(b' ');
    }
    out.extend_from_slice(units);
    out.truncate(VALUE_STRING_MAX);
    out
}

/// C's `format_value_and_unit(buf, 100, value, units, -1)`, as the alert log asks. Units are bytes up to their first
/// NUL; C's NULL units are the empty text.
pub fn format_value_and_unit(value: f64, units: &[u8]) -> Vec<u8> {
    format_value_and_unit_precision(value, units, -1)
}

/// C's `format_value_and_unit(buf, 100, value, units, precision)`, as a badge asks: the precision counts only for a
/// value that is printed as a number.
pub fn format_value_and_unit_precision(value: f64, units: &[u8], precision: i32) -> Vec<u8> {
    let units = crate::c::c_str(units);
    let format = FORMATTERS.iter().find(|(name, _)| *name == units).map_or(Format::None, |(_, format)| *format);

    let ago = |format: Format, this: Format| if format == this { " ago" } else { "" };
    let units: &[u8] = match format {
        Format::Seconds | Format::SecondsAgo => {
            let suffix = ago(format, Format::SecondsAgo);
            return time(value, |seconds| {
                let (d, h, m, s) = (seconds / 86400, seconds % 86400 / 3600, seconds % 3600 / 60, seconds % 60);
                match d {
                    0 => format!("{h:02}:{m:02}:{s:02}{suffix}"),
                    1 => format!("1 day {h:02}:{m:02}:{s:02}{suffix}"),
                    _ => format!("{d} days {h:02}:{m:02}:{s:02}{suffix}"),
                }
            });
        }
        Format::Minutes | Format::MinutesAgo => {
            let suffix = ago(format, Format::MinutesAgo);
            return time(value, |minutes| {
                let (d, h, m) = (minutes / 1440, minutes % 1440 / 60, minutes % 60);
                if d == 0 { format!("{h}h {m}m{suffix}") } else { format!("{d}d {h:02}h {m:02}m{suffix}") }
            });
        }
        Format::Hours | Format::HoursAgo => {
            let suffix = ago(format, Format::HoursAgo);
            return time(value, |hours| {
                let (d, h) = (hours / 24, hours % 24);
                if d == 0 { format!("{h}h{suffix}") } else { format!("{d}d {h}h{suffix}") }
            });
        }
        // C asks `value != 0.0`: a NaN is the first word
        Format::OnOff => return if value != 0.0 { b"on".to_vec() } else { b"off".to_vec() },
        Format::UpDown => return if value != 0.0 { b"up".to_vec() } else { b"down".to_vec() },
        Format::OkError => return if value != 0.0 { b"ok".to_vec() } else { b"error".to_vec() },
        Format::OkFailed => return if value != 0.0 { b"ok".to_vec() } else { b"failed".to_vec() },
        Format::Empty => b"",
        Format::Percent => b"%",
        Format::None => units,
    };

    if !value.is_finite() {
        return b"-".to_vec();
    }
    number(value, units, precision)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: f64, units: &str) -> String {
        String::from_utf8(format_value_and_unit(value, units.as_bytes())).unwrap()
    }

    // C's own answers for 2,784 inputs are in the health crate's `tests/vectors/units.tsv` (its `tests/loop.rs`);
    // these are the shapes.
    #[test]
    fn a_number_keeps_the_digits_its_size_needs() {
        for (value, units, expected) in [
            (10.0, "things", "10 things"),
            (10.0, "%", "10%"),
            (10.0, "", "10"),
            (1.5, "MiB", "1.5 MiB"),
            (0.00012345, "x", "0.000123 x"),
            (1000.5, "x", "1000 x"),
            (-0.5, "x", "-0.5 x"),
            (0.0, "x", "0 x"),
            (f64::NAN, "x", "-"),
            (f64::INFINITY, "percent", "-"),
            (12.0, "percent", "12%"),
            (12.0, "null", "12"),
        ] {
            assert_eq!(text(value, units), expected, "{value} {units:?}");
        }
    }

    #[test]
    fn a_time_is_a_duration_and_a_two_word_unit_is_a_word() {
        for (value, units, expected) in [
            (0.0, "seconds", "now"),
            (-1.0, "seconds", "undefined"),
            (f64::NAN, "minutes ago", "undefined"),
            (70.0, "seconds", "00:01:10"),
            (90061.0, "seconds ago", "1 day 01:01:01 ago"),
            (172801.0, "seconds", "2 days 00:00:01"),
            (61.0, "minutes", "1h 1m"),
            (1441.0, "minutes ago", "1d 00h 01m ago"),
            (5.0, "hours", "5h"),
            (49.0, "hours ago", "2d 1h ago"),
            (0.0, "on/off", "off"),
            (f64::NAN, "on/off", "on"),
            (0.0, "updown", "down"),
            (2.0, "ok-error", "ok"),
            (0.0, "okfailed", "failed"),
            // the table is exact: another case is another unit
            (70.0, "Seconds", "70 Seconds"),
        ] {
            assert_eq!(text(value, units), expected, "{value} {units:?}");
        }
    }

    #[test]
    fn a_text_is_cut_where_c_s_buffer_ends() {
        let units = "u".repeat(200);
        assert_eq!(text(1.0, &units).len(), VALUE_STRING_MAX);
        assert_eq!(text(1e300, "x").len(), VALUE_STRING_MAX);
    }
}
