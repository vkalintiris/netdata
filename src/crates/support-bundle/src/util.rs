//! Small shared helpers: progress output, UTC timestamps, PATH lookup,
//! interrupt flag.

#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Set by the signal handler; checked at every collector boundary so an
/// interrupted run cleans its staging and exits 130 without leaving work
/// half-published.
pub static INTERRUPTED: AtomicBool = AtomicBool::new(false);

pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

pub fn info(msg: &str) {
    eprintln!(" [*] {msg}");
}

/// Days-from-civil inverse (Howard Hinnant's algorithm): epoch seconds to
/// (year, month, day, hour, minute, second) in UTC. Avoids a date-time
/// dependency for the two fixed formats this tool needs.
fn utc_parts(secs: u64) -> (i64, u32, u32, u32, u32, u32) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, mi, s) = (
        (rem / 3600) as u32,
        ((rem % 3600) / 60) as u32,
        (rem % 60) as u32,
    );
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d, h, mi, s)
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `2026-07-17T12:34:56Z` — the provenance/manifest timestamp format.
pub fn utc_now_iso() -> String {
    let (y, m, d, h, mi, s) = utc_parts(now_epoch());
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// `20260717-123456` — the bundle name timestamp format.
pub fn utc_now_compact() -> String {
    let (y, m, d, h, mi, s) = utc_parts(now_epoch());
    format!("{y:04}{m:02}{d:02}-{h:02}{mi:02}{s:02}")
}

/// ISO timestamp for an arbitrary SystemTime (file mtimes in listings).
#[cfg(windows)]
pub fn iso_from_system_time(t: SystemTime) -> String {
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (y, m, d, h, mi, s) = utc_parts(secs);
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

/// Collapse a string to one line: newlines/tabs/CR become spaces, runs of
/// spaces are squeezed. With `collapse_backslash`, backslashes are removed
/// (shell line continuations; callers pass `cfg!(unix)` for command lines,
/// `false` for manifest fields where `\` may be a Windows path separator).
pub fn flatten_single_line(s: &str, collapse_backslash: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_space = false;
    for c in s.chars() {
        if collapse_backslash && c == '\\' {
            continue;
        }
        let c = if c == '\n' || c == '\t' || c == '\r' {
            ' '
        } else {
            c
        };
        if c == ' ' && last_space {
            continue;
        }
        last_space = c == ' ';
        out.push(c);
    }
    out
}

/// PATH lookup, the `command -v` equivalent.
#[cfg(unix)]
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(name);
        if is_executable(&cand) {
            return Some(cand);
        }
    }
    None
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    p.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(unix)]
pub fn have(name: &str) -> bool {
    which(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_parts_known_dates() {
        assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0));
        // 2026-07-17 00:00:00 UTC
        assert_eq!(utc_parts(1784246400), (2026, 7, 17, 0, 0, 0));
        // leap day: 2024-02-29 12:00:00 UTC
        assert_eq!(utc_parts(1709208000), (2024, 2, 29, 12, 0, 0));
    }
}
