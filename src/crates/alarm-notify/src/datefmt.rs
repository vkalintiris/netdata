//! Timestamp rendering.
//!
//! `date_format` is documented as "standard `date` command format strings", so users
//! have arbitrary strftime in their configuration - including `%Z`, which chrono
//! renders as a numeric offset rather than the zone abbreviation `date` prints. To
//! keep those strings meaning what they always meant, POSIX builds call the C
//! library's `strftime`; Windows, which has no `date` to be compatible with, uses
//! chrono.

/// The format `date` uses when given none, in the C locale.
const DEFAULT_FORMAT: &str = "%a %b %e %H:%M:%S %Z %Y";

/// Render `epoch_secs` using a `date`-style format string.
///
/// An empty `format` reproduces the script's fallback chain, which ended at plain
/// `date` and therefore at the C-locale default format.
pub fn format(epoch_secs: i64, format: &str, utc: bool) -> String {
    let fmt = format.trim();
    let fmt = fmt.strip_prefix('+').unwrap_or(fmt);
    let fmt = if fmt.is_empty() { DEFAULT_FORMAT } else { fmt };
    platform_format(epoch_secs, fmt, utc)
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub fn current_year() -> String {
    platform_format(now_secs(), "%Y", false)
}

/// RFC 3339-ish stamp with milliseconds, as PagerDuty expects it.
pub fn pagerduty_timestamp(epoch_secs: i64) -> String {
    platform_format(epoch_secs, "%Y-%m-%dT%H:%M:%S.000", false)
}

#[cfg(unix)]
fn platform_format(epoch_secs: i64, fmt: &str, utc: bool) -> String {
    use std::ffi::CString;

    let Ok(cfmt) = CString::new(fmt) else {
        // An embedded NUL cannot reach strftime; fall back to chrono.
        return chrono_format(epoch_secs, fmt, utc);
    };

    // SAFETY: `tm` is fully initialised by {local,gm}time_r before use, `buf` is
    // sized for strftime's output and only the returned prefix is read, and the
    // format string is a valid NUL-terminated C string that outlives the call.
    unsafe {
        let time = epoch_secs as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        let ok = if utc {
            !libc::gmtime_r(&time, &mut tm).is_null()
        } else {
            !libc::localtime_r(&time, &mut tm).is_null()
        };
        if !ok {
            return chrono_format(epoch_secs, fmt, utc);
        }

        let mut buf = vec![0u8; 1024];
        let n = libc::strftime(
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            cfmt.as_ptr(),
            &tm,
        );
        if n == 0 {
            // Either the output did not fit or the format produced nothing.
            return chrono_format(epoch_secs, fmt, utc);
        }
        buf.truncate(n);
        String::from_utf8_lossy(&buf).into_owned()
    }
}

#[cfg(not(unix))]
fn platform_format(epoch_secs: i64, fmt: &str, utc: bool) -> String {
    chrono_format(epoch_secs, fmt, utc)
}

fn chrono_format(epoch_secs: i64, fmt: &str, utc: bool) -> String {
    use chrono::{Local, TimeZone, Utc};
    match Utc.timestamp_opt(epoch_secs, 0).single() {
        Some(utc_dt) => {
            if utc {
                utc_dt.format(fmt).to_string()
            } else {
                Local
                    .from_utc_datetime(&utc_dt.naive_utc())
                    .format(fmt)
                    .to_string()
            }
        }
        None => String::new(),
    }
}

#[cfg(test)]
#[path = "datefmt_tests.rs"]
mod tests;
