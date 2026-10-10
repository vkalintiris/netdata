//! A column as C's `sqlite3_column_int64()` and `sqlite3_column_double()` read it, whatever its stored type
//! (SQLite 3.53.4, as C links it: `sqlite3VdbeIntValue()`, `sqlite3VdbeRealValue()`): NULL is 0, a real is
//! truncated, and a text or a blob is read as the number its bytes begin with, up to their first NUL.

use rusqlite::Row;
use rusqlite::types::ValueRef;

/// `sqlite3_column_int64()`.
pub(crate) fn int(row: &Row<'_>, i: usize) -> i64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v,
        // sqlite3RealToI64(): truncated, saturated at the type's ends
        Ok(ValueRef::Real(v)) => v as i64,
        Ok(ValueRef::Text(t) | ValueRef::Blob(t)) => atoi64(t),
        _ => 0,
    }
}

/// `sqlite3_column_double()`.
pub(crate) fn double(row: &Row<'_>, i: usize) -> f64 {
    match row.get_ref(i) {
        Ok(ValueRef::Integer(v)) => v as f64,
        Ok(ValueRef::Real(v)) => v,
        Ok(ValueRef::Text(t) | ValueRef::Blob(t)) => atof(c_string(t)),
        _ => 0.0,
    }
}

/// The bytes before the first NUL: what C's string functions see of a value.
pub(crate) fn c_string(bytes: &[u8]) -> &[u8] {
    bytes.iter().position(|&c| c == 0).map_or(bytes, |end| &bytes[..end])
}

/// `sqlite3Isspace()`.
fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r')
}

/// `sqlite3Atoi64()` of UTF-8 text: spaces, a sign, the digits that follow; past the type's range, its end on
/// that side; 0 without a digit.
fn atoi64(bytes: &[u8]) -> i64 {
    let mut at = bytes.iter().position(|&c| !is_space(c)).unwrap_or(bytes.len());
    let negative = bytes.get(at) == Some(&b'-');
    if matches!(bytes.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let mut magnitude: u128 = 0;
    for &c in bytes[at..].iter().take_while(|c| c.is_ascii_digit()) {
        magnitude = (magnitude * 10 + u128::from(c - b'0')).min(1 << 64);
    }
    if negative {
        if magnitude >= 1 << 63 { i64::MIN } else { -(magnitude as i64) }
    } else {
        i64::try_from(magnitude).unwrap_or(i64::MAX)
    }
}

/// `sqlite3AtoF()`: the longest prefix of `[spaces][sign](digits[.digits]|.digits)[(e|E)[sign]digits]`, 0 when there
/// is none. Its first 19 or so significant digits make an integer `s` and the rest a power of ten `d`, as C's; C then
/// takes the double closest to `s * 10^d` (`sqlite3Fp10Convert2()`), which is what Rust's parse of that exact
/// decimal gives.
fn atof(bytes: &[u8]) -> f64 {
    const CAP: u64 = (u64::MAX - 9) / 10;
    let digit = |at: usize| bytes.get(at).filter(|c| c.is_ascii_digit()).map(|c| u64::from(c - b'0'));
    let mut at = bytes.iter().position(|&c| !is_space(c)).unwrap_or(bytes.len());
    let negative = bytes.get(at) == Some(&b'-');
    if matches!(bytes.get(at), Some(b'-' | b'+')) {
        at += 1;
    }
    let (mut s, mut d, mut seen) = (0u64, 0i64, false);
    while let Some(v) = digit(at) {
        seen = true;
        at += 1;
        if s >= CAP {
            // the digits past the mantissa's room only scale it
            d += 1;
        } else {
            s = s * 10 + v;
        }
    }
    if bytes.get(at) == Some(&b'.') {
        at += 1;
        while let Some(v) = digit(at) {
            seen = true;
            at += 1;
            if s < CAP {
                s = s * 10 + v;
                d -= 1;
            }
        }
    }
    if !seen {
        return 0.0;
    }
    if matches!(bytes.get(at), Some(b'e' | b'E')) {
        let mut after = at + 1;
        let sign = if bytes.get(after) == Some(&b'-') { -1 } else { 1 };
        if matches!(bytes.get(after), Some(b'-' | b'+')) {
            after += 1;
        }
        if digit(after).is_some() {
            let mut exp = 0i64;
            while let Some(v) = digit(after) {
                exp = if exp < 10000 { exp * 10 + v as i64 } else { 10000 };
                after += 1;
            }
            d += sign * exp;
        }
    }
    let value = if s == 0 { 0.0 } else { format!("{s}e{d}").parse().unwrap_or(0.0) };
    if negative { -value } else { value }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    /// `sqlite3Atoi64()`: spaces and a sign, the leading digits, the type's ends past its range.
    #[test]
    fn a_text_s_integer_is_its_leading_number() {
        for (text, want) in [
            ("42", 42),
            (" \t-7x", -7),
            ("+0012", 12),
            ("1e3", 1),
            ("", 0),
            ("x1", 0),
            ("-", 0),
            ("9223372036854775807", i64::MAX),
            ("9223372036854775808", i64::MAX),
            ("-9223372036854775808", i64::MIN),
            ("-99999999999999999999999", i64::MIN),
            ("000000000000000000000000000042", 42),
        ] {
            assert_eq!(atoi64(text.as_bytes()), want, "{text:?}");
        }
    }

    /// `sqlite3AtoF()`: the longest prefix that reads as a number; an exponent without digits is not part of it.
    #[test]
    fn a_text_s_real_is_its_leading_number() {
        for (text, want) in [
            ("42", 42.0),
            (" 4.5e1x", 45.0),
            (".5", 0.5),
            ("-.", 0.0),
            ("1e", 1.0),
            ("2e+", 2.0),
            ("e5", 0.0),
            ("1e400", f64::INFINITY),
            ("-1e400", f64::NEG_INFINITY),
            ("1e-400", 0.0),
            ("0.1", 0.1),
            ("12345678901234567890123", 1.2345678901234568e22),
            ("1.5abc", 1.5),
        ] {
            assert_eq!(atof(text.as_bytes()), want, "{text:?}");
        }
        assert!(atof(b"-0").is_sign_negative());
    }

    /// A value stored with another type than its column's, as a hand-edited database has it (the health oracle's
    /// `transitions-coerce`): a blob and a text read as their leading numbers, a real truncated, NULL as 0.
    #[test]
    fn a_column_of_any_type_reads_as_c_reads_it() {
        let c = Connection::open_in_memory().unwrap();
        let row = |sql: &str| -> (i64, f64) {
            c.query_row(sql, [], |row| Ok((int(row, 0), double(row, 0)))).unwrap()
        };
        assert_eq!(row("SELECT CAST('42' AS BLOB)"), (42, 42.0));
        assert_eq!(row("SELECT X'34320061'"), (42, 42.0));
        assert_eq!(row("SELECT ' 12abc'"), (12, 12.0));
        assert_eq!(row("SELECT 3.9"), (3, 3.9));
        assert_eq!(row("SELECT NULL"), (0, 0.0));
    }
}
