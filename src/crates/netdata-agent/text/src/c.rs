//! C runtime semantics the ported code depends on, shared with the other agent crates that port C string code.
//!
//! The Agent never calls `setlocale()`, so every `<ctype.h>` and `strcasecmp()`
//! call in the C sources runs in the "C" locale: ASCII-only classification and
//! case folding. The numeric conversions below reproduce what gcc emits for
//! x86-64 (SSE2), including the results of out-of-range conversions that are
//! undefined in ISO C but deterministic on that target.

/// The C string view of `s`: everything before the first NUL byte.
///
/// Inputs are byte slices; the C functions they mirror stop at the terminator.
pub fn c_str(s: &[u8]) -> &[u8] {
    match s.iter().position(|&b| b == 0) {
        Some(n) => &s[..n],
        None => s,
    }
}

/// `s[i]`, or the terminating NUL past the end (C reads the terminator).
#[inline]
pub fn at(s: &[u8], i: usize) -> u8 {
    s.get(i).copied().unwrap_or(0)
}

/// `isspace()` in the C locale.
#[inline]
pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `isalpha()` in the C locale.
#[inline]
pub fn is_alpha(c: u8) -> bool {
    c.is_ascii_alphabetic()
}

/// `iscntrl()` in the C locale.
#[inline]
pub fn is_cntrl(c: u8) -> bool {
    c < 0x20 || c == 0x7f
}

/// `isprint()` in the C locale.
#[inline]
pub fn is_print(c: u8) -> bool {
    (0x20..0x7f).contains(&c)
}

/// Index of the first byte of `s` at or after `i` that is not `isspace()`.
#[inline]
pub fn skip_spaces(s: &[u8], mut i: usize) -> usize {
    while is_space(at(s, i)) {
        i += 1;
    }
    i
}

/// `strcasecmp(a, b) == 0` in the C locale.
#[inline]
pub fn eq_ignore_case(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// `strcasestr()` in the C locale: offset of the first match of a non-empty needle.
pub fn find_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len())
        .find(|&i| haystack[i..i + needle.len()].eq_ignore_ascii_case(needle))
}

/// `strstr()`: offset of the first match of a non-empty needle.
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// `strsep_skip_consecutive_separators()`: the next non-empty token of `rest` delimited by any byte of `separators`.
/// Returns an empty token once `rest` is exhausted (`None`), or when its last token is empty; C never returns NULL.
pub fn strsep_skip<'a>(rest: &mut Option<&'a [u8]>, separators: &[u8]) -> &'a [u8] {
    while let Some(s) = *rest {
        let token = match s.iter().position(|c| separators.contains(c)) {
            Some(end) => {
                *rest = Some(&s[end + 1..]);
                &s[..end]
            }
            None => {
                *rest = None;
                s
            }
        };
        if !token.is_empty() {
            return token;
        }
    }
    b""
}

/// `filename_from_path_entry()`: `path` and `entry` joined by one slash (trailing slashes of `path` and leading ones of
/// `entry` collapse; an empty `path` is `.`), with `.extension` when one is given.
pub fn filename_from_path_entry(path: &str, entry: &str, extension: Option<&str>) -> String {
    let path = if path.is_empty() { "." } else { path };
    let trimmed = path.trim_end_matches('/');
    let entry = entry.trim_start_matches('/');
    let (head, slash) = if trimmed.len() < path.len() && (!entry.is_empty() || trimmed.is_empty()) {
        // keep one of the trailing slashes instead of adding one
        (&path[..trimmed.len() + 1], "")
    } else if entry.is_empty() {
        (trimmed, "")
    } else {
        (trimmed, "/")
    };
    let mut out = format!("{head}{slash}{entry}");
    if let Some(extension) = extension.filter(|e| !e.is_empty()) {
        out.push('.');
        out.push_str(extension);
    }
    out
}

/// The pieces successive `fgets(buf, size, fp)` calls return from `data`: each ends after a newline or holds
/// `size - 1` bytes, whichever comes first; the last may end without a newline.
pub fn fgets_chunks(data: &[u8], size: usize) -> impl Iterator<Item = &[u8]> {
    let max = size.saturating_sub(1).max(1);
    let mut rest = data;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let end = rest[..rest.len().min(max)]
            .iter()
            .position(|&c| c == b'\n')
            .map_or(rest.len().min(max), |nl| nl + 1);
        let (chunk, tail) = rest.split_at(end);
        rest = tail;
        Some(chunk)
    })
}

const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;
const TWO_POW_31: f64 = 2_147_483_648.0;

/// `cvttsd2si` with a 64-bit destination: truncation, or the "integer
/// indefinite" value 0x8000000000000000 when the result does not fit.
#[inline]
fn cvttsd2si64(x: f64) -> i64 {
    if (-TWO_POW_63..TWO_POW_63).contains(&x) {
        x as i64
    } else {
        i64::MIN
    }
}

