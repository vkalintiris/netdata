//! A JSON document as json-c 0.18 holds and prints one: what `/api/v3/settings` parses, edits and stores
//! (`json_tokener_parse()`, `json_object_object_get_ex()`, `json_object_get_int()`, `json_object_object_add()`,
//! `json_object_to_json_string()`). The bytes on disk are json-c's print of the document, so the members keep
//! their order, a number with a fraction or an exponent keeps its source text, and strings are bytes, not UTF-8.
//!
//! The grammar read is JSON (RFC 8259) as json-c's tokener reads it where a strict writer can end up: the text
//! ends at its first NUL; what follows the root value is ignored, unless it starts (after whitespace) with a `/`,
//! where json-c looks for a comment and fails the text when there is none (and a comment is refused here);
//! control bytes are taken raw inside a string, and no byte is checked for being UTF-8; a surrogate escape
//! without its pair is U+FFFD; a key ends at a NUL an escape brought. What json-c reads beyond that (comments,
//! single quotes, trailing commas, `NaN` and `Infinity`, leading zeros, literals in another case, `1.` and
//! `-.5`, an exponent without digits such as `1e`, `1e+` or `1.0e`) is refused here: decisions D234 F10, D240 and
//! D242 in the status repository.
//!
//! [`parse_errno`] and [`Value::get_int_errno`] also say what json-c leaves in the thread's `errno`, which C's
//! logger attaches to the next record (D242).

use std::collections::HashMap;

use crate::c::c_str;

/// `JSON_TOKENER_DEFAULT_DEPTH`: json-c refuses a value nested deeper.
const TOKENER_DEPTH: usize = 32;

/// Linux's `ERANGE` and `EINVAL`.
const ERANGE: i32 = 34;
const EINVAL: i32 = 22;

/// A json-c object (`json_object`).
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// A number without fraction and exponent that fits json-c's signed integer: negative ones saturate at the
    /// lowest (`-0` is 0).
    Int(i64),
    /// One above the signed range: json-c keeps it unsigned, saturated at the highest.
    Uint(u64),
    /// A number with a fraction or an exponent: its source text, which json-c prints back as it was.
    Double(Vec<u8>),
    String(Vec<u8>),
    Array(Vec<Value>),
    /// The members in the order of their first appearance; a repeated key has its last value.
    Object(Vec<(Vec<u8>, Value)>),
}

/// A json-c string (`json_escape_str()`): `"`, `\` and `/` behind a backslash, the five short escapes, every other
/// byte below 0x20 as a lower-case `\u00xx`, every other byte as it is.
pub fn jsonc_string(out: &mut Vec<u8>, text: &[u8]) {
    out.push(b'"');
    for &ch in text {
        match ch {
            0x08 => out.extend_from_slice(b"\\b"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'\t' => out.extend_from_slice(b"\\t"),
            0x0c => out.extend_from_slice(b"\\f"),
            b'"' | b'\\' | b'/' => out.extend_from_slice(&[b'\\', ch]),
            0..0x20 => out.extend_from_slice(format!("\\u{ch:04x}").as_bytes()),
            _ => out.push(ch),
        }
    }
    out.push(b'"');
}

/// `json_tokener_parse()`: the first JSON value of `text`; `None` where json-c gives NULL, and for what this
/// reader refuses of json-c's leniencies (the module's text). json-c's null value is its NULL object, so a text
/// whose root is `null` gives the caller what a text that does not parse gives.
pub fn parse(text: &[u8]) -> Option<Value> {
    parse_errno(text).0
}

/// [`parse`], and the `errno` json-c leaves after it: `None` when no number json-c converted touched it (the
/// thread's stays), else what the last one left. An integer is read by `json_parse_int64()` or
/// `json_parse_uint64()`, which clear it, then leave `ERANGE` for one beyond 64 bits and `EINVAL` for a `-` without
/// digits; a number with a fraction or an exponent by `strtod()`, which leaves `ERANGE` for one out of range or tiny
/// and inexact, and clears nothing. In an array or an object json-c fails the text at a byte that cannot end a
/// number before it converts the number. A text that fails keeps what the numbers before the failure left; where
/// this reader refuses one of json-c's leniencies, the numbers json-c would convert after it are not followed
/// (D243).
pub fn parse_errno(text: &[u8]) -> (Option<Value>, Option<i32>) {
    let mut reader = Reader { text: c_str(text), at: 0, errno: None };
    let root = reader.document();
    (root, reader.errno)
}

struct Reader<'t> {
    text: &'t [u8],
    at: usize,
    /// What the numbers read so far left in json-c's `errno`.
    errno: Option<i32>,
}

