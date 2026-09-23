//! Number printing, mirroring the printers in `libnetdata/buffer/buffer.h`.
//!
//! Every function appends ASCII to a byte buffer, exactly like the C
//! `buffer_print_*()` functions append to a `BUFFER`.

use crate::c;

/// Uppercase hex digits (`hex_digits` in `buffer.c`).
pub const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";
/// Lowercase hex digits (`hex_digits_lower` in `buffer.c`).
pub const HEX_DIGITS_LOWER: &[u8; 16] = b"0123456789abcdef";
/// The base64 alphabet of the streaming number encodings (`base64_digits`).
pub const BASE64_DIGITS: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Prefix of hex encoded integers (`HEX_PREFIX`).
pub const HEX_PREFIX: &[u8] = b"0x";
/// Prefix of base64 encoded integers (`IEEE754_UINT64_B64_PREFIX`).
pub const UINT64_B64_PREFIX: &[u8] = b"#";
/// Prefix of base64 encoded IEEE-754 doubles (`IEEE754_DOUBLE_B64_PREFIX`).
pub const DOUBLE_B64_PREFIX: &[u8] = b"@";
/// Prefix of hex encoded IEEE-754 doubles (`IEEE754_DOUBLE_HEX_PREFIX`).
pub const DOUBLE_HEX_PREFIX: &[u8] = b"%";

/// Number encodings of the streaming protocol (`NUMBER_ENCODING`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberEncoding {
    Decimal,
    Hex,
    Base64,
}

/// Appends the digits of `value` in `radix` (a power of two or 10), most
/// significant first; at least one digit is written.
fn push_digits(dst: &mut Vec<u8>, mut value: u64, radix: u64, digits: &[u8]) {
    let start = dst.len();
    loop {
        dst.push(digits[(value % radix) as usize]);
        value /= radix;
        if value == 0 {
            break;
        }
    }
    dst[start..].reverse();
}

/// `print_uint64()`.
pub fn print_uint64(dst: &mut Vec<u8>, value: u64) {
    push_digits(dst, value, 10, b"0123456789");
}

/// `print_int64()`.
pub fn print_int64(dst: &mut Vec<u8>, value: i64) {
    if value < 0 {
        dst.push(b'-');
    }
    print_uint64(dst, value.unsigned_abs());
}

/// `print_uint64_hex()`: `0x` followed by uppercase digits, no padding.
pub fn print_uint64_hex(dst: &mut Vec<u8>, value: u64) {
    dst.extend_from_slice(HEX_PREFIX);
    push_digits(dst, value, 16, HEX_DIGITS);
}

/// `print_uint64_hex_full()`: `0x` followed by 16 uppercase digits.
pub fn print_uint64_hex_full(dst: &mut Vec<u8>, value: u64) {
    dst.extend_from_slice(HEX_PREFIX);
    for shift in (0..16).rev() {
        dst.push(HEX_DIGITS[((value >> (shift * 4)) & 0xf) as usize]);
    }
}

/// `buffer_print_int64_hex()`: the sign, then [`print_uint64_hex`] of the magnitude.
pub fn print_int64_hex(dst: &mut Vec<u8>, value: i64) {
    if value < 0 {
        dst.push(b'-');
    }
    print_uint64_hex(dst, value.unsigned_abs());
}

/// `buffer_print_uint64_base64()`: `#` followed by base64 digits.
pub fn print_uint64_base64(dst: &mut Vec<u8>, value: u64) {
    dst.extend_from_slice(UINT64_B64_PREFIX);
    push_digits(dst, value, 64, BASE64_DIGITS);
}

/// `buffer_print_int64_base64()`: the sign, then [`print_uint64_base64`] of the magnitude.
pub fn print_int64_base64(dst: &mut Vec<u8>, value: i64) {
    if value < 0 {
        dst.push(b'-');
    }
    print_uint64_base64(dst, value.unsigned_abs());
}

/// `buffer_print_netdata_double_hex()`: `%` followed by the IEEE-754 bits in hex.
pub fn print_netdata_double_hex(dst: &mut Vec<u8>, value: f64) {
    dst.extend_from_slice(DOUBLE_HEX_PREFIX);
    push_digits(dst, value.to_bits(), 16, HEX_DIGITS);
}

/// `buffer_print_netdata_double_base64()`: `@` followed by the IEEE-754 bits in base64.
pub fn print_netdata_double_base64(dst: &mut Vec<u8>, value: f64) {
    dst.extend_from_slice(DOUBLE_B64_PREFIX);
    push_digits(dst, value.to_bits(), 64, BASE64_DIGITS);
}

/// `print_netdata_double()`: at most 7 fractional digits (rounded with
/// `llrint()`, trailing zeros removed), switching to an 18-digit mantissa
/// with an `e+NN` exponent from `UINT64_MAX / 10` upwards.
///
/// Values below zero that round to zero print as `-0`; `-0.0` prints as `0`.
/// NaN and infinities are not special-cased (see
/// [`print_netdata_double_or_null`]); for them this reproduces what the C
/// code computes on x86-64, e.g. `0e+2147483648` for infinity.
pub fn print_netdata_double(dst: &mut Vec<u8>, value: f64) {
    let mut value = value;
    if value < 0.0 {
        dst.push(b'-');
        value = value.abs();
    }

    let mut fractional_precision: u64 = 10_000_000;
    let mut fractional_wanted_digits = 7;
    let mut exponent: i32 = 0;
    if value >= (u64::MAX / 10) as f64 {
        // too big for 64-bit integer printing: switch to exponential notation
        exponent = c::double_to_i32(value.log10().floor());
        value /= 10f64.powf(f64::from(exponent));
        fractional_precision = 1_000_000_000_000_000_000;
        fractional_wanted_digits = 18;
    }

    let (fractional_d, integral_d) = c::modf(value);
    let mut integral = c::double_to_u64(integral_d);
    let mut fractional = c::llrint(fractional_d * fractional_precision as f64) as u64;
    if fractional >= fractional_precision {
        integral = integral.wrapping_add(1);
        fractional -= fractional_precision;
    }

    print_uint64(dst, integral);

    if fractional != 0 {
        dst.push(b'.');
        let start = dst.len();
        print_uint64(dst, fractional);
        let printed = dst.len() - start;
        if printed < fractional_wanted_digits {
            // zero-pad on the left to the full precision
            let pad = fractional_wanted_digits - printed;
            dst.splice(start..start, std::iter::repeat_n(b'0', pad));
        }
        while dst.last() == Some(&b'0') {
            dst.pop();
        }
    }

    if exponent != 0 {
        dst.extend_from_slice(b"e+");
        print_uint64(dst, u64::from(exponent as u32));
    }
}

