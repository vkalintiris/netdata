//! The member readers of `src/libnetdata/json/json-c-parser-inline.h` (`JSONC_PARSE_*_OR_ERROR_AND_RETURN`) over
//! parsed JSON: json-c 0.18's coercions between types and the texts appended to the caller's error buffer (spec:
//! `evidence/2026-09-26-sp1-jsonc-spec.md` §3 in the status repository). `path` is C's: a member prints as
//! `'{path}.{member}'`. json-c sees every string up to its first NUL.
//!
//! `None` is a failure (the text is in the buffer). A member an optional reader skips reads as zero or absent: every
//! destination the callers use starts at zero.

use std::borrow::Cow;

use netdata_agent_log::netdata_log_error_errno;
use netdata_agent_nrpc::reply::Reply;
use netdata_agent_text::c::c_str;
use netdata_agent_text::jsonc_doc::ends_a_number;
use netdata_agent_text::parse::{strtod_range, strtoll10, strtoull10, uuid_parse_flexi};
use netdata_agent_text::print::{print_int64, print_netdata_double};
use serde_json::{Map, Number, Value};

/// `JSON_TOKENER_DEFAULT_DEPTH`: json-c refuses values nested deeper (serde_json allows 127).
const TOKENER_DEPTH: usize = 32;

/// The text json-c's tokener reads, as serde reads it: up to its first NUL, with U+FFFD in place of each invalid UTF-8
/// sequence (D89), the control bytes json-c takes raw inside a string escaped, and the number `-0` without its sign:
/// json-c types it as the integer 0, where serde makes the double -0.0 of it (as both do of `-0.0` and `-0e0`).
fn prepare(text: &[u8]) -> Cow<'_, [u8]> {
    let text = match String::from_utf8_lossy(c_str(text)) {
        Cow::Borrowed(s) => Cow::Borrowed(s.as_bytes()),
        Cow::Owned(s) => Cow::Owned(s.into_bytes()),
    };
    let (mut in_string, mut escaped, mut out) = (false, false, None::<Vec<u8>>);
    for (i, &c) in text.iter().enumerate() {
        if in_string && !escaped && c < 0x20 {
            out.get_or_insert_with(|| text[..i].to_vec()).extend_from_slice(format!("\\u{c:04x}").as_bytes());
            continue;
        }
        let whole_zero =
            || text.get(i + 1) == Some(&b'0') && !matches!(text.get(i + 2), Some(b'0'..=b'9' | b'.' | b'e' | b'E'));
        if !in_string && c == b'-' && whole_zero() {
            out.get_or_insert_with(|| text[..i].to_vec());
            continue;
        }
        if let Some(out) = &mut out {
            out.push(c);
        }
        match c {
            _ if !in_string => in_string = c == b'"',
            _ if escaped => escaped = false,
            b'\\' => escaped = true,
            b'"' => in_string = false,
            _ => {}
        }
    }
    out.map_or(text, Cow::Owned)
}

/// Whether an object key of `value` holds a NUL. json-c cuts such a key there, and a later member of the cut name
/// replaces an earlier one; serde's map no longer knows the members' order, so such a text is refused (D46.1). A key
/// gets a NUL only from a `\u0000` escape (`text`).
fn has_nul_key(text: &[u8], value: &Value) -> bool {
    fn walk(value: &Value) -> bool {
        match value {
            Value::Object(members) => members.iter().any(|(key, v)| key.contains('\0') || walk(v)),
            Value::Array(items) => items.iter().any(walk),
            _ => false,
        }
    }
    text.windows(6).any(|w| w == b"\\u0000") && walk(value)
}

/// `json_tokener_parse()`: the first JSON value of `text` up to its first NUL; what follows the value is ignored, and
/// the text is read as [`prepare`] gives it. Input json-c tolerates and serde_json does not (comments, single quotes,
/// trailing commas, NaN, a number or literal root followed by text, a NUL in a key) is refused: Rust only ever refuses
/// more than C (D46.1).
pub fn tokener_parse(text: &[u8]) -> Option<Value> {
    let text = prepare(text);
    let mut values = serde_json::Deserializer::from_slice(&text).into_iter::<Value>();
    let value = values.next()?.ok()?;
    let consumed = &text[..values.byte_offset()];
    (!too_deep(consumed) && !has_nul_key(consumed, &value)).then_some(value)
}

/// `json_tokener_error_desc()`: json-c 0.18's texts for the errors [`tokener_parse_ex`] reports.
pub mod tokener_error {
    pub const CONTINUE: &str = "continue";
    pub const DEPTH: &str = "nesting too deep";
    pub const UNEXPECTED: &str = "unexpected character";
    pub const NULL: &str = "null expected";
    pub const BOOLEAN: &str = "boolean expected";
    pub const NUMBER: &str = "number expected";
    pub const ARRAY: &str = "array value separator ',' expected";
    pub const OBJECT_KEY_NAME: &str = "quoted object property name expected";
    pub const OBJECT_KEY_SEP: &str = "object property name separator ':' expected";
    pub const OBJECT_VALUE_SEP: &str = "object value separator ',' expected";
    pub const STRING: &str = "invalid string sequence";
    pub const COMMENT: &str = "expected comment";
    pub const EOF: &str = "unexpected end of data";
}

