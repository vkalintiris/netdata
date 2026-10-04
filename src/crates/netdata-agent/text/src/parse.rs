//! Number parsing, mirroring the parsers in `libnetdata/inlined.h` and glibc
//! `strtod()` in the "C" locale.
//!
//! All parsers take bytes and treat the end of the slice, or a NUL byte, as the
//! C string terminator. Functions that have an `endptr` in C return
//! `(value, consumed)`, where `consumed` is `endptr - input` in bytes. Integer
//! accumulation wraps on overflow exactly like the unsigned C arithmetic.

use std::cmp::Ordering;
use std::iter::repeat;

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

/// `pow(10.0, x)` as `str2ndd()` calls it: glibc's `pow()` sets `errno` to `ERANGE` when its result overflows or
/// underflows to zero (a subnormal result sets nothing).
fn pow10(x: f64) -> f64 {
    let power = 10f64.powf(x);
    if power == 0.0 || power.is_infinite() {
        c::set_errno(c::ERANGE);
    }
    power
}

/// `str2ndd()`: the Agent's fast decimal parser (not correctly rounded).
///
/// Quirks kept from C: `nan`, `null` and `inf` are recognized only as the
/// first non-space bytes (`null` consumes 3 bytes); a lone sign or `.` is
/// consumed. Spaces after the sign, the dot or the exponent marker are
/// skipped but not counted, so the parse resumes inside the number and later
/// parts are dropped: `"- 1.5"` is -1 (2 bytes), `"1. 5e3"` is 1.5 (3 bytes).
/// An exponent or a fraction too long for `pow()` leaves `ERANGE` in the C
/// `errno` ([`c::set_errno`]).
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
        result *= pow10(exponent);
    }
    if fractional_digits != 0 {
        let scale = if exponent_digits != 0 {
            pow10(exponent)
        } else {
            1.0
        };
        result += fractional / pow10(fractional_digits as f64) * scale;
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

/// A binary floating point format: the bits of its significand, and its largest exponent.
#[derive(Clone, Copy)]
struct Format {
    precision: i64,
    emax: i64,
}

const DOUBLE: Format = Format { precision: 53, emax: 1023 };
const FLOAT: Format = Format { precision: 24, emax: 127 };

/// glibc's `TININESS_AFTER_ROUNDING`: x86 finds a result tiny after rounding
/// it to the type's full precision, so a number that rounds up to the
/// smallest normal value is not tiny there; ARM finds it before rounding.
const TININESS_AFTER_ROUNDING: bool = cfg!(any(target_arch = "x86", target_arch = "x86_64"));

/// Rounds `mantissa * 2^exp2` (plus a sticky bit for discarded non-zero
/// digits) to the nearest value of `format`, ties to even, with gradual
/// underflow: the value's bits, and whether glibc reports a range error for
/// it (an overflow, or a tiny result that is not exact).
fn hex_to_bits(mut mantissa: u64, mut exp2: i64, sticky: bool, format: Format) -> (u64, bool) {
    if mantissa == 0 {
        return (0, false);
    }
    let lz = mantissa.leading_zeros();
    mantissa <<= lz;
    exp2 -= i64::from(lz);
    // value = mantissa / 2^63 * 2^e
    let e = exp2 + 63;
    let emin = 1 - format.emax;
    let infinity = ((2 * format.emax + 1) as u64) << (format.precision - 1);
    if e > format.emax {
        return (infinity, true);
    }
    // the mantissa's top `bits` bits, whether they round up, and whether the rest is not zero
    let round = |bits: i64| {
        let drop = 64 - bits as u32;
        let keep = mantissa >> drop;
        let rest = mantissa & ((1u64 << drop) - 1);
        let half = 1u64 << (drop - 1);
        (
            keep,
            rest > half || (rest == half && (sticky || keep & 1 == 1)),
            rest != 0 || sticky,
        )
    };
    // significant bits available at this magnitude
    let bits = if e >= emin { format.precision } else { e - emin + format.precision };
    if bits < 0 {
        return (0, true);
    }
    let (keep, round_up, inexact) = if bits == 0 {
        (0u64, mantissa > 1 << 63 || (mantissa == 1 << 63 && sticky), true)
    } else {
        round(bits)
    };
    let tiny = e < emin
        && !(TININESS_AFTER_ROUNDING && e == emin - 1 && {
            let (keep, round_up, _) = round(format.precision);
            round_up && keep == (1u64 << format.precision) - 1
        });
    let keep = keep + u64::from(round_up);
    let raw = if e >= emin {
        // the implicit leading bit of `keep` carries into the exponent field
        (((e - emin) as u64) << (format.precision - 1)) + keep
    } else {
        keep
    };
    (raw.min(infinity), raw >= infinity || (tiny && inexact))
}

/// The hex float mantissa `s` (hex digits and at most one dot) with binary
/// exponent `exp2`, as an integer mantissa, its exponent and a sticky bit.
fn hex_mantissa(s: &[u8], exp2: i64) -> (u64, i64, bool) {
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
    (mantissa, exp, sticky)
}

/// The decimal mantissa `s` (digits and at most one dot) times `10^exp10`.
enum Decimal<'a> {
    Zero,
    /// Too small for any exponent.
    Underflow,
    Infinity,
    Number(Digits<'a>),
}

/// A decimal number: its significant digits (ASCII; neither the first nor
/// the last is a zero) and the power of ten the first one weighs.
struct Digits<'a> {
    digits: &'a [u8],
    exponent: i64,
}

