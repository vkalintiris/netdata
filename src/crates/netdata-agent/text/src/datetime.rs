//! RFC 3339 timestamps, mirroring `rfc3339_datetime_ut()` in
//! `libnetdata/datetime/rfc3339.c`.
//!
//! The local-time variant needs the process time zone (`localtime_r()`), so its caller breaks the time down and
//! passes the fields with their UTC offset.

/// `RFC3339_MAX_LENGTH`: the buffer size every caller uses.
pub const RFC3339_MAX_LENGTH: usize = 36;

/// The C locale's abbreviated month names (`%b`).
pub const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// The broken-down time `rfc3339_datetime_ut()` prints (`struct tm` with the calendar year and the month counted
/// from 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CivilTime {
    pub year: i64,
    pub month: i64,
    pub day: i64,
    pub hour: i64,
    pub minute: i64,
    pub second: i64,
}

/// `gmtime_r()` for non-negative times.
fn civil_from_unix(t: u64) -> CivilTime {
    let days = (t / 86_400) as i64;
    let secs = (t % 86_400) as u32;

    // Howard Hinnant's days_from_civil inverse (proleptic Gregorian calendar)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);

    CivilTime {
        year,
        month: i64::from(month),
        day: i64::from(day),
        hour: i64::from(secs / 3_600),
        minute: i64::from(secs / 60 % 60),
        second: i64::from(secs % 60),
    }
}

/// `print_2digit()`: values outside 0..=99 are clamped.
fn push_2digits(out: &mut String, value: i64) {
    let value = value.clamp(0, 99) as u8;
    out.push(char::from(b'0' + value / 10));
    out.push(char::from(b'0' + value % 10));
}

/// `rfc3339_datetime_ut(buf, RFC3339_MAX_LENGTH, now_ut, fractional_digits, true)`:
/// `YYYY-MM-DDThh:mm:ss[.fraction]Z`. The year is clamped to 0..=9999, the
/// fraction (at most 9 digits) is printed only when the microseconds are not
/// zero, and it is truncated, not rounded.
pub fn rfc3339_datetime_utc(now_ut: u64, fractional_digits: usize) -> String {
    rfc3339_format(
        &civil_from_unix(now_ut / 1_000_000),
        now_ut,
        fractional_digits,
        None,
    )
}

/// `rfc3339_datetime_ut(buf, RFC3339_MAX_LENGTH, now_ut, fractional_digits, false)`, given `localtime_r()` of
/// `now_ut / 1_000_000` and its `tm_gmtoff`: the zone is `Z` when the offset is under a minute, else `±HH:MM`,
/// with the sign taken from the hours alone (C's `hours >= 0`), so an offset of -00:30 prints `+00:30`.
pub fn rfc3339_datetime_local(
    tm: &CivilTime,
    gmtoff: i64,
    now_ut: u64,
    fractional_digits: usize,
) -> String {
    rfc3339_format(tm, now_ut, fractional_digits, Some(gmtoff))
}

fn rfc3339_format(
    tm: &CivilTime,
    now_ut: u64,
    fractional_digits: usize,
    gmtoff: Option<i64>,
) -> String {
    let year = tm.year.clamp(0, 9999);

    let mut out = String::with_capacity(RFC3339_MAX_LENGTH);
    push_2digits(&mut out, year / 100);
    push_2digits(&mut out, year % 100);
    out.push('-');
    push_2digits(&mut out, tm.month);
    out.push('-');
    push_2digits(&mut out, tm.day);
    out.push('T');
    push_2digits(&mut out, tm.hour);
    out.push(':');
    push_2digits(&mut out, tm.minute);
    out.push(':');
    push_2digits(&mut out, tm.second);

    let digits = fractional_digits.min(9);
    let micros = now_ut % 1_000_000;
    if digits > 0 && micros > 0 {
        // scale the microseconds to the requested number of digits
        let scaled = if digits < 6 {
            micros / 10u64.pow(6 - digits as u32)
        } else {
            micros * 10u64.pow(digits as u32 - 6)
        };
        out.push('.');
        out.push_str(&format!("{scaled:0digits$}"));
    }

    match gmtoff {
        None => out.push('Z'),
        Some(offset) => {
            // C truncates both parts toward zero: -9000 s is -2 hours and 30 minutes.
            let hours = offset / 3600;
            let minutes = (offset % 3600 / 60).abs();
            if hours == 0 && minutes == 0 {
                out.push('Z');
            } else {
                out.push(if hours >= 0 { '+' } else { '-' });
                push_2digits(&mut out, hours.abs());
                out.push(':');
                push_2digits(&mut out, minutes);
            }
        }
    }
    out
}

/// `timegm()`: seconds since the epoch of a UTC civil time, days past the month's end carried forward as `timegm()`
/// normalizes them.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468 + day - 1
}

/// `strptime()`'s `%Y`/`%m`/... number: up to `max_digits` digits within `min..=max`, after `isspace()` bytes.
fn strptime_number(s: &[u8], at: &mut usize, max_digits: usize, min: i64, max: i64) -> Option<i64> {
    while s.get(*at).is_some_and(|c| matches!(c, b' ' | b'\t'..=b'\r')) {
        *at += 1;
    }
    let start = *at;
    let mut value = 0i64;
    while *at - start < max_digits && s.get(*at).is_some_and(u8::is_ascii_digit) {
        value = value * 10 + i64::from(s[*at] - b'0');
        *at += 1;
    }
    (*at > start && (min..=max).contains(&value)).then_some(value)
}