impl Reader<'_> {
    fn document(&mut self) -> Option<Value> {
        self.whitespace();
        let root = self.value(1)?;
        // after the root json-c reads on over whitespace and comments, and fails the text at a `/` that opens none
        self.whitespace();
        if self.peek() == Some(b'/') {
            return None;
        }
        (root != Value::Null).then_some(root)
    }

    fn peek(&self) -> Option<u8> {
        self.text.get(self.at).copied()
    }

    fn whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.at += 1;
        }
    }

    /// The literal `word`, if the text goes on with it.
    fn literal(&mut self, word: &[u8], value: Value) -> Option<Value> {
        self.text[self.at..].starts_with(word).then(|| {
            self.at += word.len();
            value
        })
    }

    /// A value that starts here, `depth` containers deep counting itself.
    fn value(&mut self, depth: usize) -> Option<Value> {
        if depth > TOKENER_DEPTH {
            return None;
        }
        match self.peek()? {
            b'n' => self.literal(b"null", Value::Null),
            b't' => self.literal(b"true", Value::Bool(true)),
            b'f' => self.literal(b"false", Value::Bool(false)),
            b'"' => self.string().map(Value::String),
            b'[' => self.array(depth),
            b'{' => self.object(depth),
            b'-' | b'0'..=b'9' => self.number(depth),
            _ => None,
        }
    }

    fn array(&mut self, depth: usize) -> Option<Value> {
        self.at += 1;
        let mut items = Vec::new();
        self.whitespace();
        if self.peek()? == b']' {
            self.at += 1;
            return Some(Value::Array(items));
        }
        loop {
            self.whitespace();
            items.push(self.value(depth + 1)?);
            self.whitespace();
            match self.peek()? {
                b',' => self.at += 1,
                b']' => {
                    self.at += 1;
                    return Some(Value::Array(items));
                }
                _ => return None,
            }
        }
    }

    fn object(&mut self, depth: usize) -> Option<Value> {
        self.at += 1;
        // the members in their order, and where each key stands: a repeated key takes its new value there
        let mut members: Vec<(Vec<u8>, Value)> = Vec::new();
        let mut places: HashMap<Vec<u8>, usize> = HashMap::new();
        self.whitespace();
        if self.peek()? == b'}' {
            self.at += 1;
            return Some(Value::Object(members));
        }
        loop {
            self.whitespace();
            if self.peek()? != b'"' {
                return None;
            }
            // json-c holds a key as a C string: it ends at a NUL (only an escape can bring one)
            let mut key = self.string()?;
            key.truncate(c_str(&key).len());
            self.whitespace();
            if self.peek()? != b':' {
                return None;
            }
            self.at += 1;
            self.whitespace();
            let value = self.value(depth + 1)?;
            match places.get(&key) {
                Some(&place) => members[place].1 = value,
                None => {
                    places.insert(key.clone(), members.len());
                    members.push((key, value));
                }
            }
            self.whitespace();
            match self.peek()? {
                b',' => self.at += 1,
                b'}' => {
                    self.at += 1;
                    return Some(Value::Object(members));
                }
                _ => return None,
            }
        }
    }

    /// Four hex digits of a `\u` escape.
    fn hex4(&mut self) -> Option<u32> {
        let digits = self.text.get(self.at..self.at + 4)?;
        let mut code = 0;
        for &digit in digits {
            code = code * 16 + char::from(digit).to_digit(16)?;
        }
        self.at += 4;
        Some(code)
    }

    /// A string, from its opening quote: its bytes, the escapes decoded (a `\u` escape to UTF-8, a surrogate pair
    /// to its one character). A leading surrogate that no trailing one follows in the next `\u` escape is U+FFFD,
    /// and what follows it is read as it stands; a trailing surrogate alone is U+FFFD.
    fn string(&mut self) -> Option<Vec<u8>> {
        self.at += 1;
        let mut out = Vec::new();
        loop {
            let c = self.peek()?;
            self.at += 1;
            match c {
                b'"' => return Some(out),
                b'\\' => {
                    let escape = self.peek()?;
                    self.at += 1;
                    match escape {
                        b'"' | b'\\' | b'/' => out.push(escape),
                        b'b' => out.push(0x08),
                        b'f' => out.push(0x0c),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            while (0xd800..0xdc00).contains(&code) {
                                if self.text.get(self.at..self.at + 2) != Some(b"\\u") {
                                    code = 0xfffd;
                                    break;
                                }
                                self.at += 2;
                                let next = self.hex4()?;
                                if (0xdc00..0xe000).contains(&next) {
                                    code = 0x10000 + ((code - 0xd800) << 10) + (next - 0xdc00);
                                    break;
                                }
                                out.extend_from_slice("\u{fffd}".as_bytes());
                                code = next;
                            }
                            // (a trailing surrogate alone is no character)
                            let ch = char::from_u32(code).unwrap_or('\u{fffd}');
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return None,
                    }
                }
                _ => out.push(c),
            }
        }
    }

    /// A number as json-c's tokener reads one, `depth` containers deep counting itself (`json_tokener.c`, the
    /// number state): the bytes it takes, whether it converts them, and what the conversion leaves in `errno`. The
    /// number is kept only when those bytes are JSON's number and no `.`, `e`, `E` or sign follows them.
    fn number(&mut self, depth: usize) -> Option<Value> {
        let start = self.at;
        // json-c takes digits, one `.` before an exponent, one `e` or `E`, a `-` first, and a sign after the `.` or
        // the `e`
        let (mut is_double, mut is_exponent, mut minus_ok, mut plus_ok) = (false, false, true, false);
        while let Some(c) = self.peek() {
            let takes = c.is_ascii_digit()
                || (!is_exponent && matches!(c, b'e' | b'E'))
                || (minus_ok && c == b'-')
                || (plus_ok && c == b'+')
                || (!is_double && c == b'.');
            if !takes {
                break;
            }
            self.at += 1;
            let marker = matches!(c, b'.' | b'e' | b'E');
            (minus_ok, plus_ok) = (marker, marker);
            is_double |= marker;
            is_exponent |= matches!(c, b'e' | b'E');
        }
        let taken = &self.text[start..self.at];
        let next = self.peek();
        // in a container json-c fails the text at any other byte, before converting anything
        if depth > 1 && !next.is_some_and(ends_a_number) {
            return None;
        }
        // `-Infinity`
        if taken == b"-" && matches!(next, Some(b'I' | b'i')) {
            return None;
        }
        let value = if is_double {
            // json-c drops a trailing exponent marker or sign; strtod() must take the rest whole, and leaves
            // `ERANGE` or nothing
            let mut text = taken;
            while let [rest @ .., b'e' | b'E' | b'+' | b'-'] = text
                && !rest.is_empty()
            {
                text = rest;
            }
            let (_, used, erange) = crate::parse::strtod_range(text);
            if erange {
                self.errno = Some(ERANGE);
            }
            (used == text.len()).then(|| Value::Double(text.to_vec()))
        } else {
            // json_parse_int64() and json_parse_uint64(): strtoll() and strtoull(), which clear `errno` and
            // saturate with `ERANGE`; a `-` without digits is `EINVAL`
            let (value, used, erange) = if taken[0] == b'-' {
                let (n, used, erange) = crate::parse::strtoll10(taken);
                (Value::Int(n), used, erange)
            } else {
                let (n, used, erange) = crate::parse::strtoull10(taken);
                (i64::try_from(n).map_or(Value::Uint(n), Value::Int), used, erange)
            };
            self.errno = Some(if used == 0 {
                EINVAL
            } else if erange {
                ERANGE
            } else {
                0
            });
            (used != 0).then_some(value)
        };
        // what json-c reads beyond JSON's number is refused (the module's text): at the root it would ignore the
        // rest after `1.5` in `1.5.3`, so that is refused too
        if !is_json_number(taken) || matches!(next, Some(b'.' | b'e' | b'E' | b'+' | b'-')) {
            return None;
        }
        value
    }
}