/// `json_tokener_parse_ex()` of a whole text, as a caller passing its length runs it: the first value of `text` (read
/// as [`prepare`] gives it), or json-c's error text. Unlike [`tokener_parse`], json-c then reads one byte past a root
/// number or literal, so a text that ends in one is `continue`; a NUL ends the data as the text's end does, but with
/// its own text. The texts map serde's refusals onto json-c's (D176.5, D179); input json-c accepts and serde refuses
/// (D46.1) gets the text of serde's refusal.
pub fn tokener_parse_ex(text: &[u8]) -> Result<Value, &'static str> {
    let at_nul = c_str(text).len() < text.len();
    let end = if at_nul { tokener_error::EOF } else { tokener_error::CONTINUE };
    let text = prepare(text);
    let text = text.as_ref();
    let mut values = serde_json::Deserializer::from_slice(text).into_iter::<Value>();
    match values.next() {
        None => Err(end),
        Some(Ok(value)) => {
            let consumed = &text[..values.byte_offset()];
            if too_deep(consumed) {
                Err(tokener_error::DEPTH)
            } else if has_nul_key(consumed, &value) {
                Err(tokener_error::UNEXPECTED)
            } else if !at_nul
                && consumed.len() == text.len()
                && matches!(value, Value::Null | Value::Bool(_) | Value::Number(_))
            {
                Err(tokener_error::CONTINUE)
            } else {
                Ok(value)
            }
        }
        Some(Err(e)) => Err(refusal(text, &e, end)),
    }
}

