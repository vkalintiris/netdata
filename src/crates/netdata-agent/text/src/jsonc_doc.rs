//! A JSON document as json-c 0.18 holds and prints one: what `/api/v3/settings` parses, edits and stores
//! (`json_tokener_parse()`, `json_object_object_get_ex()`, `json_object_get_int()`, `json_object_object_add()`,
//! `json_object_to_json_string()`). The bytes on disk are json-c's print of the document, so the members keep
//! their order, a number with a fraction or an exponent keeps its source text, and strings are bytes, not UTF-8.
//!
//! The grammar read is strict JSON (RFC 8259) with what json-c's tokener adds that no strict writer can produce
//! by accident: the text ends at its first NUL, whatever follows the root value is ignored, control bytes are taken
//! raw inside a string, and no byte is checked for being UTF-8. What json-c reads beyond that (comments, single
//! quotes, trailing commas, `NaN` and `Infinity`, leading zeros, literals in another case, `1.` and `-.5`, a lone
//! surrogate escape) is refused here: decision D234 F10 in the status repository. A key with a NUL in it, which
//! json-c cuts there, is refused too.

use crate::c::c_str;

/// `JSON_TOKENER_DEFAULT_DEPTH`: json-c refuses a value nested deeper.
const TOKENER_DEPTH: usize = 32;

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
    let mut reader = Reader { text: c_str(text), at: 0 };
    reader.whitespace();
    reader.value(1).filter(|root| *root != Value::Null)
}

struct Reader<'t> {
    text: &'t [u8],
    at: usize,
}

impl Reader<'_> {
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
            b'-' | b'0'..=b'9' => self.number(),
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
        let mut object = Value::Object(Vec::new());
        self.whitespace();
        if self.peek()? == b'}' {
            self.at += 1;
            return Some(object);
        }
        loop {
            self.whitespace();
            if self.peek()? != b'"' {
                return None;
            }
            let key = self.string()?;
            // json-c cuts a key at a NUL (only an escape can bring one); such a text is refused
            if key.contains(&0) {
                return None;
            }
            self.whitespace();
            if self.peek()? != b':' {
                return None;
            }
            self.at += 1;
            self.whitespace();
            let value = self.value(depth + 1)?;
            object.object_set(&key, value);
            self.whitespace();
            match self.peek()? {
                b',' => self.at += 1,
                b'}' => {
                    self.at += 1;
                    return Some(object);
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
    /// to its one character).
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
                            if (0xd800..0xdc00).contains(&code) {
                                // a leading surrogate needs its trailing one
                                if self.text.get(self.at..self.at + 2) != Some(b"\\u") {
                                    return None;
                                }
                                self.at += 2;
                                let low = self.hex4()?;
                                if !(0xdc00..0xe000).contains(&low) {
                                    return None;
                                }
                                code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
                            }
                            // (a trailing surrogate alone is no character)
                            let ch = char::from_u32(code)?;
                            out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
                        }
                        _ => return None,
                    }
                }
                _ => out.push(c),
            }
        }
    }

    /// A number in JSON's grammar: `-`? then `0` or digits without a leading zero, then an optional fraction and
    /// an optional exponent, each with at least one digit.
    fn number(&mut self) -> Option<Value> {
        let start = self.at;
        let digits = |reader: &mut Self| {
            let from = reader.at;
            while matches!(reader.peek(), Some(b'0'..=b'9')) {
                reader.at += 1;
            }
            reader.at - from
        };
        if self.peek() == Some(b'-') {
            self.at += 1;
        }
        match self.peek()? {
            b'0' => self.at += 1,
            b'1'..=b'9' => {
                digits(self);
            }
            _ => return None,
        }
        let mut is_double = false;
        if self.peek() == Some(b'.') {
            self.at += 1;
            if digits(self) == 0 {
                return None;
            }
            is_double = true;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if digits(self) == 0 {
                return None;
            }
            is_double = true;
        }
        // json-c's number goes on over these bytes where JSON's has ended (`0123`, `1.5.3`): refused, not cut
        if matches!(self.peek(), Some(b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')) {
            return None;
        }
        let text = &self.text[start..self.at];
        if is_double {
            return Some(Value::Double(text.to_vec()));
        }
        // json_parse_int64() and json_parse_uint64(): strtoll() and strtoull(), which saturate
        let digits = std::str::from_utf8(text).ok()?;
        Some(if let Some(magnitude) = digits.strip_prefix('-') {
            Value::Int(magnitude.parse::<u64>().map_or(i64::MIN, |m| 0i64.checked_sub_unsigned(m).unwrap_or(i64::MIN)))
        } else {
            match digits.parse::<u64>() {
                Ok(n) => i64::try_from(n).map_or(Value::Uint(n), Value::Int),
                Err(_) => Value::Uint(u64::MAX),
            }
        })
    }
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
