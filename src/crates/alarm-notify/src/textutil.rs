//! Small text helpers that must reproduce the shell script's behaviour exactly.
//!
//! Anything in here is a direct port of a function in the old
//! `alarm-notify.sh`; the observable output of a notification depends on these
//! matching byte for byte, so they are ported literally rather than idiomatically
//! and pinned by tests.

/// Percent-encode like the script's `urlencode()`.
///
/// The script ran under `LC_ALL=C`, so its per-"character" loop was really a
/// per-byte loop: multi-byte UTF-8 is encoded byte by byte.
pub fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'-' | b'_' | b'.' | b'~' | b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02x}")),
        }
    }
    out
}

/// Render a duration in seconds the way `duration4human()` did.
///
/// Note the deliberate quirks: pluralisation is `> 1`, so zero renders as
/// "0 second", and the hour/minute carry only happens in the larger branches.
pub fn duration4human(seconds: i64) -> String {
    let mut s = seconds.max(0);
    let d = s / 86400;
    s -= d * 86400;
    let mut h = s / 3600;
    s -= h * 3600;
    let mut m = s / 60;
    s -= m * 60;

    let unit = |n: i64, singular: &str| {
        if n > 1 {
            format!("{singular}s")
        } else {
            singular.to_string()
        }
    };

    if d > 0 {
        if m >= 30 {
            h += 1;
        }
        if h > 0 {
            format!("{d} {} and {h} {}", unit(d, "day"), unit(h, "hour"))
        } else {
            format!("{d} {}", unit(d, "day"))
        }
    } else if h > 0 {
        if s >= 30 {
            m += 1;
        }
        if m > 0 {
            format!("{h} {} and {m} {}", unit(h, "hour"), unit(m, "minute"))
        } else {
            format!("{h} {}", unit(h, "hour"))
        }
    } else if m > 0 {
        if s > 0 {
            format!("{m} {} and {s} {}", unit(m, "minute"), unit(s, "second"))
        } else {
            format!("{m} {}", unit(m, "minute"))
        }
    } else {
        format!("{s} {}", unit(s, "second"))
    }
}

/// `${var//_/ }` - replace every underscore with a space.
pub fn underscores_to_spaces(s: &str) -> String {
    s.replace('_', " ")
}

/// `${name//[._]/-}` - used for the community forum link in the HTML e-mail.
pub fn dots_and_underscores_to_dashes(s: &str) -> String {
    s.replace(['.', '_'], "-")
}

/// Truncate to `max` characters, appending an ellipsis, like the script's
/// `${title:0:247}...` idiom. Operates on characters so multi-byte text is never
/// split mid-codepoint.
pub fn truncate_with_ellipsis(s: &str, max: usize, keep: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(keep).collect();
    format!("{head}...")
}

/// Hard truncation to `max` characters, no ellipsis (SMS bodies).
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        s.chars().take(max).collect()
    }
}

/// Split on commas *and* whitespace, dropping empties - the shell's
/// `for x in ${list//,/ }` behaviour under default `IFS`.
pub fn split_list(s: &str) -> Vec<&str> {
    s.split([',', ' ', '\t', '\n', '\r'])
        .filter(|t| !t.is_empty())
        .collect()
}

/// Expand a `${placeholder}` template against a lookup function.
///
/// Unknown placeholders are left verbatim: a template is data, and silently
/// blanking an unrecognised key would hide template/porting mistakes.
pub fn expand<'a, F>(template: &str, lookup: F) -> String
where
    F: Fn(&str) -> Option<&'a str>,
{
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = template[i + 2..].find('}') {
                let key = &template[i + 2..i + 2 + end];
                match lookup(key) {
                    Some(v) => out.push_str(v),
                    None => out.push_str(&template[i..i + 2 + end + 1]),
                }
                i += 2 + end + 1;
                continue;
            }
        }
        // Not a placeholder: copy this byte through.
        let ch_len = utf8_len(bytes[i]);
        out.push_str(&template[i..i + ch_len]);
        i += ch_len;
    }
    out
}

fn utf8_len(first: u8) -> usize {
    match first {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

#[cfg(test)]
#[path = "textutil_tests.rs"]
mod tests;