/// Whether json-c's tokener lets a number in an array or an object end before the byte `c`: at any other byte, and
/// at the end of the text, it fails the text (`json_tokener.c`, the number state).
pub fn ends_a_number(c: u8) -> bool {
    matches!(c, b',' | b']' | b'}' | b'/' | b'I' | b'i' | b' ' | b'\t' | b'\n' | b'\r')
}

/// JSON's number (RFC 8259): `-`? then `0` or digits without a leading zero, then an optional fraction and an
/// optional exponent, each with at least one digit.
fn is_json_number(text: &[u8]) -> bool {
    let digits = |text: &[u8]| text.iter().take_while(|c| c.is_ascii_digit()).count();
    let mut rest = text.strip_prefix(b"-").unwrap_or(text);
    let whole = digits(rest);
    if whole == 0 || (whole > 1 && rest[0] == b'0') {
        return false;
    }
    rest = &rest[whole..];
    if let Some(fraction) = rest.strip_prefix(b".") {
        let count = digits(fraction);
        if count == 0 {
            return false;
        }
        rest = &fraction[count..];
    }
    if let Some(exponent) = rest.strip_prefix(b"e").or_else(|| rest.strip_prefix(b"E")) {
        let exponent = exponent.strip_prefix(b"+").or_else(|| exponent.strip_prefix(b"-")).unwrap_or(exponent);
        let count = digits(exponent);
        if count == 0 {
            return false;
        }
        rest = &exponent[count..];
    }
    rest.is_empty()
}

