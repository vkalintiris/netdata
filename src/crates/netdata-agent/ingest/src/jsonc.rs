//! The member readers of `src/libnetdata/json/json-c-parser-inline.h` (`JSONC_PARSE_*_OR_ERROR_AND_RETURN`) over
//! parsed JSON: json-c 0.18's coercions between types and the texts appended to the caller's error buffer (spec:
//! `evidence/2026-09-26-sp1-jsonc-spec.md` §3 in the status repository). Members are read at the root of an object
//! (C's `path` is empty), so they print as `'.member'`. json-c sees every string up to its first NUL.
//!
//! `None` is a failure (the text is in the buffer). A member an optional reader skips reads as zero or absent: every
//! destination the callers use starts at zero.

use netdata_agent_text::c::c_str;
use netdata_agent_text::parse::{strtoll10, strtoull10, uuid_parse_flexi};
use netdata_agent_text::print::{print_int64, print_netdata_double};
use serde_json::{Map, Number, Value};

/// `JSONC_REQUIRED` or `JSONC_OPTIONAL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Presence {
    Required,
    Optional,
}

/// A JSON number as json-c typed it: integers by range, doubles when the literal had a fraction or an exponent.
/// Integer literals beyond the 64-bit ranges are doubles for serde (json-c saturates them): an accepted divergence.
enum Num {
    Unsigned(u64),
    Signed(i64),
    Double(f64),
}

fn num(n: &Number) -> Num {
    if let Some(u) = n.as_u64() {
        Num::Unsigned(u)
    } else if let Some(i) = n.as_i64() {
        Num::Signed(i)
    } else {
        Num::Double(n.as_f64().unwrap_or(f64::NAN))
    }
}

/// `json_object_get_int64()` of an integer: values above `i64::MAX` saturate.
fn int_as_i64(n: &Num) -> Option<i64> {
    match *n {
        Num::Unsigned(u) => Some(i64::try_from(u).unwrap_or(i64::MAX)),
        Num::Signed(i) => Some(i),
        Num::Double(_) => None,
    }
}

/// 2^63 and 2^64 as doubles, the bounds of `jsonc_double_fits_integer_destination()`.
const TWO_63: f64 = 9_223_372_036_854_775_808.0;
const TWO_64: f64 = 18_446_744_073_709_551_616.0;

fn fail<T>(error: &mut String, text: String) -> Option<T> {
    error.push_str(&text);
    None
}

/// A missing member: an error when required, else zero.
fn missing<T: Default>(member: &str, presence: Presence, error: &mut String) -> Option<T> {
    match presence {
        Presence::Required => fail(error, format!("missing '.{member}'")),
        Presence::Optional => Some(T::default()),
    }
}

/// An object or array where a scalar belongs: an error when required, else skipped.
fn wrong_type<T: Default>(text: String, presence: Presence, error: &mut String) -> Option<T> {
    match presence {
        Presence::Required => fail(error, text),
        Presence::Optional => Some(T::default()),
    }
}

