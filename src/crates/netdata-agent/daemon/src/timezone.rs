//! The agent's time zone (`get_system_timezone()` and `refresh_system_timezone()`, `src/daemon/analytics.c`): its
//! name, from `TZ`, `/etc/localtime`, `/etc/timezone` or `[global] timezone`, and the abbreviation and UTC offset in
//! effect now, from the tzdb file (glibc builds have no `tzalloc()`) or the process time zone.

use std::path::Path;

use netdata_agent_inicfg::{Config, SECTION_GLOBAL};
use netdata_agent_sys::localtime;

/// The time zone triplet the host reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTimezone {
    pub name: String,
    pub abbrev: String,
    pub utc_offset: i32,
}

/// `timezone_name_is_safe_tzdb_path()`: relative, and only alphanumerics, `_`, `/`, `-` and `+`.
fn is_safe_tzdb_path(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'/' | b'-' | b'+'))
}

/// `timezone_abbrev_normalize()` into a 64-byte buffer: alphanumerics, `_`, `+` and `-` only, never empty.
fn normalize_abbrev(abbrev: &str) -> Option<String> {
    let ok = !abbrev.is_empty()
        && abbrev.len() < 64
        && abbrev
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-'));
    ok.then(|| abbrev.to_string())
}

/// `detect_system_timezone_name()`: the `/etc/localtime` symlink under `/usr/share/zoneinfo/`, else `/etc/timezone`,
/// sanitized to alphanumerics, `_`, `/`, `-` and `+`.
fn detect_name(root: &Path) -> Option<String> {
    let from_link = std::fs::read_link(root.join("etc/localtime"))
        .ok()
        .and_then(|target| {
            let target = target.to_string_lossy().into_owned();
            let marker = "/usr/share/zoneinfo/";
            target
                .find(marker)
                .map(|at| target[at + marker.len()..].to_string())
                .filter(|rest| !rest.is_empty())
        });
    let name = from_link.or_else(|| {
        let mut text = std::fs::read(root.join("etc/timezone")).ok()?;
        text.truncate(4096);
        Some(String::from_utf8_lossy(&text).into_owned())
    })?;
    let clean: String = name
        .chars()
        .filter(|&c| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-' | '+'))
        .collect();
    (!clean.is_empty()).then_some(clean)
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// `tzif_validate_header_counts()`.
fn counts_valid(c: &TzifCounts) -> bool {
    c.timecnt <= 2048
        && c.typecnt <= 256
        && c.charcnt <= 2048
        && c.leapcnt <= 50
        && c.isstdcnt <= 256
        && c.isgmtcnt <= 256
        && (c.isstdcnt == 0 || c.isstdcnt == c.typecnt)
        && (c.isgmtcnt == 0 || c.isgmtcnt == c.typecnt)
}

struct TzifCounts {
    isgmtcnt: usize,
    isstdcnt: usize,
    leapcnt: usize,
    timecnt: usize,
    typecnt: usize,
    charcnt: usize,
}

/// A TZif header (44 bytes): magic, version, 15 reserved bytes, then the six big-endian counts.
fn header(data: &[u8], at: usize) -> Option<(u8, TzifCounts)> {
    let h = data.get(at..at + 44)?;
    if &h[..4] != b"TZif" {
        return None;
    }
    let n = |i: usize| be32(&h[20 + 4 * i..]) as usize;
    Some((
        h[4],
        TzifCounts {
            isgmtcnt: n(0),
            isstdcnt: n(1),
            leapcnt: n(2),
            timecnt: n(3),
            typecnt: n(4),
            charcnt: n(5),
        },
    ))
}

/// `timezone_info_from_tzfile()`: the abbreviation and offset of the type in effect at `t`, from
/// `$TZDIR/<name>` (default `/usr/share/zoneinfo`); the v2 64-bit block when present.
fn from_tzfile(name: &str, t: i64) -> Option<(String, i32)> {
    if !is_safe_tzdb_path(name) {
        return None;
    }
    let dir = std::env::var("TZDIR").ok().filter(|d| !d.is_empty());
    let dir = dir.as_deref().unwrap_or("/usr/share/zoneinfo");
    let data = std::fs::read(format!("{dir}/{name}")).ok()?;
    let (version, mut counts) = header(&data, 0)?;
    let mut at = 44;
    let time_size = if version >= b'2' { 8 } else { 4 };
    if version >= b'2' {
        if !counts_valid(&counts) {
            return None;
        }
        at += counts.timecnt * 4
            + counts.timecnt
            + counts.typecnt * 6
            + counts.charcnt
            + counts.leapcnt * (4 + 4)
            + counts.isstdcnt
            + counts.isgmtcnt;
        counts = header(&data, at)?.1;
        at += 44;
    }
    if counts.typecnt == 0 || counts.charcnt == 0 || !counts_valid(&counts) {
        return None;
    }
    let times = data.get(at..at + counts.timecnt * time_size)?;
    at += counts.timecnt * time_size;
    let kinds = data.get(at..at + counts.timecnt)?;
    at += counts.timecnt;
    if kinds.iter().any(|&k| usize::from(k) >= counts.typecnt) {
        return None;
    }
    let types = data.get(at..at + counts.typecnt * 6)?;
    at += counts.typecnt * 6;
    let abbrs = data.get(at..at + counts.charcnt)?;
    let ttinfo = |i: usize| {
        let b = &types[i * 6..i * 6 + 6];
        (be32(b) as i32, b[4], usize::from(b[5]))
    };
    // tzif_default_type_index(): the first standard-time type, else the first.
    let default_type = (0..counts.typecnt).find(|&i| ttinfo(i).1 == 0).unwrap_or(0);
    let mut index = None;
    for i in 0..counts.timecnt {
        let b = &times[i * time_size..];
        let when = if time_size == 8 {
            i64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        } else {
            i64::from(be32(b) as i32)
        };
        if t < when {
            break;
        }
        index = Some(usize::from(kinds[i]));
    }
    let (gmtoff, _, abbrind) = ttinfo(index.unwrap_or(default_type));
    if abbrind >= counts.charcnt {
        return None;
    }
    let text = &abbrs[abbrind..];
    let end = text.iter().position(|&b| b == 0).unwrap_or(text.len());
    let mut abbrev = String::from_utf8_lossy(&text[..end]).into_owned();
    if abbrev.is_empty() {
        abbrev = "UTC".to_string();
    }
    // strncpyz() into 64 bytes.
    abbrev.truncate(63);
    Some((abbrev, gmtoff))
}

/// `current_process_timezone_info()`: `strftime("%Z")` (empty: `UTC`) and the offset `strftime("%z")` prints, from
/// the process time zone.
fn from_process(t: i64) -> Option<(String, i32)> {
    let tm = localtime(t)?;
    let abbrev = if tm.zone.is_empty() {
        "UTC".to_string()
    } else {
        tm.zone
    };
    // "%z" is +hhmm: the seconds of the offset are dropped.
    let minutes = tm.gmtoff.abs() / 60;
    let offset = ((minutes / 60) * 3600 + (minutes % 60) * 60) as i32;
    Some((abbrev, if tm.gmtoff < 0 { -offset } else { offset }))
}

/// `refresh_system_timezone()`: the abbreviation (else `UTC`) and offset of `name` at `t`.
fn refresh(name: &str, tzdb: bool, t: i64) -> SystemTimezone {
    let info = tzdb
        .then(|| from_tzfile(name, t))
        .flatten()
        .or_else(|| from_process(t));
    let (abbrev, utc_offset) = match info {
        Some((abbrev, offset)) => (
            normalize_abbrev(&abbrev).unwrap_or_else(|| "UTC".to_string()),
            offset,
        ),
        None => ("UTC".to_string(), 0),
    };
    SystemTimezone {
        name: name.to_string(),
        abbrev,
        utc_offset,
    }
}

/// `get_system_timezone()` after `TZ` is set: detects the name, lets `[global] timezone` override it (an empty
/// value does not), and resolves the abbreviation and offset at `now_s`. `root` is `/` outside tests.
pub fn system_timezone(c: &mut Config, root: &Path, now_s: i64) -> SystemTimezone {
    let mut tzdb = false;
    let mut name = std::env::var("TZ")
        .ok()
        .filter(|tz| !tz.is_empty() && !tz.starts_with(':'));
    if name.is_some() {
        tzdb = true;
    }
    if name.is_none() {
        name = detect_name(root);
        tzdb = name.is_some();
    }
    if name.is_none() {
        name = localtime(now_s).map(|tm| tm.zone).filter(|z| !z.is_empty());
    }
    let name = name.unwrap_or_else(|| "unknown".to_string());
    let mut user_configured = c.exists(SECTION_GLOBAL, "timezone");
    let default = if tzdb {
        if is_safe_tzdb_path(&name) {
            name
        } else {
            "unknown".to_string()
        }
    } else {
        normalize_abbrev(&name).unwrap_or_else(|| "unknown".to_string())
    };
    let mut configured = String::from_utf8_lossy(
        &c.get(SECTION_GLOBAL, "timezone", Some(&default))
            .unwrap_or_default(),
    )
    .into_owned();
    if user_configured && configured.is_empty() {
        user_configured = false;
        configured = default;
    }
    if user_configured {
        tzdb = true;
    }
    refresh(&configured, tzdb, now_s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_abbreviations_are_checked_like_c() {
        assert!(is_safe_tzdb_path("Europe/Athens"));
        assert!(is_safe_tzdb_path("Etc/GMT+2"));
        assert!(!is_safe_tzdb_path("/etc/passwd"));
        assert!(!is_safe_tzdb_path("../x y"));
        assert_eq!(normalize_abbrev("EEST").as_deref(), Some("EEST"));
        assert_eq!(normalize_abbrev("+03"), Some("+03".to_string()));
        assert_eq!(normalize_abbrev("A B"), None);
        assert_eq!(normalize_abbrev(""), None);
    }

    #[test]
    fn detects_the_zone_from_etc() {
        let dir = std::env::temp_dir().join(format!("nd-tz-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("etc")).unwrap();
        std::fs::write(dir.join("etc/timezone"), "Europe/Athens\n").unwrap();
        assert_eq!(detect_name(&dir).as_deref(), Some("Europe/Athens"));
        std::os::unix::fs::symlink(
            "/usr/share/zoneinfo/America/New_York",
            dir.join("etc/localtime"),
        )
        .unwrap();
        assert_eq!(detect_name(&dir).as_deref(), Some("America/New_York"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tzfiles_give_the_type_in_effect() {
        if !Path::new("/usr/share/zoneinfo/Europe/Athens").exists() {
            return;
        }
        // 2024-01-15 and 2024-07-15, 12:00 UTC.
        assert_eq!(
            from_tzfile("Europe/Athens", 1_705_320_000),
            Some(("EET".to_string(), 7200))
        );
        assert_eq!(
            from_tzfile("Europe/Athens", 1_721_044_800),
            Some(("EEST".to_string(), 10800))
        );
        assert_eq!(
            from_tzfile("UTC", 1_721_044_800),
            Some(("UTC".to_string(), 0))
        );
        assert_eq!(from_tzfile("No/Such_Zone", 0), None);
    }
}