impl Digits<'_> {
    /// As text std's parsers round correctly, like glibc's: std caps the
    /// exponent it reads, so the digits are normalized to `d.ddd` with the
    /// (saturated) decimal exponent of the leading significant digit.
    fn text(&self) -> String {
        let mut text = String::with_capacity(self.digits.len() + 24);
        text.push(char::from(self.digits[0]));
        text.push('.');
        text.extend(self.digits[1..].iter().map(|&d| char::from(d)));
        text.push_str(&format!("e{}", self.exponent));
        text
    }

    /// The number compared with `mantissa / 2^shift` (`mantissa` not zero).
    fn cmp_dyadic(&self, mantissa: u32, shift: u32) -> Ordering {
        // mantissa / 2^shift is mantissa * 5^shift / 10^shift: its digits, the least significant first
        let mut product: Vec<u8> = mantissa.to_string().bytes().rev().map(|d| d - b'0').collect();
        for _ in 0..shift {
            let mut carry = 0;
            for digit in &mut product {
                let times_five = *digit * 5 + carry;
                *digit = times_five % 10;
                carry = times_five / 10;
            }
            if carry != 0 {
                product.push(carry);
            }
        }
        let exponent = product.len() as i64 - 1 - i64::from(shift);
        self.exponent.cmp(&exponent).then_with(|| {
            let len = self.digits.len().max(product.len());
            let ours = self.digits.iter().map(|d| d - b'0').chain(repeat(0));
            let theirs = product.iter().rev().copied().chain(repeat(0));
            ours.take(len).cmp(theirs.take(len))
        })
    }

    /// Whether glibc's `strtof()` reports a range error for this number,
    /// which rounds to `value`: an overflow, or a tiny result that is not
    /// exact.
    fn float_range_error(&self, value: f32) -> bool {
        if value.is_infinite() || value == 0.0 {
            return true;
        }
        match value.partial_cmp(&f32::MIN_POSITIVE) {
            // a subnormal value's bits count units of 2^-149
            Some(Ordering::Less) => self.cmp_dyadic(value.to_bits(), 149) != Ordering::Equal,
            // rounded up to the smallest normal value, 2^-126, the number is not exact; it is tiny when
            // below it, or (x86) below 2^-126 - 2^-151, where rounding to 24 bits reaches 2^-126 too
            Some(Ordering::Equal) => {
                let (mantissa, shift) = if TININESS_AFTER_ROUNDING { ((1 << 25) - 1, 151) } else { (1, 126) };
                self.cmp_dyadic(mantissa, shift) == Ordering::Less
            }
            _ => false,
        }
    }
}

