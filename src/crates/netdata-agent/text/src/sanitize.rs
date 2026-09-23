//! Text sanitizers, mirroring `libnetdata/sanitizers/` (`utf8-sanitizer.c`,
//! `chart_id_and_name.c`, `sanitizers-labels.c`, `sanitizers-functions.c`).
//!
//! The C functions write into a destination of `dst_size` bytes, including the
//! terminator; the ports take the same `dst_size` because the output is
//! truncated to it, and return the bytes C would leave before the terminator.

use crate::c::{self, is_cntrl, is_print, is_space};
use crate::print::HEX_DIGITS_LOWER;

/// `IS_UTF8_BYTE()`.
#[inline]
fn is_utf8_byte(b: u8) -> bool {
    b & 0x80 != 0
}

/// `IS_UTF8_STARTBYTE()`.
#[inline]
fn is_utf8_startbyte(b: u8) -> bool {
    b & 0xc0 == 0xc0
}

/// A byte translation table (`unsigned char char_map[256]`).
pub type CharMap = [u8; 256];

/// Builds a table from the printable ASCII overrides; control bytes, DEL and
/// bytes >= 0x80 map to space and printable ASCII maps to itself.
const fn char_map(overrides: &[(u8, u8)]) -> CharMap {
    let mut map = [b' '; 256];
    map[0] = 0;
    let mut c = 0x20;
    while c < 0x7f {
        map[c] = c as u8;
        c += 1;
    }
    let mut i = 0;
    while i < overrides.len() {
        map[overrides[i].0 as usize] = overrides[i].1;
        i += 1;
    }
    map
}

/// `rrd_string_allowed_chars[]`: units, titles, families, contexts, plugins, modules.
pub const RRD_STRING_ALLOWED_CHARS: CharMap = char_map(&[(b'"', b'\''), (b'\\', b'/')]);

/// `chart_names_allowed_chars[]`: chart and dimension ids and names.
pub const CHART_NAMES_ALLOWED_CHARS: CharMap = char_map(&[
    (b'"', b'_'),
    (b'$', b'_'),
    (b'%', b'_'),
    (b'&', b'_'),
    (b'\'', b'_'),
    (b'*', b'_'),
    (b'+', b'_'),
    (b',', b'.'),
    (b'=', b'_'),
    (b'?', b'_'),
    (b'@', b'_'),
    (b'\\', b'/'),
    (b'^', b'_'),
    (b'`', b'_'),
    (b'|', b'_'),
]);

/// `functions_allowed_chars[]` (`sanitizers-functions.c`).
pub const FUNCTIONS_ALLOWED_CHARS: CharMap = char_map(&[(b'"', b'\'')]);

/// `label_values_char_map[]`.
pub const LABEL_VALUES_CHAR_MAP: CharMap = char_map(&[
    (b'!', b'_'),
    (b'"', b'_'),
    (b'#', b'_'),
    (b'$', b'_'),
    (b'%', b'_'),
    (b'&', b'_'),
    (b'\'', b'_'),
    (b'*', b'_'),
    (b',', b'.'),
    (b';', b':'),
    (b'<', b'_'),
    (b'=', b':'),
    (b'>', b'_'),
    (b'?', b'_'),
    (b'\\', b'/'),
    (b'^', b'_'),
    (b'`', b'_'),
    (b'{', b'_'),
    (b'|', b'_'),
    (b'}', b'_'),
    (b'~', b'_'),
]);

/// `label_names_char_map[]`: the values map with the key-specific overrides
/// of `initialize_labels_keys_char_map()`.
pub const LABEL_NAMES_CHAR_MAP: CharMap = {
    let mut map = LABEL_VALUES_CHAR_MAP;
    map[b'=' as usize] = b'_';
    map[b':' as usize] = b'_';
    map[b'+' as usize] = b'_';
    map[b';' as usize] = b'_';
    map[b'@' as usize] = b'_';
    map[b'(' as usize] = b'_';
    map[b')' as usize] = b'_';
    map[b'\\' as usize] = b'/';
    map
};

/// `prometheus_label_names_char_map[]`: `[A-Za-z0-9:_]`, everything else `_`.
pub const PROMETHEUS_LABEL_NAMES_CHAR_MAP: CharMap = {
    let mut map = [b'_'; 256];
    let mut c = 0;
    while c < 256 {
        if (c as u8).is_ascii_alphanumeric() || c == b':' as usize {
            map[c] = c as u8;
        }
        c += 1;
    }
    map[0] = 0;
    map
};

