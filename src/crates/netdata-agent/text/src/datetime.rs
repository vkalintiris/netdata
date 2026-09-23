//! RFC 3339 timestamps, mirroring `rfc3339_datetime_ut()` in
//! `libnetdata/datetime/rfc3339.c` for UTC.
//!
//! The local-time variant depends on the process time zone (`localtime_r()`)
//! and is not part of this crate.

/// `RFC3339_MAX_LENGTH`: the buffer size every caller uses.
pub const RFC3339_MAX_LENGTH: usize = 36;

/// `gmtime_r()` for non-negative times: (year, month 1-12, day, hour, minute, second).
fn civil_from_unix(t: u64) -> (i64, u32, u32, u32, u32, u32) {
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

    (year, month, day, secs / 3_600, secs / 60 % 60, secs % 60)
}

fn push_2digits(out: &mut String, value: u32) {
    out.push(char::from(b'0' + (value / 10 % 10) as u8));
    out.push(char::from(b'0' + (value % 10) as u8));
}

/// `rfc3339_datetime_ut(buf, RFC3339_MAX_LENGTH, now_ut, fractional_digits, true)`:
/// `YYYY-MM-DDThh:mm:ss[.fraction]Z`. The year is clamped to 0..=9999, the
/// fraction (at most 9 digits) is printed only when the microseconds are not
/// zero, and it is truncated, not rounded.
pub fn rfc3339_datetime_utc(now_ut: u64, fractional_digits: usize) -> String {
    let (year, month, day, hour, minute, second) = civil_from_unix(now_ut / 1_000_000);
    let year = year.clamp(0, 9999) as u32;

    let mut out = String::with_capacity(RFC3339_MAX_LENGTH);
    push_2digits(&mut out, year / 100);
    push_2digits(&mut out, year % 100);
    out.push('-');
    push_2digits(&mut out, month);
    out.push('-');
    push_2digits(&mut out, day);
    out.push('T');
    push_2digits(&mut out, hour);
    out.push(':');
    push_2digits(&mut out, minute);
    out.push(':');
    push_2digits(&mut out, second);

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

    out.push('Z');
    out
}