/// `digits` is the caller's buffer for the mantissa without its dot.
fn decimal_mantissa<'a>(s: &[u8], exp10: i64, digits: &'a mut Vec<u8>) -> Decimal<'a> {
    let point = s.iter().position(|&b| b == b'.').unwrap_or(s.len());
    digits.clear();
    digits.extend(s.iter().copied().filter(|&b| b != b'.'));
    let Some(first) = digits.iter().position(|&d| d != b'0') else {
        return Decimal::Zero;
    };
    let last = digits.iter().rposition(|&d| d != b'0').unwrap_or(first);

    // the leading significant digit weighs 10^exponent
    let exponent = (point as i64 - 1 - first as i64).saturating_add(exp10);
    if exponent > 400 {
        return Decimal::Infinity;
    }
    if exponent < -400 {
        return Decimal::Underflow;
    }

    Decimal::Number(Digits { digits: &digits[first..=last], exponent })
}

/// What `strtod()` and `strtof()` read, before it is rounded to their type.
enum Scanned<'a> {
    /// Nothing was converted.
    Nothing,
    Infinity,
    Nan(Option<u64>),
    /// "0x" without hex digits: only the "0" is a number.
    Zero,
    Hex(&'a [u8], i64),
    Decimal(&'a [u8], i64),
}

/// glibc's grammar in the "C" locale: whether the number is negative, what it
/// is, and the length of the longest valid prefix.
fn scan_float(s: &[u8]) -> (bool, Scanned<'_>, usize) {
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
    let rest = &s[i..];

    if starts_with_ignore_case(rest, b"inf") {
        let len = if starts_with_ignore_case(rest, b"infinity") {
            8
        } else {
            3
        };
        return (negative, Scanned::Infinity, i + len);
    }

    if starts_with_ignore_case(rest, b"nan") {
        let mut end = i + 3;
        let mut payload = None;
        if at(s, end) == b'(' {
            let seq_start = end + 1;
            let mut j = seq_start;
            while at(s, j).is_ascii_alphanumeric() || at(s, j) == b'_' {
                j += 1;
            }
            if at(s, j) == b')' {
                payload = nan_payload(&s[seq_start..j]);
                end = j + 1;
            }
        }
        return (negative, Scanned::Nan(payload), end);
    }

    if at(s, i) == b'0' && at(s, i + 1) | 0x20 == b'x' {
        let (end, any) = scan_mantissa(s, i + 2, |ch| ch.is_ascii_hexdigit());
        if any {
            let (end_exp, exp2) = scan_exponent(s, end, b'p');
            return (negative, Scanned::Hex(&s[i + 2..end], exp2), end_exp);
        }
        return (negative, Scanned::Zero, i + 1);
    }

    let (end, any) = scan_mantissa(s, i, is_digit);
    if !any {
        return (false, Scanned::Nothing, 0);
    }
    let (end_exp, exp10) = scan_exponent(s, end, b'e');
    (negative, Scanned::Decimal(&s[i..end], exp10), end_exp)
}

/// glibc `strtod()` in the "C" locale: skips `isspace()`, accepts decimal and
/// hexadecimal floats, `inf`/`infinity` and `nan`/`nan(n-char-sequence)`
/// (case-insensitive), and returns the correctly rounded value with the
/// length of the longest valid prefix (0 when nothing was converted).
pub fn strtod(s: &[u8]) -> (f64, usize) {
    let (negative, scanned, len) = scan_float(c::c_str(s));
    let value = match scanned {
        Scanned::Nothing | Scanned::Zero => 0.0,
        Scanned::Infinity => f64::INFINITY,
        Scanned::Nan(payload) => {
            f64::from_bits(f64::NAN.to_bits() | (payload.unwrap_or(0) & ((1 << 51) - 1)))
        }
        Scanned::Hex(mantissa, exp2) => {
            let (mantissa, exp, sticky) = hex_mantissa(mantissa, exp2);
            f64::from_bits(hex_to_bits(mantissa, exp, sticky, DOUBLE).0)
        }
        Scanned::Decimal(mantissa, exp10) => match decimal_mantissa(mantissa, exp10, &mut Vec::new()) {
            Decimal::Zero | Decimal::Underflow => 0.0,
            Decimal::Infinity => f64::INFINITY,
            Decimal::Number(number) => number.text().parse().unwrap_or(0.0),
        },
    };
    (if negative { -value } else { value }, len)
}

/// glibc `strtof()` in the "C" locale: [`strtod`]'s grammar, rounded to
/// `f32` once (converting through a double would round twice).
///
/// As glibc, it leaves `ERANGE` in the C `errno` ([`c::set_errno`]) when a
/// number overflows, and when its result is tiny (below the smallest normal
/// value) and not exact.
pub fn strtof(s: &[u8]) -> (f32, usize) {
    let (negative, scanned, len) = scan_float(c::c_str(s));
    let (value, range_error) = match scanned {
        Scanned::Nothing | Scanned::Zero => (0.0, false),
        Scanned::Infinity => (f32::INFINITY, false),
        Scanned::Nan(payload) => (
            f32::from_bits(f32::NAN.to_bits() | (payload.unwrap_or(0) & ((1 << 22) - 1)) as u32),
            false,
        ),
        Scanned::Hex(mantissa, exp2) => {
            let (mantissa, exp, sticky) = hex_mantissa(mantissa, exp2);
            let (bits, range_error) = hex_to_bits(mantissa, exp, sticky, FLOAT);
            (f32::from_bits(bits as u32), range_error)
        }
        Scanned::Decimal(mantissa, exp10) => match decimal_mantissa(mantissa, exp10, &mut Vec::new()) {
            Decimal::Zero => (0.0, false),
            Decimal::Underflow => (0.0, true),
            Decimal::Infinity => (f32::INFINITY, true),
            Decimal::Number(number) => {
                let value: f32 = number.text().parse().unwrap_or(0.0);
                (value, number.float_range_error(value))
            }
        },
    };
    if range_error {
        c::set_errno(c::ERANGE);
    }
    (if negative { -value } else { value }, len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strtoul0_matches_glibc() {
        let cases: [(&[u8], (u64, usize)); 7] = [
            (b" -1", (u64::MAX, 3)),
            (b"0x10z", (16, 4)),
            (b"010", (8, 3)),
            (b"0x", (0, 1)),
            (b"18446744073709551616", (u64::MAX, 20)),
            (b"-18446744073709551616", (u64::MAX, 21)),
            (b"x", (0, 0)),
        ];
        for (input, expected) in cases {
            assert_eq!(
                strtoul0(input),
                expected,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn strtoull16_matches_glibc() {
        let cases: [(&[u8], (u64, usize)); 7] = [
            (b"0x1f", (31, 4)),
            (b" 1F ", (31, 3)),
            (b"-1", (u64::MAX, 2)),
            (b"0xg", (0, 1)),
            (b"x1", (0, 0)),
            (b"10000000000000000", (u64::MAX, 17)),
            (b"", (0, 0)),
        ];
        for (input, expected) in cases {
            assert_eq!(
                strtoull16(input),
                expected,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }

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

/// `strtoull(s, &end, 10)` in the "C" locale: leading `isspace()`, an optional sign (a `-` negates modulo 2^64),
/// then decimal digits. Returns `(value, consumed, erange)`; on overflow the value is `u64::MAX` and `erange` is
/// set, as glibc sets `errno = ERANGE`. With no digits nothing is consumed.
pub fn strtoull10(s: &[u8]) -> (u64, usize, bool) {
    match scan_base(s, 10) {
        None => (0, 0, false),
        Some((_, _, true, used)) => (u64::MAX, used, true),
        Some((negative, magnitude, false, used)) => (
            if negative {
                magnitude.wrapping_neg()
            } else {
                magnitude
            },
            used,
            false,
        ),
    }
}

/// `strtoll(s, &end, 10)` in the "C" locale: leading `isspace()`, an optional sign, then decimal digits. Returns
/// `(value, consumed, erange)`; out-of-range values clamp to `i64::MAX`/`i64::MIN` with `erange` set, as glibc sets
/// `errno = ERANGE`. With no digits nothing is consumed.
pub fn strtoll10(s: &[u8]) -> (i64, usize, bool) {
    let Some((negative, magnitude, overflow, used)) = scan_base(s, 10) else {
        return (0, 0, false);
    };
    let (value, erange) = clamp_signed(negative, magnitude, overflow);
    (value, used, erange)
}

/// `strtol(s, &end, 16)` in the "C" locale: as [`strtoll10`] in base 16, with an optional `0x`.
pub fn strtoll16(s: &[u8]) -> (i64, usize, bool) {
    let Some((negative, magnitude, overflow, used)) = scan_base(s, 16) else {
        return (0, 0, false);
    };
    let (value, erange) = clamp_signed(negative, magnitude, overflow);
    (value, used, erange)
}

/// A signed result of a scan: out-of-range magnitudes clamp to `i64::MAX` or `i64::MIN` (the `bool` is ERANGE).
fn clamp_signed(negative: bool, magnitude: u64, overflow: bool) -> (i64, bool) {
    let limit: u64 = if negative {
        1u64 << 63
    } else {
        i64::MAX as u64
    };
    if overflow || magnitude > limit {
        (if negative { i64::MIN } else { i64::MAX }, true)
    } else if negative {
        ((magnitude as i64).wrapping_neg(), false)
    } else {
        (magnitude as i64, false)
    }
}

/// The part `strtol()`, `strtoll()` and `strtoul()` share for base 0, 10 or 16 in the "C" locale: leading
/// `isspace()`, an optional sign, then digits. Base 10 takes decimal digits only; base 16 and 0 take a `0x`/`0X`
/// prefix (only when a hex digit follows) and hex digits; base 0 otherwise takes a leading `0` as octal, or decimal.
/// Returns `(negative, magnitude, overflowed u64, consumed)`, or `None` without digits (nothing is consumed).
fn scan_base(s: &[u8], base: u32) -> Option<(bool, u64, bool, usize)> {
    debug_assert!(base == 0 || base == 10 || base == 16);
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
    let (base, start) = if base == 10 {
        (10u32, i)
    } else if at(s, i) == b'0'
        && matches!(at(s, i + 1), b'x' | b'X')
        && at(s, i + 2).is_ascii_hexdigit()
    {
        (16u32, i + 2)
    } else if base == 16 {
        (16, i)
    } else if at(s, i) == b'0' {
        (8, i)
    } else {
        (10, i)
    };
    let digit = |c: u8| char::from(c).to_digit(base);
    digit(at(s, start))?;
    let mut magnitude: u64 = 0;
    let mut overflow = false;
    let mut j = start;
    while let Some(d) = digit(at(s, j)) {
        match magnitude
            .checked_mul(u64::from(base))
            .and_then(|m| m.checked_add(u64::from(d)))
        {
            Some(m) if !overflow => magnitude = m,
            _ => overflow = true,
        }
        j += 1;
    }
    Some((negative, magnitude, overflow, j))
}

/// `strtoll(s, NULL, 0)` in the "C" locale (see `scan_base`). Out-of-range values clamp to `i64::MAX`/`i64::MIN`
/// (glibc sets `ERANGE`). Returns `(value, consumed)`; with no digits nothing is consumed.
pub fn strtoll0(s: &[u8]) -> (i64, usize) {
    let Some((negative, magnitude, overflow, used)) = scan_base(s, 0) else {
        return (0, 0);
    };
    (clamp_signed(negative, magnitude, overflow).0, used)
}

/// The unsigned result of a scan: a `-` negates modulo 2^64; overflow of either sign is `u64::MAX`.
fn unsigned(scan: Option<(bool, u64, bool, usize)>) -> (u64, usize) {
    match scan {
        None => (0, 0),
        Some((_, _, true, used)) => (u64::MAX, used),
        Some((true, magnitude, false, used)) => (magnitude.wrapping_neg(), used),
        Some((false, magnitude, false, used)) => (magnitude, used),
    }
}

/// `strtoull(s, NULL, 16)` in the "C" locale (see `scan_base`): a `-` negates modulo 2^64; out-of-range values of
/// either sign are `u64::MAX` (glibc sets `ERANGE`). Returns `(value, consumed)`.
pub fn strtoull16(s: &[u8]) -> (u64, usize) {
    unsigned(scan_base(s, 16))
}

/// `strtoul(s, NULL, 0)` on LP64 in the "C" locale (see `scan_base`): a `-` negates modulo 2^64; out-of-range
/// values of either sign are `u64::MAX` (glibc sets `ERANGE`). Returns `(value, consumed)`.
pub fn strtoul0(s: &[u8]) -> (u64, usize) {
    unsigned(scan_base(s, 0))
}

/// `uuid_parse_flexi()`: 32 hex digits with either no hyphens or exactly four anywhere between byte pairs;
/// parsing stops after 16 bytes, so trailing text is ignored. `None` where C returns an error (and leaves the
/// destination untouched).
pub fn uuid_parse_flexi(s: &[u8]) -> Option<[u8; 16]> {
    let s = c::c_str(s);
    if s.is_empty() {
        return None;
    }
    let mut uuid = [0u8; 16];
    let (mut i, mut hyphens, mut hex_chars, mut byte) = (0usize, 0usize, 0usize, 0usize);
    while at(s, i) != 0 && byte < 16 {
        if at(s, i) == b'-' {
            i += 1;
            hyphens += 1;
            if hyphens > 4 {
                return None;
            }
        }
        let high = hex_value(at(s, i))?;
        i += 1;
        hex_chars += 1;
        let low = hex_value(at(s, i))?;
        i += 1;
        hex_chars += 1;
        uuid[byte] = (high << 4) | low;
        byte += 1;
    }
    if byte < 16 || hex_chars != 32 || (hyphens != 0 && hyphens != 4) {
        return None;
    }
    Some(uuid)
}

#[cfg(test)]
mod uuid_and_strtoull_tests {
    use super::*;

    /// What C's records showed after each number (`health/tests/corpus/keys/errno.conf`, a `delay: multiplier`).
    #[test]
    fn strtof_range_errors_match_glibc() {
        // 2^-149, the smallest float, in all its digits, and one unit more in the last of them
        const SMALLEST: &str = "1.40129846432481707092372958328991613128026194187651577175706828388979108268586060148663818836212158203125e-45";
        let inexact = SMALLEST.replace("125e-45", "126e-45");
        let cases: [(&str, bool); 38] = [
            ("1e39", true),
            ("3e38", false),
            ("4e38", true),
            ("3.5e38", true),
            ("1e-37", false),
            ("1e-38", true),
            ("1e-40", true),
            ("1e-45", true),
            ("1e-46", true),
            ("0x1p-149", false),
            ("0x1p-150", true),
            ("0x1p-126", false),
            ("0x1.000002p-126", false),
            ("0x1p128", true),
            ("inf", false),
            ("nan", false),
            ("1e400", true),
            ("-1e39", true),
            ("0", false),
            ("2", false),
            // below the smallest normal float and rounded up to it: tiny, unless 24 bits round to it as well
            ("1.17549430e-38", true),
            ("1.17549431e-38", true),
            ("1.17549432e-38", false),
            ("1.17549435e-38", false),
            (SMALLEST, false),
            (&inexact, true),
            ("0x0.ffffffp-126", true),
            ("0x0.ffffff4p-126", true),
            ("0x0.ffffff8p-126", false),
            ("0x0.fffffep-126", false),
            ("0x0.000002p-126", false),
            ("0x0.000003p-126", true),
            ("0x1.fffffep127", false),
            ("0x1.fffffe7p127", false),
            ("0x1.fffffe8p127", false),
            ("0x1.ffffffp127", true),
            ("3.4028235e38", false),
            ("3.4028236e38", true),
        ];
        for (input, range_error) in cases {
            c::take_errno();
            strtof(input.as_bytes());
            assert_eq!(c::take_errno(), if range_error { c::ERANGE } else { 0 }, "{input}");
        }
    }

    /// What C's records showed after each number (the same file, `green`): `pow()` overflowing or reaching zero.
    #[test]
    fn str2ndd_range_errors_match_glibc() {
        let fraction = format!("0.{}", "1".repeat(400));
        let integer = "1".repeat(400);
        let cases: [(&str, bool); 14] = [
            ("1e400", true),
            ("1e309", true),
            ("1e308", false),
            ("1e-307", false),
            ("1e-308", false),
            ("1e-320", false),
            ("1e-323", false),
            ("1e-324", true),
            ("1e-400", true),
            (&fraction, true),
            (&integer, false),
            ("1e99999999999999999999", true),
            ("1e-99999999999999999999", true),
            ("5", false),
        ];
        for (input, range_error) in cases {
            c::take_errno();
            str2ndd(input.as_bytes());
            assert_eq!(c::take_errno(), if range_error { c::ERANGE } else { 0 }, "{input}");
        }
        // the errno stays until it is taken
        str2ndd(b"1e400");
        str2ndd(b"5");
        assert_eq!(c::take_errno(), c::ERANGE);
        assert_eq!(c::take_errno(), 0);
    }

    #[test]
    fn strtoull10_matches_glibc() {
        type Case = (&'static [u8], (u64, usize, bool));
        let cases: [Case; 7] = [
            (b"123\r\n", (123, 3, false)),
            (b"  +7x", (7, 4, false)),
            (b"-1", (u64::MAX, 2, false)),
            (b"18446744073709551615", (u64::MAX, 20, false)),
            (b"18446744073709551616", (u64::MAX, 20, true)),
            (b"99999999999999999999999 ", (u64::MAX, 23, true)),
            (b"x1", (0, 0, false)),
        ];
        for (input, want) in cases {
            assert_eq!(
                strtoull10(input),
                want,
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    /// The spec's strtoll probes (`evidence/2026-09-26-sp1-jsonc-spec.md` §3.1 in the status repository).
    #[test]
    fn strtoll10_matches_glibc() {
        type Case = (&'static [u8], (i64, usize, bool));
        let cases: [Case; 9] = [
            (b"010", (10, 3, false)),
            (b"\t12", (12, 3, false)),
            (b"0x10", (0, 1, false)),
            (b"-0", (0, 2, false)),
            (b"12 ", (12, 2, false)),
            (b"+-1", (0, 0, false)),
            (b"9223372036854775808", (i64::MAX, 19, true)),
            (b"-9223372036854775808", (i64::MIN, 20, false)),
            (b"-9223372036854775809", (i64::MIN, 20, true)),
        ];
        for (input, want) in cases {
            assert_eq!(
                strtoll10(input),
                want,
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn strtoll0_matches_glibc() {
        type Case = (&'static [u8], (i64, usize));
        let cases: [Case; 10] = [
            (b"42", (42, 2)),
            (b"  -17x", (-17, 5)),
            (b"0x1F", (31, 4)),
            (b"0xg", (0, 1)),
            (b"010", (8, 3)),
            (b"09", (0, 1)),
            (b"9223372036854775807", (i64::MAX, 19)),
            (b"9223372036854775808", (i64::MAX, 19)),
            (b"-9223372036854775808", (i64::MIN, 20)),
            (b"abc", (0, 0)),
        ];
        for (input, want) in cases {
            assert_eq!(
                strtoll0(input),
                want,
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn uuid_parse_flexi_matches_c() {
        let want = [
            0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc,
            0xde, 0xf0,
        ];
        let cases: [(&[u8], Option<[u8; 16]>); 9] = [
            (b"12345678-9abc-def0-1234-56789abcdef0", Some(want)),
            (b"123456789ABCDEF0123456789abcdef0", Some(want)),
            (b"123456789abcdef0123456789abcdef0trailing", Some(want)),
            (b"12-345678-9abcdef0123456789abc-def-0", None), // hyphen splits a byte pair
            (b"1234-5678-9abc-def0-1234-56789abcdef0", None), // five hyphens
            (b"12345678-9abcdef0123456789abcdef0", None),    // one hyphen
            (b"12345678-9abc-def0-1234-56789abcde", None),   // short
            (b"", None),
            (b"g2345678-9abc-def0-1234-56789abcdef0", None),
        ];
        for (input, want) in cases {
            assert_eq!(
                uuid_parse_flexi(input),
                want,
                "{:?}",
                String::from_utf8_lossy(input)
            );
        }
    }
}