/// Output of [`text_sanitize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sanitized {
    /// The `len` bytes C returns: the text before the terminator it writes.
    pub text: Vec<u8>,
    /// The C `multibyte_length`: characters, counting a UTF-8 sequence (or an
    /// invalid one hex-encoded) as one.
    pub chars: usize,
}

/// C `strncpyz(dst, empty, dst_size)` followed by `dst[dst_size - 1] = '\0'`.
fn empty_result(empty: &[u8], dst_size: usize) -> Sanitized {
    let empty = c::c_str(empty);
    let text = empty[..empty.len().min(dst_size - 1)].to_vec();
    let chars = text.len();
    Sanitized { text, chars }
}

/// `text_sanitize()`: maps each byte through `char_map`, collapses spaces,
/// trims leading and trailing spaces, keeps valid UTF-8 sequences when `utf`
/// (hex-encoding invalid ones) or replaces them with `_`, and returns `empty`
/// when the result is empty or all underscores. `dst_size == 0` yields an
/// empty result with `chars == 0`, where C writes nothing.
pub fn text_sanitize(
    src: &[u8],
    dst_size: usize,
    char_map: &CharMap,
    utf: bool,
    empty: &[u8],
) -> Sanitized {
    if dst_size == 0 {
        return Sanitized {
            text: Vec::new(),
            chars: 0,
        };
    }
    let src = c::c_str(src);

    // skip leading spaces and invalid characters
    let mut i = src
        .iter()
        .position(|&b| is_utf8_byte(b) || !(is_space(b) || is_cntrl(b) || !is_print(b)))
        .unwrap_or(src.len());
    if i == src.len() {
        return empty_result(empty, dst_size);
    }

    let at = |i: usize| c::at(src, i);
    let end = dst_size - 1;
    let mut d: Vec<u8> = Vec::with_capacity(end.min(src.len() * 2));
    let mut chars = 0usize;
    let mut last_is_space = true;

    while i < src.len() && d.len() < end {
        let ch = src[i];

        if is_utf8_startbyte(ch) {
            let sequence = if ch & 0xe0 == 0xc0 {
                2
            } else if ch & 0xf0 == 0xe0 {
                3
            } else if ch & 0xf8 == 0xf0 {
                4
            } else {
                1
            };
            let valid = sequence > 1
                && (1..sequence).all(|k| is_utf8_byte(at(i + k)) && !is_utf8_startbyte(at(i + k)));

            if utf {
                if valid && d.len() + sequence <= end {
                    d.extend_from_slice(&src[i..i + sequence]);
                    i += sequence;
                } else {
                    // hex-encode the start byte and every continuation byte after it
                    loop {
                        if d.len() + 1 < end {
                            d.push(HEX_DIGITS_LOWER[usize::from(src[i] >> 4)]);
                            d.push(HEX_DIGITS_LOWER[usize::from(src[i] & 0x0f)]);
                        }
                        i += 1;
                        if !(is_utf8_byte(at(i)) && !is_utf8_startbyte(at(i))) {
                            break;
                        }
                    }
                }
            } else {
                d.push(b'_');
                i += 1;
                while is_utf8_byte(at(i)) && !is_utf8_startbyte(at(i)) {
                    i += 1;
                }
            }
            last_is_space = false;
            chars += 1;
            continue;
        }

        let mapped = char_map[usize::from(ch)];
        if mapped == b' ' {
            if !last_is_space {
                d.push(b' ');
                chars += 1;
            }
            last_is_space = true;
        } else {
            d.push(mapped);
            last_is_space = false;
            chars += 1;
        }
        i += 1;
    }

    // remove trailing spaces
    while d.last() == Some(&b' ') {
        d.pop();
        chars -= 1;
    }

    // a result made only of underscores is empty; like C, a NUL emitted by a
    // custom map ends the string for this check but is kept in the output
    let text_len = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    if text_len == 0 || d[..text_len].iter().all(|&b| b == b'_') {
        return empty_result(empty, dst_size);
    }
    Sanitized { text: d, chars }
}