/// `buffer_print_netdata_double()`: `null` for NaN and infinities, otherwise
/// [`print_netdata_double`].
pub fn print_netdata_double_or_null(dst: &mut Vec<u8>, value: f64) {
    if value.is_finite() {
        print_netdata_double(dst, value);
    } else {
        dst.extend_from_slice(b"null");
    }
}

/// [`print_netdata_double`] into a new `String`.
pub fn netdata_double_to_string(value: f64) -> String {
    let mut out = Vec::with_capacity(32);
    print_netdata_double(&mut out, value);
    // the printer only emits ASCII digits, '-', '.', 'e' and '+'
    String::from_utf8(out).unwrap_or_default()
}

/// `buffer_print_uint64_encoded()`.
pub fn print_uint64_encoded(dst: &mut Vec<u8>, encoding: NumberEncoding, value: u64) {
    match encoding {
        NumberEncoding::Decimal => print_uint64(dst, value),
        NumberEncoding::Hex => print_uint64_hex(dst, value),
        NumberEncoding::Base64 => print_uint64_base64(dst, value),
    }
}

/// `buffer_print_int64_encoded()`.
pub fn print_int64_encoded(dst: &mut Vec<u8>, encoding: NumberEncoding, value: i64) {
    match encoding {
        NumberEncoding::Decimal => print_int64(dst, value),
        NumberEncoding::Hex => print_int64_hex(dst, value),
        NumberEncoding::Base64 => print_int64_base64(dst, value),
    }
}

/// `buffer_print_netdata_double_encoded()`; the decimal form prints `null`
/// for NaN and infinities, the hex and base64 forms carry the raw bits.
pub fn print_netdata_double_encoded(dst: &mut Vec<u8>, encoding: NumberEncoding, value: f64) {
    match encoding {
        NumberEncoding::Decimal => print_netdata_double_or_null(dst, value),
        NumberEncoding::Hex => print_netdata_double_hex(dst, value),
        NumberEncoding::Base64 => print_netdata_double_base64(dst, value),
    }
}

/// `uuid_unparse_lower()`: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
pub fn print_uuid_lower(dst: &mut Vec<u8>, uuid: &[u8; 16]) {
    for (i, byte) in uuid.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            dst.push(b'-');
        }
        dst.push(HEX_DIGITS_LOWER[usize::from(byte >> 4)]);
        dst.push(HEX_DIGITS_LOWER[usize::from(byte & 0xf)]);
    }
}

/// `uuid_unparse_lower_compact()`: 32 lowercase hex digits.
pub fn print_uuid_lower_compact(dst: &mut Vec<u8>, uuid: &[u8; 16]) {
    for byte in uuid {
        dst.push(HEX_DIGITS_LOWER[usize::from(byte >> 4)]);
        dst.push(HEX_DIGITS_LOWER[usize::from(byte & 0xf)]);
    }
}

/// `buffer_strcat_htmlescape()`.
pub fn html_escape(out: &mut Vec<u8>, text: &[u8]) {
    for &c in c::c_str(text) {
        match c {
            b'&' => out.extend_from_slice(b"&amp;"),
            b'<' => out.extend_from_slice(b"&lt;"),
            b'>' => out.extend_from_slice(b"&gt;"),
            b'"' => out.extend_from_slice(b"&quot;"),
            b'/' => out.extend_from_slice(b"&#x2F;"),
            b'\'' => out.extend_from_slice(b"&#x27;"),
            _ => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_matches_c() {
        let mut out = b"x: ".to_vec();
        html_escape(&mut out, b"a/<b>&\"c'\0ignored");
        assert_eq!(out, b"x: a&#x2F;&lt;b&gt;&amp;&quot;c&#x27;".to_vec());
    }

    fn printed(f: impl FnOnce(&mut Vec<u8>)) -> String {
        let mut out = Vec::new();
        f(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn non_finite_doubles() {
        let cases = [
            (f64::INFINITY, "0e+2147483648"),
            (f64::NEG_INFINITY, "-0e+2147483648"),
            (f64::NAN, "9223372036854775809.9223372036844775808"),
        ];
        for (value, expected) in cases {
            assert_eq!(netdata_double_to_string(value), expected);
            assert_eq!(printed(|o| print_netdata_double_or_null(o, value)), "null");
        }
    }

    #[test]
    fn uuids() {
        let uuid = [
            0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0xfe, 0xdc, 0xba, 0x98, 0x76, 0x54,
            0x32, 0x10,
        ];
        assert_eq!(
            printed(|o| print_uuid_lower(o, &uuid)),
            "01234567-89ab-cdef-fedc-ba9876543210"
        );
        assert_eq!(
            printed(|o| print_uuid_lower_compact(o, &uuid)),
            "0123456789abcdeffedcba9876543210"
        );
    }
}