impl Value {
    /// `json_object_object_get_ex()`: the member's value; none when this is not an object.
    pub fn object_get(&self, key: &[u8]) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(name, _)| name == key).map(|(_, value)| value),
            _ => None,
        }
    }

    /// `json_object_object_add()`: the member takes the value where it is, or is added at the end. Nothing when
    /// this is not an object.
    pub fn object_set(&mut self, key: &[u8], value: Value) {
        if let Value::Object(members) = self {
            match members.iter_mut().find(|(name, _)| name == key) {
                Some(member) => member.1 = value,
                None => members.push((key.to_vec(), value)),
            }
        }
    }

    /// `json_object_get_int()`: an integer clamped to 32 bits; a double truncated and clamped; a string as
    /// `strtoll()` reads its start (0 when it has no number); a boolean 0 or 1; anything else 0.
    pub fn get_int(&self) -> i32 {
        let clamp = |n: i64| n.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        match self {
            Value::Int(n) => clamp(*n),
            Value::Uint(_) => i32::MAX,
            Value::Double(text) => {
                // (a number of JSON's grammar always parses; one beyond the doubles is an infinity)
                let d = std::str::from_utf8(text).ok().and_then(|text| text.parse::<f64>().ok()).unwrap_or(0.0);
                d as i32
            }
            Value::String(text) => {
                // json_parse_int64(): no digits is a failure, 0; out of range saturates
                let (n, used, _) = crate::parse::strtoll10(c_str(text));
                if used == 0 { 0 } else { clamp(n) }
            }
            Value::Bool(b) => i32::from(*b),
            Value::Null | Value::Array(_) | Value::Object(_) => 0,
        }
    }

    /// [`Value::get_int`], and the `errno` json-c leaves: a string is read by `json_parse_int64()`, which clears it,
    /// then leaves `ERANGE` for a number out of range and `EINVAL` for none; nothing else touches it (`None`).
    pub fn get_int_errno(&self) -> (i32, Option<i32>) {
        let errno = match self {
            Value::String(text) => {
                let (_, used, erange) = crate::parse::strtoll10(c_str(text));
                Some(if used == 0 {
                    EINVAL
                } else if erange {
                    ERANGE
                } else {
                    0
                })
            }
            _ => None,
        };
        (self.get_int(), errno)
    }

    /// `json_object_to_json_string()`: json-c's spaced print.
    pub fn print_spaced(&self, out: &mut Vec<u8>) {
        match self {
            Value::Null => out.extend_from_slice(b"null"),
            Value::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Value::Int(n) => out.extend_from_slice(n.to_string().as_bytes()),
            Value::Uint(n) => out.extend_from_slice(n.to_string().as_bytes()),
            Value::Double(text) => out.extend_from_slice(text),
            Value::String(text) => jsonc_string(out, text),
            Value::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    out.extend_from_slice(if i == 0 { b" " } else { b", " });
                    item.print_spaced(out);
                }
                out.extend_from_slice(b" ]");
            }
            Value::Object(members) => {
                out.push(b'{');
                for (i, (key, value)) in members.iter().enumerate() {
                    out.extend_from_slice(if i == 0 { b" " } else { b", " });
                    jsonc_string(out, key);
                    out.extend_from_slice(b": ");
                    value.print_spaced(out);
                }
                out.extend_from_slice(b" }");
            }
        }
    }
}