/// `JSONC_PARSE_INT64_OR_ERROR_AND_RETURN`: bad strings and doubles fail even when optional.
pub fn int64(
    obj: &Map<String, Value>,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<i64> {
    let Some(value) = obj.get(member) else {
        return missing(member, presence, error);
    };
    match value {
        Value::Null => Some(0),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => match num(n) {
            Num::Double(d) => {
                if d.is_finite() && (-TWO_63..TWO_63).contains(&d) {
                    Some(d as i64)
                } else {
                    fail(error, format!("cannot convert to int64 for '.{member}'"))
                }
            }
            n => int_as_i64(&n),
        },
        Value::String(s) => {
            let s = c_str(s.as_bytes());
            match strtoll10(s) {
                (v, used, false) if used > 0 && used == s.len() => Some(v),
                _ => fail(
                    error,
                    format!(
                        "cannot convert string '{}' to int64 for '.{member}'",
                        String::from_utf8_lossy(s)
                    ),
                ),
            }
        }
        Value::Object(_) | Value::Array(_) => wrong_type(
            format!("cannot convert to int64 for '.{member}'"),
            presence,
            error,
        ),
    }
}

/// `JSONC_PARSE_UINT64_OR_ERROR_AND_RETURN`: negative integers read as 0; bad strings and doubles fail even when
/// optional.
pub fn uint64(
    obj: &Map<String, Value>,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<u64> {
    let Some(value) = obj.get(member) else {
        return missing(member, presence, error);
    };
    match value {
        Value::Null => Some(0),
        Value::Bool(b) => Some(u64::from(*b)),
        Value::Number(n) => match num(n) {
            Num::Unsigned(u) => Some(u),
            Num::Signed(_) => Some(0),
            Num::Double(d) => {
                if d.is_finite() && (0.0..TWO_64).contains(&d) {
                    Some(d as u64)
                } else {
                    fail(error, format!("cannot convert to uint64 for '.{member}'"))
                }
            }
        },
        Value::String(s) => {
            let s = c_str(s.as_bytes());
            let shown = String::from_utf8_lossy(s);
            if s.first() == Some(&b'-') {
                return fail(
                    error,
                    format!("cannot convert negative string '{shown}' to uint64 for '.{member}'"),
                );
            }
            match strtoull10(s) {
                (v, used, false) if used > 0 && used == s.len() => Some(v),
                _ => fail(
                    error,
                    format!("cannot convert string '{shown}' to uint64 for '.{member}'"),
                ),
            }
        }
        Value::Object(_) | Value::Array(_) => wrong_type(
            format!("cannot convert to uint64 for '.{member}'"),
            presence,
            error,
        ),
    }
}

/// `JSONC_PARSE_TXT2STRING_OR_ERROR_AND_RETURN`: numbers and booleans as json-c prints them; `None` inside for a
/// null or empty text (`string_strdupz()` of `""` is NULL).
pub fn txt(
    obj: &Map<String, Value>,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<String>> {
    let Some(value) = obj.get(member) else {
        return missing(member, presence, error);
    };
    let text = match value {
        Value::Null => return Some(None),
        Value::String(s) => String::from_utf8_lossy(c_str(s.as_bytes())).into_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            let mut out = Vec::new();
            match num(n) {
                Num::Double(d) => print_netdata_double(&mut out, d),
                n => print_int64(&mut out, int_as_i64(&n).unwrap_or_default()),
            }
            String::from_utf8_lossy(&out).into_owned()
        }
        Value::Object(_) | Value::Array(_) => {
            return wrong_type(
                format!("cannot convert to string for '.{member}'"),
                presence,
                error,
            );
        }
    };
    Some((!text.is_empty()).then_some(text))
}

/// `JSONC_PARSE_TXT2UUID_OR_ERROR_AND_RETURN` (`uuid_parse()` is `uuid_parse_flexi()`): a null member is the zero
/// UUID.
pub fn uuid(
    obj: &Map<String, Value>,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<[u8; 16]> {
    let required = presence == Presence::Required;
    let Some(value) = obj.get(member) else {
        return if required {
            fail(error, format!("missing UUID '.{member}'"))
        } else {
            Some([0; 16])
        };
    };
    match value {
        Value::Null => Some([0; 16]),
        Value::String(s) => match uuid_parse_flexi(s.as_bytes()) {
            Some(uuid) => Some(uuid),
            None if required => fail(error, format!("invalid UUID '.{member}'")),
            None => Some([0; 16]),
        },
        _ if required => fail(error, format!("invalid type for UUID '.{member}'")),
        _ => Some([0; 16]),
    }
}

/// `JSONC_PARSE_ARRAY_OF_TXT2BITMAP_OR_ERROR_AND_RETURN`: the bits of the names in an array. An unknown name is
/// only reported; an item that is not a string fails.
pub fn bitmap(
    obj: &Map<String, Value>,
    member: &str,
    parse_one: fn(&[u8]) -> u32,
    presence: Presence,
    error: &mut String,
) -> Option<u32> {
    let required = presence == Presence::Required;
    let Some(value) = obj.get(member) else {
        return if required {
            fail(error, format!("missing '.{member}' array"))
        } else {
            Some(0)
        };
    };
    let Value::Array(items) = value else {
        return if required {
            fail(error, format!("invalid type for '.{member}' array"))
        } else {
            Some(0)
        };
    };
    let mut bits = 0;
    for (i, item) in items.iter().enumerate() {
        let Value::String(name) = item else {
            return fail(error, format!("invalid type for '.{member}' at index {i}"));
        };
        let name = c_str(name.as_bytes());
        let bit = parse_one(name);
        if bit == 0 {
            error.push_str(&format!(
                "unknown option '{}' in '.{member}' at index {i}",
                String::from_utf8_lossy(name)
            ));
        }
        bits |= bit;
    }
    Some(bits)
}