/// json-c's text for serde's refusal `e` of `text`, `end` when json-c runs out of data.
fn refusal(text: &[u8], e: &serde_json::Error, end: &'static str) -> &'static str {
    use tokener_error::*;
    // serde places a refusal one byte past the byte it refused, an end of text at the end
    let at = offset(text, e.line(), e.column());
    // json-c checks the depth as a value starts, before anything else about it
    if too_deep(&text[..at]) {
        return DEPTH;
    }
    if e.classify() == serde_json::error::Category::Eof {
        return end;
    }
    let refused = at.checked_sub(1).map(|i| text[i]);
    // json-c reads a comment wherever whitespace may be
    if refused == Some(b'/') {
        match text.get(at) {
            None => return end,
            Some(b'*' | b'/') => {}
            Some(_) => return COMMENT,
        }
    }
    let message = e.to_string();
    match message.split(" at line ").next().unwrap_or_default() {
        "expected `:`" => OBJECT_KEY_SEP,
        "expected `,` or `]`" | "expected `,` or `}`" if after_number(text, at) => NUMBER,
        "expected `,` or `]`" => ARRAY,
        "expected `,` or `}`" => OBJECT_VALUE_SEP,
        "key must be a string" => OBJECT_KEY_NAME,
        "expected ident" => literal(text, at),
        // json-c's literal states ignore case
        "expected value" => match refused {
            Some(b'T' | b'F') => BOOLEAN,
            Some(b'N') => NULL,
            _ => UNEXPECTED,
        },
        "invalid number" | "number out of range" => NUMBER,
        "recursion limit exceeded" => DEPTH,
        "invalid escape"
        | "invalid unicode code point"
        | "lone leading surrogate in hex escape"
        | "unexpected end of hex escape" => STRING,
        m if m.starts_with("control character") => STRING,
        _ => UNEXPECTED,
    }
}

/// The byte offset of serde's position: its line counts from 1, its column in bytes from the line's start.
fn offset(text: &[u8], line: usize, column: usize) -> usize {
    let start = match line {
        0 | 1 => 0,
        n => text.iter().enumerate().filter(|(_, c)| **c == b'\n').nth(n - 2).map_or(text.len(), |(i, _)| i + 1),
    };
    (start + column).min(text.len())
}

/// Whether json-c meets a value nested deeper than it allows in `prefix`, a text serde read up to its last byte:
/// json-c checks as an array's item or an object's value starts (the root counts as 1), so a value serde's map later
/// drops for a repeated key counts too.
fn too_deep(prefix: &[u8]) -> bool {
    let (mut open, mut value_next, mut in_string, mut escaped) = (Vec::new(), true, false, false);
    for &c in prefix {
        if in_string {
            match c {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        if matches!(c, b' ' | b'\t' | b'\n' | b'\r') {
            continue;
        }
        // an array's `]` ends it before the check
        if value_next && !(c == b']' && open.last() == Some(&b']')) && open.len() + 1 > TOKENER_DEPTH {
            return true;
        }
        value_next = false;
        match c {
            b'[' => {
                open.push(b']');
                value_next = true;
            }
            b'{' => open.push(b'}'),
            b']' | b'}' => {
                open.pop();
            }
            b':' => value_next = true,
            b',' => value_next = open.last() == Some(&b']'),
            b'"' => in_string = true,
            _ => {}
        }
    }
    false
}

/// Whether the byte refused before `at` follows a number's last byte and cannot end a number in a container: json-c's
/// number state refuses it there.
fn after_number(text: &[u8], at: usize) -> bool {
    let Some(refused) = at.checked_sub(1) else {
        return false;
    };
    if ends_a_number(text[refused]) {
        return false;
    }
    let start = text[..refused]
        .iter()
        .rposition(|c| !matches!(c, b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-'))
        .map_or(0, |i| i + 1);
    start < refused && matches!(text[start], b'0'..=b'9' | b'-')
}

/// The literal serde refused a letter of before `at`: json-c's null state refuses one starting with `n`, its boolean
/// state the others.
fn literal(text: &[u8], at: usize) -> &'static str {
    let start = text[..at.saturating_sub(1)]
        .iter()
        .rposition(|c| !c.is_ascii_alphabetic())
        .map_or(0, |i| i + 1);
    if text.get(start) == Some(&b'n') { tokener_error::NULL } else { tokener_error::BOOLEAN }
}

/// `JSON_PARSER_ERROR_MSG_MAX`: C's buffer for a parser error, its NUL included.
const ERROR_MSG_MAX: usize = 256;

/// `json_parser_format_error()`: the prefix, then the text cut to fit C's buffer, with `...` when cut; an empty text
/// reads `unknown error`.
fn format_error(text: &[u8]) -> Vec<u8> {
    const PREFIX: &[u8] = b"JSON parser failed: ";
    const SUFFIX: &[u8] = b"...";
    let text = if text.is_empty() { b"unknown error".as_slice() } else { text };
    let mut available = ERROR_MSG_MAX - 1 - PREFIX.len();
    let truncated = text.len() > available;
    if truncated {
        available -= SUFFIX.len();
    }
    let mut out = PREFIX.to_vec();
    out.extend_from_slice(&text[..text.len().min(available)]);
    if truncated {
        out.extend_from_slice(SUFFIX);
    }
    out
}

/// `json_parse_function_payload_or_error()`: what `parse` (C's callback) reads from the payload's members, or the code
/// of the error `reply` now holds: 400 without a payload or with `parse`'s text, 500 with json-c's. A root that is no
/// object has no members.
pub fn function_payload_or_error<T>(
    reply: &mut Reply,
    payload: Option<&[u8]>,
    parse: impl FnOnce(&Map<String, Value>, &mut String) -> Option<T>,
) -> Result<T, u16> {
    let Some(payload) = payload.filter(|p| !p.is_empty()) else {
        return Err(reply.error("No payload given, but a payload is required for this feature.", 400));
    };
    let root = tokener_parse_ex(payload).map_err(|e| reply.error(format_error(e.as_bytes()), 500))?;
    let empty = Map::new();
    let members = match &root {
        Value::Object(members) => members,
        _ => &empty,
    };
    let mut error = String::new();
    parse(members, &mut error).ok_or_else(|| reply.error(format_error(error.as_bytes()), 400))
}

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
fn missing<T: Default>(path: &str, member: &str, presence: Presence, error: &mut String) -> Option<T> {
    match presence {
        Presence::Required => fail(error, format!("missing '{path}.{member}'")),
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
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<i64> {
    let Some(value) = obj.get(member) else {
        return missing(path, member, presence, error);
    };
    match value {
        Value::Null => Some(0),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::Number(n) => match num(n) {
            Num::Double(d) => {
                if d.is_finite() && (-TWO_63..TWO_63).contains(&d) {
                    Some(d as i64)
                } else {
                    fail(error, format!("cannot convert to int64 for '{path}.{member}'"))
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
                        "cannot convert string '{}' to int64 for '{path}.{member}'",
                        String::from_utf8_lossy(s)
                    ),
                ),
            }
        }
        Value::Object(_) | Value::Array(_) => wrong_type(
            format!("cannot convert to int64 for '{path}.{member}'"),
            presence,
            error,
        ),
    }
}

/// `JSONC_PARSE_UINT64_OR_ERROR_AND_RETURN`: negative integers read as 0; bad strings and doubles fail even when
/// optional.
pub fn uint64(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<u64> {
    let Some(value) = obj.get(member) else {
        return missing(path, member, presence, error);
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
                    fail(error, format!("cannot convert to uint64 for '{path}.{member}'"))
                }
            }
        },
        Value::String(s) => {
            let s = c_str(s.as_bytes());
            let shown = String::from_utf8_lossy(s);
            if s.first() == Some(&b'-') {
                return fail(
                    error,
                    format!("cannot convert negative string '{shown}' to uint64 for '{path}.{member}'"),
                );
            }
            match strtoull10(s) {
                (v, used, false) if used > 0 && used == s.len() => Some(v),
                _ => fail(
                    error,
                    format!("cannot convert string '{shown}' to uint64 for '{path}.{member}'"),
                ),
            }
        }
        Value::Object(_) | Value::Array(_) => wrong_type(
            format!("cannot convert to uint64 for '{path}.{member}'"),
            presence,
            error,
        ),
    }
}

/// `JSONC_PARSE_TXT2STRING_OR_ERROR_AND_RETURN`: numbers and booleans as json-c prints them; `None` inside for a
/// null or empty text (`string_strdupz()` of `""` is NULL).
pub fn txt(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<String>> {
    let Some(value) = obj.get(member) else {
        return missing(path, member, presence, error);
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
                format!("cannot convert to string for '{path}.{member}'"),
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
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<[u8; 16]> {
    let required = presence == Presence::Required;
    let Some(value) = obj.get(member) else {
        return if required {
            fail(error, format!("missing UUID '{path}.{member}'"))
        } else {
            Some([0; 16])
        };
    };
    match value {
        Value::Null => Some([0; 16]),
        Value::String(s) => match uuid_parse_flexi(s.as_bytes()) {
            Some(uuid) => Some(uuid),
            None if required => fail(error, format!("invalid UUID '{path}.{member}'")),
            None => Some([0; 16]),
        },
        _ if required => fail(error, format!("invalid type for UUID '{path}.{member}'")),
        _ => Some([0; 16]),
    }
}

/// `JSONC_PARSE_ARRAY_OF_TXT2BITMAP_OR_ERROR_AND_RETURN`: the bits of the names in an array, in the destination's
/// integer type. An unknown name is only reported; an item that is not a string fails.
pub fn bitmap<B: Copy + Default + PartialEq + std::ops::BitOrAssign>(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    parse_one: fn(&[u8]) -> B,
    presence: Presence,
    error: &mut String,
) -> Option<B> {
    let mut bits = B::default();
    bitmap_into(obj, path, member, parse_one, presence, error, &mut bits).map(|()| bits)
}

/// [`bitmap`] into `bits`, as C fills its destination: zeroed once the member is an array, then each item's bit
/// added, so a failing item keeps the bits before it; untouched when an optional member is missing or no array.
pub fn bitmap_into<B: Copy + Default + PartialEq + std::ops::BitOrAssign>(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    parse_one: fn(&[u8]) -> B,
    presence: Presence,
    error: &mut String,
    bits: &mut B,
) -> Option<()> {
    let required = presence == Presence::Required;
    let Some(value) = obj.get(member) else {
        return if required {
            fail(error, format!("missing '{path}.{member}' array"))
        } else {
            Some(())
        };
    };
    let Value::Array(items) = value else {
        return if required {
            fail(error, format!("invalid type for '{path}.{member}' array"))
        } else {
            Some(())
        };
    };
    *bits = B::default();
    for (i, item) in items.iter().enumerate() {
        let Value::String(name) = item else {
            return fail(error, format!("invalid type for '{path}.{member}' at index {i}"));
        };
        let name = c_str(name.as_bytes());
        let bit = parse_one(name);
        if bit == B::default() {
            error.push_str(&format!(
                "unknown option '{}' in '{path}.{member}' at index {i}",
                String::from_utf8_lossy(name)
            ));
        }
        *bits |= bit;
    }
    Some(())
}

/// `JSONC_PARSE_BOOL_OR_ERROR_AND_RETURN`: strings "true"/"yes"/"on" and "false"/"no"/"off" (any case), numbers by
/// being non-zero, null as false; another string fails even when optional. A skipped member reads as false.
pub fn boolean(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<bool> {
    let Some(value) = obj.get(member) else {
        return match presence {
            Presence::Required => fail(error, format!("missing '{path}.{member}' boolean")),
            Presence::Optional => Some(false),
        };
    };
    match value {
        Value::Bool(b) => Some(*b),
        Value::String(s) => {
            let s = c_str(s.as_bytes());
            let is = |words: [&str; 3]| words.iter().any(|w| s.eq_ignore_ascii_case(w.as_bytes()));
            if is(["true", "yes", "on"]) {
                Some(true)
            } else if is(["false", "no", "off"]) {
                Some(false)
            } else {
                fail(
                    error,
                    format!(
                        "invalid boolean string '{}' for '{path}.{member}'",
                        String::from_utf8_lossy(s)
                    ),
                )
            }
        }
        Value::Number(n) => Some(match num(n) {
            Num::Double(d) => d != 0.0,
            n => int_as_i64(&n) != Some(0),
        }),
        Value::Null => Some(false),
        Value::Object(_) | Value::Array(_) => wrong_type(
            format!("cannot convert to boolean for '{path}.{member}'"),
            presence,
            error,
        ),
    }
}

/// `JSONC_PARSE_DOUBLE_OR_ERROR_AND_RETURN`: a number, a boolean as 1 or 0, null as NaN, a text `strtod()` reads
/// whole and in range; another text fails even when optional. `None` inside for a member that is skipped: C leaves
/// its destination as it was.
pub fn double(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<f64>> {
    let Some(value) = obj.get(member) else {
        return match presence {
            Presence::Required => fail(error, format!("missing '{path}.{member}'")),
            Presence::Optional => Some(None),
        };
    };
    match value {
        Value::Null => Some(Some(f64::NAN)),
        Value::Bool(b) => Some(Some(f64::from(u8::from(*b)))),
        Value::Number(n) => Some(Some(match num(n) {
            Num::Double(d) => d,
            n => int_as_i64(&n).unwrap_or_default() as f64,
        })),
        Value::String(s) => {
            let s = c_str(s.as_bytes());
            match strtod_range(s) {
                (v, used, false) if used > 0 && used == s.len() => Some(Some(v)),
                _ => fail(
                    error,
                    format!(
                        "cannot convert string '{}' to double for '{path}.{member}'",
                        String::from_utf8_lossy(s)
                    ),
                ),
            }
        }
        Value::Object(_) | Value::Array(_) => match presence {
            Presence::Required => fail(error, format!("cannot convert to double for '{path}.{member}'")),
            Presence::Optional => Some(None),
        },
    }
}

/// The member's value when json-c types it as an integer (`json_type_int`), as `json_object_get_int64()` reads it:
/// for the callers that convert an integer and a double differently.
pub fn int_member(obj: &Map<String, Value>, member: &str) -> Option<i64> {
    match obj.get(member) {
        Some(Value::Number(n)) => int_as_i64(&num(n)),
        _ => None,
    }
}

/// `JSONC_PARSE_TXT2PATTERN_OR_ERROR_AND_RETURN`: the text of a pattern; `None` inside for `*`, for the empty text
/// (`string_strdupz()` of `""` is NULL) and for a member that is skipped (every destination starts unset).
pub fn pattern(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<String>> {
    match obj.get(member) {
        Some(Value::String(s)) => {
            let text = c_str(s.as_bytes());
            Some((text != b"*" && !text.is_empty()).then(|| String::from_utf8_lossy(text).into_owned()))
        }
        Some(_) if presence == Presence::Required => {
            fail(error, format!("invalid type for '{path}.{member}' string"))
        }
        None if presence == Presence::Required => fail(error, format!("missing '{path}.{member}' string")),
        _ => Some(None),
    }
}

/// `JSONC_PARSE_TXT2CHAR_OR_ERROR_AND_RETURN` into a buffer of `size` bytes: the text cut to `size - 1` bytes
/// (`strncpyz()`), numbers and booleans as json-c prints them, the empty text for null and for a member that is
/// skipped.
pub fn chars(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    size: usize,
    presence: Presence,
    error: &mut String,
) -> Option<Vec<u8>> {
    let Some(value) = obj.get(member) else {
        return match presence {
            Presence::Required => fail(error, format!("missing '{path}.{member}'")),
            Presence::Optional => Some(Vec::new()),
        };
    };
    let mut text = match value {
        Value::Null => Vec::new(),
        Value::String(s) => c_str(s.as_bytes()).to_vec(),
        Value::Bool(b) => b.to_string().into_bytes(),
        Value::Number(n) => {
            let mut out = Vec::new();
            match num(n) {
                Num::Double(d) => print_netdata_double(&mut out, d),
                n => print_int64(&mut out, int_as_i64(&n).unwrap_or_default()),
            }
            out
        }
        Value::Object(_) | Value::Array(_) => {
            return wrong_type(format!("cannot convert to string for '{path}.{member}'"), presence, error);
        }
    };
    text.truncate(size.saturating_sub(1));
    Some(text)
}

/// `JSONC_PARSE_TXT2RFC3339_USEC_OR_ERROR_AND_RETURN`: the microseconds of an RFC 3339 text, 0 for anything else
/// (an error only when required).
pub fn rfc3339(
    obj: &Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<u64> {
    let required = presence == Presence::Required;
    let Some(value) = obj.get(member) else {
        return if required {
            fail(error, format!("missing '{path}.{member}' string"))
        } else {
            Some(0)
        };
    };
    let Value::String(s) = value else {
        return if required {
            fail(error, format!("invalid type for '{path}.{member}' string"))
        } else {
            Some(0)
        };
    };
    // copied into an RFC3339_MAX_LENGTH buffer first
    let text = c_str(s.as_bytes());
    let text = &text[..text
        .len()
        .min(netdata_agent_text::datetime::RFC3339_MAX_LENGTH - 1)];
    match netdata_agent_text::datetime::rfc3339_parse_ut(text) {
        Some(ut) => Some(ut),
        None if required => fail(error, format!("invalid RFC3339 datetime for '{path}.{member}'")),
        None => Some(0),
    }
}

/// `JSONC_PARSE_TXT2ENUM_OR_ERROR_AND_RETURN`: the text for the caller's converter. `None` inside when the member is
/// missing or not a string: C leaves the destination as it was.
pub fn enum_text<'a>(
    obj: &'a Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<&'a [u8]>> {
    match obj.get(member) {
        Some(Value::String(s)) => Some(Some(c_str(s.as_bytes()))),
        Some(_) if presence == Presence::Required => {
            fail(error, format!("invalid type for '{path}.{member}' enum"))
        }
        None if presence == Presence::Required => fail(error, format!("missing '{path}.{member}' enum")),
        _ => Some(None),
    }
}

/// `JSONC_PARSE_SUBOBJECT`: the member's object. `None` inside when it is missing or not an object and optional: C
/// skips its block.
pub fn object<'a>(
    obj: &'a Map<String, Value>,
    path: &str,
    member: &str,
    presence: Presence,
    error: &mut String,
) -> Option<Option<&'a Map<String, Value>>> {
    match obj.get(member) {
        Some(Value::Object(o)) => Some(Some(o)),
        Some(_) if presence == Presence::Required => {
            fail(error, format!("not an object '{path}.{member}'"))
        }
        None if presence == Presence::Required => {
            fail(error, format!("missing '{path}.{member}' object"))
        }
        _ => Some(None),
    }
}

/// A parsed value whose objects keep their members in the document's order, as json-c's do: `serde_json`'s map is
/// sorted by key unless a feature other packages of the workspace turn on is unified in (D210 F3). A repeated name
/// keeps its first place and takes its last value, as json-c's `json_object_object_add()` leaves it. A number is
/// a number here because no package of the workspace turns on serde_json's `arbitrary_precision`, which hands a
/// number over as an object of one member: the walk's unit would show it.
enum Ordered {
    Null,
    Bool(bool),
    Number,
    String(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl<'de> serde::Deserialize<'de> for Ordered {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Ordered, D::Error> {
        struct Visitor;

        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Ordered;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON value")
            }

            fn visit_unit<E>(self) -> Result<Ordered, E> {
                Ok(Ordered::Null)
            }

            fn visit_bool<E>(self, v: bool) -> Result<Ordered, E> {
                Ok(Ordered::Bool(v))
            }

            fn visit_i64<E>(self, _: i64) -> Result<Ordered, E> {
                Ok(Ordered::Number)
            }

            fn visit_u64<E>(self, _: u64) -> Result<Ordered, E> {
                Ok(Ordered::Number)
            }

            fn visit_f64<E>(self, _: f64) -> Result<Ordered, E> {
                Ok(Ordered::Number)
            }

            fn visit_str<E>(self, v: &str) -> Result<Ordered, E> {
                Ok(Ordered::String(v.to_owned()))
            }

            fn visit_string<E>(self, v: String) -> Result<Ordered, E> {
                Ok(Ordered::String(v))
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Ordered::Array(items))
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
                let mut members: Vec<(String, Ordered)> = Vec::new();
                while let Some((name, value)) = map.next_entry::<String, Ordered>()? {
                    match members.iter_mut().find(|(known, _)| *known == name) {
                        Some(member) => member.1 = value,
                        None => members.push((name, value)),
                    }
                }
                Ok(Ordered::Object(members))
            }
        }

        deserializer.deserialize_any(Visitor)
    }
}

impl Ordered {
    fn has_nul_key(&self) -> bool {
        match self {
            Ordered::Object(members) => members.iter().any(|(name, value)| name.contains('\0') || value.has_nul_key()),
            Ordered::Array(items) => items.iter().any(Ordered::has_nul_key),
            _ => false,
        }
    }
}

/// What `json_parse()`'s walk hands its callback, in the walk's order (`json_walk()` and `json_jsonc_parse_array()`
/// of `src/libnetdata/json/json.c`, the json-c build). C also calls it for an integer of the root, which no callback
/// of the agent looks at, and cuts a member's name at 256 bytes, which no name a callback knows reaches.
#[derive(Debug, PartialEq, Eq)]
pub enum Walked<'a> {
    /// An array's element that is an object, of any array the walk meets: C's `JSON_OBJECT` call. Elements are
    /// numbered from 0 as the walk meets them.
    Element,
    /// A string member directly in the element of that number.
    ElementString { element: usize, name: &'a [u8], value: &'a [u8] },
    /// A string member with no element in hand: of the root, or of an object inside an element.
    String { name: &'a [u8], value: &'a [u8] },
    /// A boolean member, wherever the walk meets it; its name is not looked at.
    Boolean(bool),
}

/// `json_walk()`: an object's members. A member that is an object is not descended into, a double and a null are
/// not visited.
fn walk_object(members: &[(String, Ordered)], elements: &mut usize, callback: &mut dyn FnMut(Walked<'_>)) {
    for (name, value) in members {
        match value {
            Ordered::Array(items) => walk_array(items, elements, callback),
            Ordered::String(v) => callback(Walked::String { name: name.as_bytes(), value: c_str(v.as_bytes()) }),
            Ordered::Bool(v) => callback(Walked::Boolean(*v)),
            Ordered::Null | Ordered::Number | Ordered::Object(_) => {}
        }
    }
}

/// `json_jsonc_parse_array()`: each element that is not null is announced, then its members are walked: an array as
/// this one, an object as the root, a string as the element's own.
fn walk_array(items: &[Ordered], elements: &mut usize, callback: &mut dyn FnMut(Walked<'_>)) {
    for item in items {
        // C skips a null. It announces an element that is a string, a number, a boolean or an array too, and then
        // walks its members through a NULL table: the agent dies. Such an element is skipped whole here (D210 F4).
        let Ordered::Object(members) = item else { continue };
        let element = *elements;
        *elements += 1;
        callback(Walked::Element);
        for (name, value) in members {
            match value {
                Ordered::Array(items) => walk_array(items, elements, callback),
                Ordered::Object(inner) => walk_object(inner, elements, callback),
                Ordered::String(v) => {
                    callback(Walked::ElementString { element, name: name.as_bytes(), value: c_str(v.as_bytes()) })
                }
                Ordered::Bool(v) => callback(Walked::Boolean(*v)),
                Ordered::Null | Ordered::Number => {}
            }
        }
    }
}

/// `json_parse()`: the first JSON value of `text` is parsed as [`tokener_parse`] parses (with its refusals, D46.1)
/// and, when it is an object, walked. False when the text is refused, with C's record; a value that is no object
/// parses and hands the callback nothing, but `null`, which json-c's parse gives as its failure.
pub fn json_parse(text: &[u8], callback: &mut dyn FnMut(Walked<'_>)) -> bool {
    let text = prepare(text);
    let mut values = serde_json::Deserializer::from_slice(&text).into_iter::<Ordered>();
    let value = match values.next() {
        Some(Ok(value)) if !too_deep(&text[..values.byte_offset()]) && !value.has_nul_key() => value,
        _ => Ordered::Null,
    };
    match value {
        Ordered::Null => {
            netdata_log_error_errno!("JSON: Invalid json string.");
            false
        }
        Ordered::Object(members) => {
            walk_object(&members, &mut 0, callback);
            true
        }
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Presence::{Optional, Required};

    fn obj(text: &str) -> Map<String, Value> {
        serde_json::from_str(text).unwrap()
    }

    /// json-c's tokener: the first value up to a NUL, text after it ignored, invalid UTF-8 read lossily, nesting
    /// deeper than 32 refused (the root and every value count, keys do not).
    #[test]
    fn tokener_as_json_c() {
        assert_eq!(tokener_parse(br#"{"a":1} tail"#), Some(serde_json::json!({"a": 1})));
        assert_eq!(tokener_parse(b"{\"a\":1}\0{"), Some(serde_json::json!({"a": 1})));
        assert_eq!(tokener_parse(b"{\"a\":\"\xff\"}"), Some(serde_json::json!({"a": "\u{fffd}"})));
        assert_eq!(tokener_parse(b"\0{}"), None);
        // raw control bytes inside a string, not after a backslash
        assert_eq!(tokener_parse(b"{\"a\":\"x\ny\x01\"}"), Some(serde_json::json!({"a": "x\ny\u{1}"})));
        assert_eq!(tokener_parse(b"{\"a\":\"\\\x01\"}"), None);
        assert_eq!(tokener_parse(b"{\"a\":\x01\"\"}"), None);
        let nested = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        assert!(tokener_parse(nested(31).as_bytes()).is_some());
        assert_eq!(tokener_parse(nested(32).as_bytes()), None);
        // a repeated key's earlier value still counts (the object is level 1); a NUL in a key is refused
        let repeated = |n: usize| format!(r#"{{"x":{},"x":1}}"#, nested(n));
        assert_eq!(tokener_parse(repeated(31).as_bytes()), None);
        assert_eq!(tokener_parse(repeated(30).as_bytes()), Some(serde_json::json!({"x": 1})));
        assert_eq!(tokener_parse(br#"{"a\u0000b":1}"#), None);
        assert_eq!(tokener_parse(br#"{"a":"b\u0000"}"#), Some(serde_json::json!({"a": "b\u{0}"})));
        // a minus zero is the integer 0; with a fraction or an exponent it is the double, and a text is a text
        let zeros = tokener_parse(br#"{"i":-0,"d":-0.0,"e":-0e0,"t":"-0","a":[-0,-0]}"#).expect("a document");
        assert_eq!(zeros["i"].as_u64(), Some(0));
        assert_eq!(zeros["a"], serde_json::json!([0, 0]));
        for member in ["d", "e"] {
            assert!(zeros[member].as_f64().is_some_and(|d| d == 0.0 && d.is_sign_negative()), "{member}");
            assert!(!zeros[member].is_u64());
        }
        assert_eq!(zeros["t"], "-0");
    }

    /// `json_parser_format_error()`: 235 bytes fit after the prefix, a longer text keeps 232 and `...`.
    #[test]
    fn parser_errors_are_cut_as_c() {
        let text = |n: usize| "e".repeat(n);
        let cut = |n: usize| String::from_utf8(format_error(text(n).as_bytes())).unwrap();
        assert_eq!(cut(235), format!("JSON parser failed: {}", text(235)));
        assert_eq!(cut(236), format!("JSON parser failed: {}...", text(232)));
        assert_eq!(cut(236).len(), 255);
        assert_eq!(format_error(b""), b"JSON parser failed: unknown error");
    }

    /// The three readers health's DynCfg payload needs: a double, a pattern text and a text for a fixed buffer.
    #[test]
    fn doubles_patterns_and_fixed_texts_as_c() {
        let o = obj(
            r#"{"d": 1.5, "i": 3, "big": 18446744073709551615, "t": true, "null": null, "s": "2.5e1", "sp": " 1",
                "tail": "1 ", "bad": "x", "empty": "", "huge": "1e400", "tiny": "1e-400", "inf": "inf", "arr": [],
                "star": "*", "p": "a=b", "n": 5, "long": "0123456789", "neg": -7, "f": 0.5}"#,
        );
        let mut e = String::new();
        let double_of = |member: &str, presence, e: &mut String| double(&o, "p", member, presence, e);
        assert_eq!(double_of("d", Required, &mut e), Some(Some(1.5)));
        assert_eq!(double_of("i", Required, &mut e), Some(Some(3.0)));
        // an integer past the signed range is read through json_object_get_int64()
        assert_eq!(double_of("big", Required, &mut e), Some(Some(i64::MAX as f64)));
        assert_eq!(double_of("t", Required, &mut e), Some(Some(1.0)));
        assert!(double_of("null", Required, &mut e).flatten().is_some_and(f64::is_nan));
        assert_eq!(double_of("s", Required, &mut e), Some(Some(25.0)));
        // strtod() skips leading spaces, and the whole text must be read
        assert_eq!(double_of("sp", Required, &mut e), Some(Some(1.0)));
        assert_eq!(double_of("inf", Optional, &mut e), Some(Some(f64::INFINITY)));
        assert_eq!(double_of("missing", Optional, &mut e), Some(None));
        assert_eq!(double_of("arr", Optional, &mut e), Some(None));
        assert_eq!(e, "");
        for (member, text) in [
            ("tail", "cannot convert string '1 ' to double for 'p.tail'"),
            ("bad", "cannot convert string 'x' to double for 'p.bad'"),
            ("empty", "cannot convert string '' to double for 'p.empty'"),
            ("huge", "cannot convert string '1e400' to double for 'p.huge'"),
            ("tiny", "cannot convert string '1e-400' to double for 'p.tiny'"),
        ] {
            // a text that is no number fails in both modes
            for presence in [Required, Optional] {
                let mut e = String::new();
                assert_eq!(double(&o, "p", member, presence, &mut e), None, "{member}");
                assert_eq!(e, text);
            }
        }
        let mut e = String::new();
        assert_eq!(double(&o, "p", "missing", Required, &mut e), None);
        assert_eq!(double(&o, "p", "arr", Required, &mut e), None);
        assert_eq!(e, "missing 'p.missing'cannot convert to double for 'p.arr'");

        assert_eq!(int_member(&o, "i"), Some(3));
        assert_eq!(int_member(&o, "neg"), Some(-7));
        assert_eq!(int_member(&o, "big"), Some(i64::MAX));
        assert_eq!(int_member(&o, "d"), None);
        assert_eq!(int_member(&o, "s"), None);
        assert_eq!(int_member(&o, "missing"), None);

        let mut e = String::new();
        assert_eq!(pattern(&o, "p", "p", Required, &mut e), Some(Some("a=b".to_owned())));
        assert_eq!(pattern(&o, "p", "star", Required, &mut e), Some(None));
        assert_eq!(pattern(&o, "p", "empty", Required, &mut e), Some(None));
        assert_eq!(pattern(&o, "p", "n", Optional, &mut e), Some(None));
        assert_eq!(pattern(&o, "p", "missing", Optional, &mut e), Some(None));
        assert_eq!(e, "");
        assert_eq!(pattern(&o, "p", "n", Required, &mut e), None);
        assert_eq!(pattern(&o, "p", "missing", Required, &mut e), None);
        assert_eq!(e, "invalid type for 'p.n' string" .to_owned() + "missing 'p.missing' string");

        let mut e = String::new();
        // cut as strncpyz() cuts: one byte less than the buffer
        assert_eq!(chars(&o, "p", "long", 8, Required, &mut e), Some(b"0123456".to_vec()));
        assert_eq!(chars(&o, "p", "long", 32, Required, &mut e), Some(b"0123456789".to_vec()));
        assert_eq!(chars(&o, "p", "n", 32, Required, &mut e), Some(b"5".to_vec()));
        assert_eq!(chars(&o, "p", "f", 32, Required, &mut e), Some(b"0.5".to_vec()));
        assert_eq!(chars(&o, "p", "t", 32, Required, &mut e), Some(b"true".to_vec()));
        assert_eq!(chars(&o, "p", "null", 32, Required, &mut e), Some(Vec::new()));
        assert_eq!(chars(&o, "p", "arr", 32, Optional, &mut e), Some(Vec::new()));
        assert_eq!(chars(&o, "p", "missing", 32, Optional, &mut e), Some(Vec::new()));
        assert_eq!(e, "");
        assert_eq!(chars(&o, "p", "arr", 32, Required, &mut e), None);
        assert_eq!(chars(&o, "p", "missing", 32, Required, &mut e), None);
        assert_eq!(e, "cannot convert to string for 'p.arr'missing 'p.missing'");

        // the bits of a wider destination
        let wide = |name: &[u8]| if name == b"a" { 1u64 << 40 } else { 0 };
        let names = obj(r#"{"o": ["a", "zz"]}"#);
        let mut e = String::new();
        assert_eq!(bitmap(&names, "p", "o", wide, Required, &mut e), Some(1u64 << 40));
        assert_eq!(e, "unknown option 'zz' in 'p.o' at index 1");
    }

    #[test]
    fn booleans_as_c() {
        let o = obj(
            r#"{"t": true, "yes": "YeS", "on": "on", "no": "No", "off": "OFF", "one": 1, "zero": 0,
                "half": 0.5, "null": null, "bad": "maybe", "arr": [], "nul": "true\u0000x"}"#,
        );
        let mut e = String::new();
        for (member, want) in [
            ("t", Some(true)),
            ("yes", Some(true)),
            ("on", Some(true)),
            ("no", Some(false)),
            ("off", Some(false)),
            ("one", Some(true)),
            ("zero", Some(false)),
            ("half", Some(true)),
            ("null", Some(false)),
            ("nul", Some(true)),
            ("arr", Some(false)),
            ("missing", Some(false)),
        ] {
            assert_eq!(boolean(&o, "", member, Optional, &mut e), want, "{member}");
        }
        assert_eq!(e, "");
        assert_eq!(boolean(&o, "", "bad", Optional, &mut e), None);
        assert_eq!(e, "invalid boolean string 'maybe' for '.bad'");
        for (member, text) in [
            ("arr", "cannot convert to boolean for '.arr'"),
            ("missing", "missing '.missing' boolean"),
        ] {
            let mut e = String::new();
            assert_eq!(boolean(&o, "", member, Required, &mut e), None);
            assert_eq!(e, text);
        }
    }

    #[test]
    fn rfc3339_as_c() {
        let o = obj(r#"{"ok": "2026-09-27T23:25:28.50Z", "bad": "yesterday", "num": 5}"#);
        let mut e = String::new();
        assert_eq!(rfc3339(&o, "", "ok", Required, &mut e), Some(1_790_551_528_500_000));
        for member in ["bad", "num", "missing"] {
            assert_eq!(rfc3339(&o, "", member, Optional, &mut e), Some(0), "{member}");
        }
        assert_eq!(e, "");
        for (member, text) in [
            ("bad", "invalid RFC3339 datetime for '.bad'"),
            ("num", "invalid type for '.num' string"),
            ("missing", "missing '.missing' string"),
        ] {
            let mut e = String::new();
            assert_eq!(rfc3339(&o, "", member, Required, &mut e), None);
            assert_eq!(e, text);
        }
    }

    #[test]
    fn enums_and_objects_as_c() {
        let o = obj(r#"{"s": "running", "n": 1, "o": {"a": 1}}"#);
        let mut e = String::new();
        assert_eq!(enum_text(&o, "", "s", Required, &mut e), Some(Some(&b"running"[..])));
        assert_eq!(enum_text(&o, "", "n", Optional, &mut e), Some(None));
        assert_eq!(enum_text(&o, "", "missing", Optional, &mut e), Some(None));
        assert_eq!(object(&o, "", "o", Required, &mut e).map(|o| o.map(Map::len)), Some(Some(1)));
        assert_eq!(object(&o, "", "s", Optional, &mut e), Some(None));
        assert_eq!(object(&o, "", "missing", Optional, &mut e), Some(None));
        assert_eq!(e, "");
        let enum_error = |member| {
            let mut e = String::new();
            assert!(enum_text(&o, "", member, Required, &mut e).is_none());
            e
        };
        let object_error = |member| {
            let mut e = String::new();
            assert!(object(&o, "", member, Required, &mut e).is_none());
            e
        };
        assert_eq!(enum_error("n"), "invalid type for '.n' enum");
        assert_eq!(enum_error("m"), "missing '.m' enum");
        assert_eq!(object_error("s"), "not an object '.s'");
        assert_eq!(object_error("m"), "missing '.m' object");
    }
    /// The walk's events, each as its debug text, and what the parse logged.
    fn walked(text: &str) -> (bool, Vec<String>, Vec<String>) {
        let mut seen = Vec::new();
        let (parsed, records) = netdata_agent_log::capture(|| {
            json_parse(text.as_bytes(), &mut |walked| {
                seen.push(match walked {
                    Walked::Element => "element".to_owned(),
                    Walked::ElementString { element, name, value } => {
                        format!("{element}:{}={}", String::from_utf8_lossy(name), String::from_utf8_lossy(value))
                    }
                    Walked::String { name, value } => {
                        format!("{}={}", String::from_utf8_lossy(name), String::from_utf8_lossy(value))
                    }
                    Walked::Boolean(v) => v.to_string(),
                })
            })
        });
        (parsed, seen, records.into_iter().filter_map(|record| record.message).collect())
    }

    #[test]
    fn the_walk_follows_the_document_s_order() {
        let events = |text| {
            let (parsed, seen, records) = walked(text);
            assert!(parsed && records.is_empty(), "{text}: {records:?}");
            seen.join(" ")
        };
        // members in the order they are written, whatever their names
        assert_eq!(events(r#"{"zz":true,"aa":false}"#), "true false");
        assert_eq!(
            events(r#"{"type":"SILENCE","all":true,"silencers":[{"b":"1","a":"2"}]}"#),
            "type=SILENCE true element 0:b=1 0:a=2"
        );
        // a repeated name keeps its first place and takes its last value
        assert_eq!(events(r#"{"aa":true,"zz":false,"aa":false}"#), "false false");
        assert_eq!(events(r#"{"x":[{"a":"1","a":"2"}]}"#), "element 0:a=2");
        // every array's elements are announced: an array in an element makes elements of its own, and the strings
        // after it are still the outer element's
        assert_eq!(events(r#"{"one":[{"a":"1"}],"two":[{"b":"2"}]}"#), "element 0:a=1 element 1:b=2");
        assert_eq!(events(r#"{"x":[{"in":[{"c":"3"}],"a":"1"}]}"#), "element element 1:c=3 0:a=1");
        // an object in an element is walked as a root: its strings have no element, its objects are not entered
        assert_eq!(
            events(r#"{"x":[{"o":{"type":"DISABLE","b":true,"o":{"b":false}},"a":"1"}]}"#),
            "element type=DISABLE true 0:a=1"
        );
        // not visited: a root's object, doubles, nulls, integers; a null element; an element that is no object
        assert_eq!(
            events(r#"{"o":{"b":true},"d":1.5,"n":null,"i":5,"x":[null,"s",5,true,[{"a":"1"}],{}]}"#),
            "element"
        );
        // a string is read up to its first NUL
        assert_eq!(events(r#"{"x":[{"a":"1\u00002"}]}"#), "element 0:a=1");
        // a root that is no object parses and gives nothing; what follows the first value is not read
        assert_eq!(events("5"), "");
        assert_eq!(events(r#""text""#), "");
        assert_eq!(events(r#"[{"a":"1"}]"#), "");
        assert_eq!(events(r#"{"b":true} trailing"#), "true");
    }

    #[test]
    fn a_text_that_is_refused_is_recorded() {
        let deep = format!("{}{}", "[".repeat(33), "]".repeat(33));
        for text in ["", " ", "{", "not json", "null", r#"{"a\u0000b":true}"#, "{'a':true}", r#"{"a":true,}"#, &deep] {
            let (parsed, seen, records) = walked(text);
            assert!(!parsed && seen.is_empty(), "{text}");
            assert_eq!(records, ["JSON: Invalid json string."], "{text}");
        }
    }
}
