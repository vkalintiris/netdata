//! Number parsing, mirroring the parsers in `libnetdata/inlined.h` and glibc
//! `strtod()` in the "C" locale.
//!
//! All parsers take bytes and treat the end of the slice, or a NUL byte, as the
//! C string terminator. Functions that have an `endptr` in C return
//! `(value, consumed)`, where `consumed` is `endptr - input` in bytes. Integer
//! accumulation wraps on overflow exactly like the unsigned C arithmetic.

use crate::c::{self, at, skip_spaces};
use crate::print::{DOUBLE_B64_PREFIX, DOUBLE_HEX_PREFIX, UINT64_B64_PREFIX};

#[inline]
fn is_digit(ch: u8) -> bool {
    ch.is_ascii_digit()
}

/// `hex_value_from_ascii[]` (255 = not a hex digit).
#[inline]
fn hex_value(ch: u8) -> Option<u8> {
    char::from(ch).to_digit(16).map(|v| v as u8)
}

/// `base64_value_from_ascii[]` (255 = not a base64 digit).
#[inline]
fn base64_value(ch: u8) -> Option<u8> {
    match ch {
        b'A'..=b'Z' => Some(ch - b'A'),
        b'a'..=b'z' => Some(ch - b'a' + 26),
        b'0'..=b'9' => Some(ch - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

/// `str2uint64_t()` / `str2ull()`: decimal digits after `isspace()`.
pub fn str2uint64(s: &[u8]) -> (u64, usize) {
    let mut i = skip_spaces(s, 0);
    let mut n: u64 = 0;
    while is_digit(at(s, i)) {
        n = n.wrapping_mul(10).wrapping_add(u64::from(at(s, i) - b'0'));
        i += 1;
    }
    (n, i)
}

/// `str2uint32_t()` (also `str2pid_t()`); arithmetic modulo 2^32 is the
/// truncation of the 64-bit accumulation.
pub fn str2uint32(s: &[u8]) -> (u32, usize) {
    let (n, i) = str2uint64(s);
    (n as u32, i)
}

/// `str2ll()`: optional sign after `isspace()`, then [`str2uint64`] (which
/// skips `isspace()` again, so `"- 5"` is `-5`).
pub fn str2ll(s: &[u8]) -> (i64, usize) {
    let i = skip_spaces(s, 0);
    match at(s, i) {
        b'-' => {
            let (n, used) = str2uint64(&s[i + 1..]);
            ((n as i64).wrapping_neg(), i + 1 + used)
        }
        b'+' => {
            let (n, used) = str2uint64(&s[i + 1..]);
            (n as i64, i + 1 + used)
        }
        _ => {
            let (n, used) = str2uint64(&s[i..]);
            (n as i64, i + used)
        }
    }
}

/// `str2u()`: unsigned 32-bit, no end pointer.
pub fn str2u(s: &[u8]) -> u32 {
    str2uint32(s).0
}

/// `str2ul()`: unsigned long (64-bit on Linux x86-64), no end pointer.
pub fn str2ul(s: &[u8]) -> u64 {
    str2uint64(s).0
}

/// `str2i()`: like [`str2ll`] on 32 bits, no end pointer.
pub fn str2i(s: &[u8]) -> i32 {
    let i = skip_spaces(s, 0);
    match at(s, i) {
        b'-' => (str2u(&s[i + 1..]) as i32).wrapping_neg(),
        b'+' => str2u(&s[i + 1..]) as i32,
        _ => str2u(&s[i..]) as i32,
    }
}

/// `str2l()`: identical to [`str2ll`] on LP64, no end pointer.
pub fn str2l(s: &[u8]) -> i64 {
    str2ll(s).0
}

/// Digits of a power-of-two radix, after `isspace()`.
fn radix_digits(s: &[u8], bits: u32, value: impl Fn(u8) -> Option<u8>) -> (u64, usize) {
    let mut i = skip_spaces(s, 0);
    let mut n: u64 = 0;
    while let Some(v) = value(at(s, i)) {
        n = (n << bits) | u64::from(v);
        i += 1;
    }
    (n, i)
}

/// `str2uint64_hex()`: hex digits (either case), no prefix.
pub fn str2uint64_hex(s: &[u8]) -> (u64, usize) {
    radix_digits(s, 4, hex_value)
}

/// `str2uint32_hex()`: the 32-bit truncation of [`str2uint64_hex`].
pub fn str2uint32_hex(s: &[u8]) -> (u32, usize) {
    let (n, i) = str2uint64_hex(s);
    (n as u32, i)
}

/// `str2uint64_base64()`: base64 digits, no prefix.
pub fn str2uint64_base64(s: &[u8]) -> (u64, usize) {
    radix_digits(s, 6, base64_value)
}

/// `str2ull_encoded()`: `#` base64, `0x` hex, or decimal. No whitespace is
/// skipped before the prefix.
pub fn str2ull_encoded(s: &[u8]) -> u64 {
    if at(s, 0) == UINT64_B64_PREFIX[0] {
        return str2uint64_base64(&s[1..]).0;
    }
    if at(s, 0) == b'0' && at(s, 1) == b'x' {
        return str2uint64_hex(&s[2..]).0;
    }
    str2uint64(s).0
}

/// `str2ll_encoded()`: an optional `-` then [`str2ull_encoded`], wrapped
/// into the signed range.
pub fn str2ll_encoded(s: &[u8]) -> i64 {
    if at(s, 0) == b'-' {
        (str2ull_encoded(&s[1..]).wrapping_neg()) as i64
    } else {
        str2ull_encoded(s) as i64
    }
}

/// `str2ndd_parse_double_decimal_digits_internal()`: skips `isspace()`, then
/// accumulates decimal digits; the digit count excludes the skipped spaces.
fn decimal_digits_as_double(s: &[u8]) -> (f64, usize) {
    let start = skip_spaces(s, 0);
    let mut i = start;
    let mut n = 0.0f64;
    while is_digit(at(s, i)) {
        // chunks that fit an unsigned long keep the arithmetic exact
        let mut chunk: u64 = 0;
        let mut exponent: u32 = 0;
        while is_digit(at(s, i)) && chunk < u64::MAX / 10 {
            chunk = chunk * 10 + u64::from(at(s, i) - b'0');
            i += 1;
            exponent += 1;
        }
        n = n * 10f64.powf(f64::from(exponent)) + chunk as f64;
    }
    (n, i - start)
}

/// `str2ndd()`: the Agent's fast decimal parser (not correctly rounded).
///
/// Quirks kept from C: `nan`, `null` and `inf` are recognized only as the
/// first non-space bytes (`null` consumes 3 bytes); a lone sign or `.` is
/// consumed; spaces are skipped after the sign, the dot and the exponent
/// marker, but those skipped spaces are not counted in `consumed`.
pub fn str2ndd(s: &[u8]) -> (f64, usize) {
    let mut i = skip_spaces(s, 0);
    let mut sign = 1.0f64;

    match at(s, i) {
        b'-' => {
            i += 1;
            sign = -1.0;
        }
        b'+' => i += 1,
        b'n' => {
            if at(s, i + 1) == b'a' && at(s, i + 2) == b'n' {
                return (f64::NAN, i + 3);
            }
            if at(s, i + 1) == b'u' && at(s, i + 2) == b'l' && at(s, i + 3) == b'l' {
                return (f64::NAN, i + 3);
            }
        }
        b'i' if at(s, i + 1) == b'n' && at(s, i + 2) == b'f' => {
            return (f64::INFINITY, i + 3);
        }
        _ => {}
    }

    let (mut result, integral_digits) = decimal_digits_as_double(&s[i..]);
    i += integral_digits;

    let mut fractional = 0.0f64;
    let mut fractional_digits = 0;
    if at(s, i) == b'.' {
        i += 1;
        (fractional, fractional_digits) = decimal_digits_as_double(&s[i..]);
        i += fractional_digits;
    }

    let mut exponent = 0.0f64;
    let mut exponent_digits = 0;
    if matches!(at(s, i), b'e' | b'E') {
        let e_ptr = i;
        i += 1;
        let mut exponent_sign = 1.0f64;
        match at(s, i) {
            b'-' => {
                exponent_sign = -1.0;
                i += 1;
            }
            b'+' => i += 1,
            _ => {}
        }
        (exponent, exponent_digits) = decimal_digits_as_double(&s[i..]);
        if exponent_digits == 0 {
            exponent = 0.0;
            i = e_ptr;
        } else {
            i += exponent_digits;
            exponent *= exponent_sign;
        }
    }

    if exponent_digits != 0 {
        result *= 10f64.powf(exponent);
    }
    if fractional_digits != 0 {
        let scale = if exponent_digits != 0 {
            10f64.powf(exponent)
        } else {
            1.0
        };
        result += fractional / 10f64.powf(fractional_digits as f64) * scale;
    }

    (sign * result, i)
}

/// `str2ndd_encoded()`: `@` base64 or `%` hex IEEE-754 bits, or an optional
/// `-` followed by `#` base64 / `0x` hex integers, or [`str2ndd`].
pub fn str2ndd_encoded(s: &[u8]) -> (f64, usize) {
    if at(s, 0) == DOUBLE_B64_PREFIX[0] {
        let (bits, used) = str2uint64_base64(&s[1..]);
        return (f64::from_bits(bits), 1 + used);
    }
    if at(s, 0) == DOUBLE_HEX_PREFIX[0] {
        let (bits, used) = str2uint64_hex(&s[1..]);
        return (f64::from_bits(bits), 1 + used);
    }

    let (sign, i) = if at(s, 0) == b'-' {
        (-1.0f64, 1)
    } else {
        (1.0, 0)
    };

    if at(s, i) == UINT64_B64_PREFIX[0] {
        let (n, used) = str2uint64_base64(&s[i + 1..]);
        return (n as f64 * sign, i + 1 + used);
    }
    if at(s, i) == b'0' && at(s, i + 1) == b'x' {
        let (n, used) = str2uint64_hex(&s[i + 2..]);
        return (n as f64 * sign, i + 2 + used);
    }

    let (v, used) = str2ndd(&s[i..]);
    (v * sign, i + used)
}

// ------------------------------------------------------------------------------------------------
// strtod

/// Case-insensitive ASCII prefix test.
fn starts_with_ignore_case(s: &[u8], prefix: &[u8]) -> bool {
    s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Scans `digit* [. digit*]` and returns (end, has_digits).
fn scan_mantissa(s: &[u8], mut i: usize, is_digit: impl Fn(u8) -> bool) -> (usize, bool) {
    let mut any = false;
    while is_digit(at(s, i)) {
        i += 1;
        any = true;
    }
    if at(s, i) == b'.' {
        let mut j = i + 1;
        let mut frac = false;
        while is_digit(at(s, j)) {
            j += 1;
            frac = true;
        }
        if any || frac {
            i = j;
            any = true;
        }
    }
    (i, any)
}

/// Scans an optional exponent `marker [+-] digit+`; returns its end and the
/// (saturated) value, or `(i, 0)` when there is none.
fn scan_exponent(s: &[u8], i: usize, marker: u8) -> (usize, i64) {
    if at(s, i) | 0x20 != marker {
        return (i, 0);
    }
    let mut j = i + 1;
    let negative = match at(s, j) {
        b'-' => {
            j += 1;
            true
        }
        b'+' => {
            j += 1;
            false
        }
        _ => false,
    };
    if !is_digit(at(s, j)) {
        return (i, 0);
    }
    let mut e: i64 = 0;
    while is_digit(at(s, j)) {
        e = (e * 10 + i64::from(at(s, j) - b'0')).min(1 << 40);
        j += 1;
    }
    (j, if negative { -e } else { e })
}

/// glibc `__strtod_nan()`: the n-char-sequence inside `nan(...)` becomes the
/// NaN payload when it is a valid `strtoull(..., 0)` number.
fn nan_payload(seq: &[u8]) -> Option<u64> {
    let (radix, digits) = if seq.len() > 1 && seq[0] == b'0' && (seq[1] | 0x20) == b'x' {
        if seq.len() == 2 || !seq[2..].iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        (16, &seq[2..])
    } else if seq.first() == Some(&b'0') {
        (8, seq)
    } else {
        (10, seq)
    };
    if !digits.iter().all(|&d| char::from(d).is_digit(radix)) {
        return None;
    }
    // strtoull() saturates on overflow
    let value = digits.iter().try_fold(0u64, |n, &d| {
        n.checked_mul(u64::from(radix))?
            .checked_add(u64::from(char::from(d).to_digit(radix)?))
    });
    Some(value.unwrap_or(u64::MAX))
}

/// Rounds `mantissa * 2^exp2` (plus a sticky bit for discarded non-zero
/// digits) to the nearest double, ties to even, with gradual underflow.
fn hex_to_double(mut mantissa: u64, mut exp2: i64, sticky: bool) -> f64 {
    if mantissa == 0 {
        return 0.0;
    }
    let lz = mantissa.leading_zeros();
    mantissa <<= lz;
    exp2 -= i64::from(lz);
    // value = mantissa / 2^63 * 2^e
    let e = exp2 + 63;
    if e > 1023 {
        return f64::INFINITY;
    }
    // significant bits available at this magnitude
    let bits = if e >= -1022 { 53 } else { e + 1075 };
    if bits < 0 {
        return 0.0;
    }
    let (keep, round_up) = if bits == 0 {
        (0u64, mantissa > 1 << 63 || (mantissa == 1 << 63 && sticky))
    } else {
        let drop = 64 - bits as u32;
        let keep = mantissa >> drop;
        let rest = mantissa & ((1u64 << drop) - 1);
        let half = 1u64 << (drop - 1);
        (
            keep,
            rest > half || (rest == half && (sticky || keep & 1 == 1)),
        )
    };
    let keep = keep + u64::from(round_up);
    let raw = if e >= -1022 {
        // the implicit leading bit of `keep` carries into the exponent field
        (((e + 1022) as u64) << 52) + keep
    } else {
        keep
    };
    if raw >= f64::INFINITY.to_bits() {
        f64::INFINITY
    } else {
        f64::from_bits(raw)
    }
}

/// Value of the hex float mantissa `s` (hex digits and at most one dot)
/// with binary exponent `exp2`.
fn hex_mantissa_value(s: &[u8], exp2: i64) -> f64 {
    let mut mantissa: u64 = 0;
    let mut used = 0; // significant digits kept in `mantissa`
    let mut sticky = false;
    let mut exp = exp2;
    let mut after_dot = false;
    for &ch in s {
        if ch == b'.' {
            after_dot = true;
            continue;
        }
        let d = u64::from(hex_value(ch).unwrap_or(0));
        if used == 0 && d == 0 {
            if after_dot {
                exp = exp.saturating_sub(4);
            }
            continue;
        }
        if used < 16 {
            mantissa = (mantissa << 4) | d;
            used += 1;
            if after_dot {
                exp = exp.saturating_sub(4);
            }
        } else {
            sticky |= d != 0;
            if !after_dot {
                exp = exp.saturating_add(4);
            }
        }
    }
    hex_to_double(mantissa, exp, sticky)
}

/// glibc `strtod()` in the "C" locale: skips `isspace()`, accepts decimal and
/// hexadecimal floats, `inf`/`infinity` and `nan`/`nan(n-char-sequence)`
/// (case-insensitive), and returns the correctly rounded value with the
/// length of the longest valid prefix (0 when nothing was converted).
pub fn strtod(s: &[u8]) -> (f64, usize) {
    let s = c::c_str(s);
    let mut i = skip_spaces(s, 0);
    let negative = match at(s, i) {
        b'-' => {
            i += 1;
            true
        }
        b'+' => {
            i += 1;
            false
        }
        _ => false,
    };
    let signed = |v: f64| if negative { -v } else { v };
    let rest = &s[i..];

    if starts_with_ignore_case(rest, b"inf") {
        let len = if starts_with_ignore_case(rest, b"infinity") {
            8
        } else {
            3
        };
        return (signed(f64::INFINITY), i + len);
    }

    if starts_with_ignore_case(rest, b"nan") {
        let mut end = i + 3;
        let mut value = f64::NAN;
        if at(s, end) == b'(' {
            let seq_start = end + 1;
            let mut j = seq_start;
            while at(s, j).is_ascii_alphanumeric() || at(s, j) == b'_' {
                j += 1;
            }
            if at(s, j) == b')' {
                if let Some(payload) = nan_payload(&s[seq_start..j]) {
                    value = f64::from_bits(f64::NAN.to_bits() | (payload & ((1 << 51) - 1)));
                }
                end = j + 1;
            }
        }
        return (signed(value), end);
    }

    if at(s, i) == b'0' && at(s, i + 1) | 0x20 == b'x' {
        let (end, any) = scan_mantissa(s, i + 2, |ch| ch.is_ascii_hexdigit());
        if any {
            let (end_exp, exp2) = scan_exponent(s, end, b'p');
            return (signed(hex_mantissa_value(&s[i + 2..end], exp2)), end_exp);
        }
        // "0x" without hex digits: only the "0" is a number
        return (signed(0.0), i + 1);
    }

    let (end, any) = scan_mantissa(s, i, is_digit);
    if !any {
        return (0.0, 0);
    }
    let (end_exp, _) = scan_exponent(s, end, b'e');
    // The prefix is valid decimal float syntax; std's parser is correctly
    // rounded like glibc's.
    let text = std::str::from_utf8(&s[i..end_exp]).unwrap_or("0");
    let value = text.parse::<f64>().unwrap_or(0.0);
    (signed(value), end_exp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_rounding() {
        let cases: &[(&[u8], u64, usize)] = &[
            (b"0x1p0", 1.0f64.to_bits(), 5),
            (b"0x1.8p1", 3.0f64.to_bits(), 7),
            (b"0x", 0, 1),
            (b"0x.p1", 0, 1),
            (b"0x1p", 1.0f64.to_bits(), 3),
            (b"0x1p-1074", 1, 9),
            (b"0x1p-1075", 0, 9),
            (b"0x1.8p-1075", 1, 11),
            (b"0x1.fffffffffffff8p0", 2.0f64.to_bits(), 20),
            (b"0x1p1024", f64::INFINITY.to_bits(), 8),
        ];
        for &(input, bits, used) in cases {
            let (v, n) = strtod(input);
            assert_eq!(
                (v.to_bits(), n),
                (bits, used),
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }
}