/// `sanitize_chart_name()`: [`CHART_NAMES_ALLOWED_CHARS`] with UTF-8, a
/// leading `!` becomes `_`, then every space becomes `_`.
fn sanitize_chart_name(src: &[u8], dst_size: usize) -> Vec<u8> {
    let mut text = text_sanitize(src, dst_size, &CHART_NAMES_ALLOWED_CHARS, true, b"").text;
    if text.first() == Some(&b'!') {
        text[0] = b'_';
    }
    for b in &mut text {
        if *b == b' ' {
            *b = b'_';
        }
    }
    text
}

/// `netdata_fix_chart_name()` / `netdata_fix_chart_id()` (identical in C):
/// sanitizes in place, so the result is at most `strlen(s)` bytes.
pub fn netdata_fix_chart_name(s: &[u8]) -> Vec<u8> {
    sanitize_chart_name(s, c::c_str(s).len() + 1)
}

/// `rrdset_strncpyz_name()`: [`netdata_fix_chart_name`] into a destination of
/// `dst_size_minus_1 + 1` bytes.
pub fn rrdset_strncpyz_name(src: &[u8], dst_size_minus_1: usize) -> Vec<u8> {
    sanitize_chart_name(src, dst_size_minus_1.saturating_add(1))
}

/// `RRDVAR_MAX_LENGTH` (`chart_id_and_name.c`, `health/rrdvar.h`).
pub const RRDVAR_MAX_LENGTH: usize = 1024;

/// `rrdvar_fix_name()`: the variable sanitized in place (truncated to
/// [`RRDVAR_MAX_LENGTH`] bytes), and whether it changed.
pub fn rrdvar_fix_name(variable: &[u8]) -> (Vec<u8>, bool) {
    let variable = c::c_str(variable);
    let len = variable.len().min(RRDVAR_MAX_LENGTH);
    let fixed = sanitize_chart_name(variable, len + 1);

    // C copies `len + 1` bytes before sanitizing in place and compares them
    // after; the bytes past the new terminator keep their old values
    let before: Vec<u8> = variable
        .iter()
        .copied()
        .chain(std::iter::once(0))
        .take(len + 1)
        .collect();
    let mut after = fixed.clone();
    after.push(0);
    if after.len() < before.len() {
        after.extend_from_slice(&before[after.len()..]);
    }
    let changed = before != after;
    (fixed, changed)
}

/// The sanitization of `rrd_string_strdupz()` (`database/rrd.c`): units,
/// titles, families, contexts, plugin and module names.
pub fn rrd_string_sanitize(s: &[u8]) -> Vec<u8> {
    let s = c::c_str(s);
    if s.is_empty() {
        return Vec::new();
    }
    text_sanitize(s, s.len() * 2 + 1, &RRD_STRING_ALLOWED_CHARS, true, b"").text
}

/// `rrdlabels_sanitize_name()`: UTF-8 becomes `_`, spaces become `_`.
pub fn rrdlabels_sanitize_name(src: &[u8], dst_size: usize) -> Vec<u8> {
    let mut text = text_sanitize(src, dst_size, &LABEL_NAMES_CHAR_MAP, false, b"").text;
    for b in &mut text {
        if *b == b' ' {
            *b = b'_';
        }
    }
    text
}

/// `rrdlabels_sanitize_value()`: UTF-8 kept, `[none]` when empty.
pub fn rrdlabels_sanitize_value(src: &[u8], dst_size: usize) -> Vec<u8> {
    text_sanitize(src, dst_size, &LABEL_VALUES_CHAR_MAP, true, b"[none]").text
}

/// `prometheus_rrdlabels_sanitize_name()`.
pub fn prometheus_rrdlabels_sanitize_name(src: &[u8], dst_size: usize) -> Vec<u8> {
    text_sanitize(src, dst_size, &PROMETHEUS_LABEL_NAMES_CHAR_MAP, false, b"").text
}

/// `nrpc_sanitize_name()`: function names.
pub fn nrpc_sanitize_name(src: &[u8], dst_size: usize) -> Vec<u8> {
    text_sanitize(src, dst_size, &FUNCTIONS_ALLOWED_CHARS, true, b"").text
}

/// `is_netdata_api_valid_character()`.
pub fn is_netdata_api_valid_character(c: u8) -> bool {
    if is_utf8_byte(c) {
        return true;
    }
    let t = CHART_NAMES_ALLOWED_CHARS[usize::from(c)];
    t == c && t != b' ' && t != b'!'
}