/// `(uint64_t)x` as lowered by gcc on x86-64: `comisd 2^63` then either a
/// plain `cvttsd2si`, or `cvttsd2si(x - 2^63)` with bit 63 flipped.
#[inline]
pub(crate) fn double_to_u64(x: f64) -> u64 {
    if x >= TWO_POW_63 {
        (cvttsd2si64(x - TWO_POW_63) as u64) ^ (1 << 63)
    } else {
        cvttsd2si64(x) as u64
    }
}

/// `(int)x` on x86-64 (`cvttsd2si` with a 32-bit destination).
#[inline]
pub(crate) fn double_to_i32(x: f64) -> i32 {
    if x > -TWO_POW_31 - 1.0 && x < TWO_POW_31 {
        x as i32
    } else {
        i32::MIN
    }
}

/// glibc `llrint()` in the default round-to-nearest-even mode; like
/// `cvtsd2si`, NaN and out-of-range inputs return `LLONG_MIN`.
#[inline]
pub(crate) fn llrint(x: f64) -> i64 {
    let r = x.round_ties_even();
    if (-TWO_POW_63..TWO_POW_63).contains(&r) {
        r as i64
    } else {
        i64::MIN
    }
}

/// glibc `modf()`: returns `(fractional, integral)`; infinities have a zero
/// fractional part carrying the sign of the input.
#[inline]
pub(crate) fn modf(x: f64) -> (f64, f64) {
    if x.is_infinite() {
        (0.0f64.copysign(x), x)
    } else {
        let integral = x.trunc();
        (x - integral, integral)
    }
}

#[cfg(test)]
mod tests {
    /// C's `check_strdupz_path_subpath()` (`src/daemon/unit_test.c`).
    #[test]
    fn paths_join_as_c() {
        let cases = [
            ("", "", "."),
            ("/", "", "/"),
            ("/etc/netdata", "", "/etc/netdata"),
            ("/etc/netdata///", "", "/etc/netdata"),
            ("/etc/netdata///", "health.d", "/etc/netdata/health.d"),
            ("/etc/netdata///", "///health.d", "/etc/netdata/health.d"),
            ("/etc/netdata", "///health.d", "/etc/netdata/health.d"),
            ("", "///health.d", "./health.d"),
            ("/", "///health.d", "/health.d"),
        ];
        for (path, entry, want) in cases {
            assert_eq!(
                filename_from_path_entry(path, entry, None),
                want,
                "{path:?} {entry:?}"
            );
        }
        assert_eq!(
            filename_from_path_entry("/x/", "tokens", Some("json")),
            "/x/tokens.json"
        );
    }

    #[test]
    fn fgets_chunks_split_at_newlines_and_size() {
        let chunks: Vec<&[u8]> = fgets_chunks(b"ab\ncdefg\n\nh", 4).collect();
        assert_eq!(chunks, [&b"ab\n"[..], b"cde", b"fg\n", b"\n", b"h"]);
        assert_eq!(fgets_chunks(b"", 4).count(), 0);
    }

    use super::*;

    #[test]
    fn strsep_skip_matches_c() {
        let mut rest = Some(&b"//api/v1//info?x"[..]);
        assert_eq!(strsep_skip(&mut rest, b"/?"), b"api");
        assert_eq!(rest, Some(&b"v1//info?x"[..]));
        assert_eq!(strsep_skip(&mut rest, b"/"), b"v1");
        assert_eq!(strsep_skip(&mut rest, b"/"), b"info?x");
        assert_eq!(rest, None);
        assert_eq!(strsep_skip(&mut rest, b"/"), b"");
        let mut rest = Some(&b"v2/"[..]);
        assert_eq!(strsep_skip(&mut rest, b"/?"), b"v2");
        assert_eq!(rest, Some(&b""[..]));
        assert_eq!(strsep_skip(&mut rest, b"/?"), b"");
        assert_eq!(rest, None);
    }

    #[test]
    fn conversions_follow_x86_64() {
        let cases: &[(f64, u64, i32, i64)] = &[
            (0.0, 0, 0, 0),
            (-1.5, u64::MAX, -1, -2),
            (TWO_POW_63, 1 << 63, i32::MIN, i64::MIN),
            (18_446_744_073_709_551_616.0, 0, i32::MIN, i64::MIN),
            (f64::INFINITY, 0, i32::MIN, i64::MIN),
            (f64::NAN, 1 << 63, i32::MIN, i64::MIN),
            (2.5, 2, 2, 2),
            (3.5, 3, 3, 4),
            (
                -2_147_483_648.9,
                0xffff_ffff_8000_0000,
                i32::MIN,
                -2_147_483_649,
            ),
        ];
        for &(x, u, i, r) in cases {
            assert_eq!(
                (double_to_u64(x), double_to_i32(x), llrint(x)),
                (u, i, r),
                "input {x}"
            );
        }
    }

    #[test]
    fn modf_matches_glibc() {
        let (f, i) = modf(f64::NEG_INFINITY);
        assert_eq!((f.to_bits(), i), ((-0.0f64).to_bits(), f64::NEG_INFINITY));
        assert_eq!(modf(-2.25), (-0.25, -2.0));
    }
}