/// `rfc3339_parse_ut()`: `YYYY-MM-DDThh:mm:ss`, an optional fraction of 1 to 9 digits (microseconds kept), then `Z`
/// or `±hh:mm`; `None` for anything else. The microseconds wrap as C's `usec_t` for times before the epoch.
pub fn rfc3339_parse_ut(text: &[u8]) -> Option<u64> {
    let s = crate::c::c_str(text);
    let mut at = 0;
    let literal = |at: &mut usize, ch: u8| (s.get(*at) == Some(&ch)).then(|| *at += 1);
    let year = strptime_number(s, &mut at, 4, 0, 9999)?;
    literal(&mut at, b'-')?;
    let month = strptime_number(s, &mut at, 2, 1, 12)?;
    literal(&mut at, b'-')?;
    let day = strptime_number(s, &mut at, 2, 1, 31)?;
    literal(&mut at, b'T')?;
    let hour = strptime_number(s, &mut at, 2, 0, 23)?;
    literal(&mut at, b':')?;
    let minute = strptime_number(s, &mut at, 2, 0, 59)?;
    literal(&mut at, b':')?;
    let second = strptime_number(s, &mut at, 2, 0, 61)?;
    let mut usec = 0u64;
    if s.get(at) == Some(&b'.') {
        at += 1;
        let start = at;
        while s.get(at).is_some_and(u8::is_ascii_digit) {
            if at - start == 9 {
                return None;
            }
            usec = usec * 10 + u64::from(s[at] - b'0');
            at += 1;
        }
        const FIX: [u64; 10] = [
            1_000_000, 100_000, 10_000, 1_000, 100, 10, 1, 10, 100, 1_000,
        ];
        match at - start {
            0 => return None,
            n @ 1..=6 => usec *= FIX[n],
            n => usec /= FIX[n],
        }
    }
    let tz_offset: i64 = match s.get(at) {
        Some(&sign @ (b'+' | b'-')) => {
            let d = |i: usize| {
                s.get(at + i)
                    .filter(|c| c.is_ascii_digit())
                    .map(|c| i64::from(c - b'0'))
            };
            if s.get(at + 3) != Some(&b':') {
                return None;
            }
            let offset = (d(1)? * 10 + d(2)?) * 3600 + (d(4)? * 10 + d(5)?) * 60;
            if sign == b'+' { offset } else { -offset }
        }
        Some(b'Z') => 0,
        _ => return None,
    };
    let epoch_s = days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second;
    Some(
        (epoch_s as u64)
            .wrapping_mul(1_000_000)
            .wrapping_add(usec)
            .wrapping_sub((tz_offset as u64).wrapping_mul(1_000_000)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// C's parser: what the formatter writes reads back; fractions of 1 to 9 digits; zones; malformed texts fail.
    #[test]
    fn rfc3339_parses_as_c() {
        for ut in [
            0u64,
            1_790_551_528_123_456,
            1_790_551_528_000_000,
            1_790_551_528_120_000,
        ] {
            assert_eq!(
                rfc3339_parse_ut(rfc3339_datetime_utc(ut, 2).as_bytes()),
                Some(ut / 10_000 * 10_000)
            );
        }
        let base = 1_790_551_528_000_000u64;
        for (text, want) in [
            ("2026-09-27T23:25:28Z", Some(base)),
            ("2026-09-27T23:25:28.5Z", Some(base + 500_000)),
            ("2026-09-27T23:25:28.123456789Z", Some(base + 123_456)),
            ("2026-09-27T23:25:28.1234567890Z", None),
            ("2026-09-27T23:25:28.Z", None),
            ("2026-09-28T02:25:28+03:00", Some(base)),
            ("2026-09-27T22:25:28-01:00", Some(base)),
            ("2026-09-27T23:25:28+0300", None),
            ("2026-09-27T23:25:28", None),
            // glibc skips any isspace() before a number
            ("\t2026-09-27T\n23:25:28Z", Some(base)),
            ("2026-13-27T23:25:28Z", None),
            ("", None),
        ] {
            assert_eq!(rfc3339_parse_ut(text.as_bytes()), want, "{text}");
        }
    }

    fn tm(year: i64, month: i64, day: i64, hour: i64, minute: i64, second: i64) -> CivilTime {
        CivilTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
        }
    }

    #[test]
    fn local_offsets_follow_c() {
        let cases: [(i64, u64, &str); 5] = [
            (0, 465_000, "2026-09-24T08:25:12.465Z"),
            (19_800, 380_000, "2026-09-24T08:25:12.380+05:30"),
            (-9_000, 1, "2026-09-24T08:25:12.000-02:30"),
            (19_800, 0, "2026-09-24T08:25:12+05:30"),
            // -00:30: the sign comes from the zero hours, as in C
            (-1_800, 0, "2026-09-24T08:25:12+00:30"),
        ];
        for (gmtoff, micros, expected) in cases {
            let t = tm(2026, 9, 24, 8, 25, 12);
            assert_eq!(
                rfc3339_datetime_local(&t, gmtoff, 1_790_238_312_000_000 + micros, 3),
                expected
            );
        }
    }
}
