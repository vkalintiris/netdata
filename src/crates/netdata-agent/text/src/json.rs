//! The Agent's streaming JSON writer, mirroring the `buffer_json_*()` API of
//! `libnetdata/buffer/buffer.h` and `buffer.c`.
//!
//! Output is byte-identical to C: 4-space indentation (or none when minified),
//! commas decided by per-level item counters, keys and values quoted with
//! configurable quote strings (`""`/`'` for the Google JSON variant), and the
//! C escaping rules. Like C, the writer does not validate the call sequence:
//! members can be added to arrays and items to objects.

use crate::c;
use crate::datetime::{RFC3339_MAX_LENGTH, rfc3339_datetime_utc};
use crate::duration::duration_to_string;
use crate::print::{
    HEX_DIGITS, print_int64, print_netdata_double_or_null, print_uint64, print_uuid_lower,
    print_uuid_lower_compact,
};

/// `BUFFER_JSON_MAX_DEPTH`.
pub const JSON_MAX_DEPTH: usize = 32;

/// `BUFFER_QUOTE_MAX_SIZE`: quote strings are truncated to this many bytes.
pub const JSON_QUOTE_MAX_SIZE: usize = 7;

/// `API_RELATIVE_TIME_MAX`: timestamps up to this are relative (3 years).
pub const API_RELATIVE_TIME_MAX: i64 = 3 * 365 * 86_400;

/// `BUFFER_JSON_OPTIONS` flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonOptions(u8);

impl JsonOptions {
    /// `BUFFER_JSON_OPTIONS_DEFAULT`: pretty printed.
    pub const DEFAULT: Self = Self(0);
    /// `BUFFER_JSON_OPTIONS_MINIFY`.
    pub const MINIFY: Self = Self(1 << 0);
    /// `BUFFER_JSON_OPTIONS_NEWLINE_ON_ARRAY_ITEMS`.
    pub const NEWLINE_ON_ARRAY_ITEMS: Self = Self(1 << 1);
    /// `BUFFER_JSON_OPTIONS_NON_ANONYMOUS`: set when the root object was not opened.
    pub const NON_ANONYMOUS: Self = Self(1 << 2);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl std::ops::BitOr for JsonOptions {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeType {
    Empty,
    Object,
    Array,
}

/// `BUFFER_JSON_NODE`; `count` is a 24-bit field in C and wraps like it.
#[derive(Debug, Clone, Copy)]
struct Node {
    kind: NodeType,
    count: u32,
}

const COUNT_MASK: u32 = (1 << 24) - 1;

/// `buffer_json_strcat()`: escapes `"` and `\`, `\n \r \t \b \f`, and other
/// control bytes as `\u00XX` (uppercase hex); other bytes, including UTF-8
/// and DEL, pass through. Stops at a NUL byte.
pub fn json_escape(dst: &mut Vec<u8>, text: &[u8]) {
    for &ch in c::c_str(text) {
        match ch {
            b'\n' => dst.extend_from_slice(b"\\n"),
            b'\r' => dst.extend_from_slice(b"\\r"),
            b'\t' => dst.extend_from_slice(b"\\t"),
            0x08 => dst.extend_from_slice(b"\\b"),
            0x0c => dst.extend_from_slice(b"\\f"),
            0..=0x1f => {
                dst.extend_from_slice(b"\\u00");
                dst.push(HEX_DIGITS[usize::from(ch >> 4)]);
                dst.push(HEX_DIGITS[usize::from(ch & 0xf)]);
            }
            b'"' | b'\\' => {
                dst.push(b'\\');
                dst.push(ch);
            }
            _ => dst.push(ch),
        }
    }
}

/// `buffer_json_quoted_strcat()`: drops one leading `"` and a final `"`,
/// escapes `"` and `\` only (control bytes pass through). Stops at a NUL byte.
pub fn json_escape_quoted(dst: &mut Vec<u8>, text: &[u8]) {
    let text = c::c_str(text);
    let text = text.strip_prefix(b"\"").unwrap_or(text);
    let text = text.strip_suffix(b"\"").unwrap_or(text);
    for &ch in text {
        if ch == b'"' || ch == b'\\' {
            dst.push(b'\\');
        }
        dst.push(ch);
    }
}

/// A JSON document being written (`BUFFER` with its `json` state).
///
/// # Panics
///
/// Opening more than [`JSON_MAX_DEPTH`] levels panics (C calls `fatal()`), and
/// so does writing after closing more levels than were opened (undefined
/// behaviour in C).
#[derive(Debug, Clone)]
pub struct JsonWriter {
    buf: Vec<u8>,
    key_quote: Vec<u8>,
    value_quote: Vec<u8>,
    depth: i32,
    /// The level of the root object (the `depth` given at initialization).
    root: i32,
    options: JsonOptions,
    stack: [Node; JSON_MAX_DEPTH],
}

impl JsonWriter {
    /// `buffer_json_initialize(wb, "\"", "\"", 0, true, options)` on an empty buffer.
    pub fn new(options: JsonOptions) -> Self {
        Self::with_quotes(b"\"", b"\"", 0, true, options)
    }

    /// `buffer_json_initialize()` on an empty buffer.
    ///
    /// `depth` is the nesting level of the root object: writers started at
    /// `parent.depth() + 2` without an anonymous object produce members that
    /// are pasted raw into a parent document (see [`JsonWriter::raw`]); such
    /// writers are not finalized.
    pub fn with_quotes(
        key_quote: &[u8],
        value_quote: &[u8],
        depth: i32,
        add_anonymous_object: bool,
        options: JsonOptions,
    ) -> Self {
        let quote = |q: &[u8]| {
            let q = c::c_str(q);
            q[..q.len().min(JSON_QUOTE_MAX_SIZE)].to_vec()
        };
        let mut writer = JsonWriter {
            buf: Vec::new(),
            key_quote: quote(key_quote),
            value_quote: quote(value_quote),
            depth: depth.saturating_sub(1),
            root: depth,
            options,
            stack: [Node {
                kind: NodeType::Empty,
                count: 0,
            }; JSON_MAX_DEPTH],
        };
        writer.push(NodeType::Object);
        if add_anonymous_object {
            writer.buf.push(b'{');
        } else {
            writer.options = writer.options | JsonOptions::NON_ANONYMOUS;
        }
        writer
    }

    /// The bytes written so far.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }

    /// The written bytes.
    pub fn into_bytes(self) -> Vec<u8> {
        self.buf
    }

    /// The current nesting level (`wb->json.depth`).
    pub fn depth(&self) -> i32 {
        self.depth
    }

    /// Appends bytes verbatim (`buffer_strcat()` / `buffer_fast_strcat()`).
    pub fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    fn top(&mut self) -> &mut Node {
        let depth = usize::try_from(self.depth).expect("BUFFER JSON: nothing is open");
        &mut self.stack[depth]
    }

    /// `_buffer_json_depth_push()`; C calls `fatal()` past the maximum depth.
    fn push(&mut self, kind: NodeType) {
        let next = self.depth + 1;
        if next < 0 || next as usize >= JSON_MAX_DEPTH {
            panic!(
                "BUFFER JSON: invalid nesting depth {} (next {}, max {})",
                self.depth,
                next,
                JSON_MAX_DEPTH - 1
            );
        }
        self.depth = next;
        self.stack[next as usize] = Node { kind, count: 0 };
    }

    fn count_item(&mut self) {
        let top = self.top();
        top.count = (top.count + 1) & COUNT_MASK;
    }

    fn spaces(&mut self, levels: i32) {
        for _ in 0..levels.max(0) {
            self.buf.extend_from_slice(b"    ");
        }
    }

    /// `buffer_print_json_comma()`.
    fn comma(&mut self) {
        if self.top().count != 0 {
            self.buf.push(b',');
        }
    }

    /// `buffer_print_json_comma_newline_spacing()`.
    fn comma_newline_spacing(&mut self) {
        self.comma();
        let in_array = self.top().kind == NodeType::Array;
        if self.options.contains(JsonOptions::MINIFY)
            || (in_array && !self.options.contains(JsonOptions::NEWLINE_ON_ARRAY_ITEMS))
        {
            return;
        }
        self.buf.push(b'\n');
        self.spaces(self.depth + 1);
    }

    /// `buffer_print_json_key()`.
    fn key(&mut self, key: &[u8]) {
        self.buf.extend_from_slice(&self.key_quote);
        json_escape(&mut self.buf, key);
        self.buf.extend_from_slice(&self.key_quote);
    }

    /// `buffer_json_add_string_value()`.
    fn string_value(&mut self, value: Option<&[u8]>) {
        match value {
            Some(value) => {
                self.buf.extend_from_slice(&self.value_quote);
                json_escape(&mut self.buf, value);
                self.buf.extend_from_slice(&self.value_quote);
            }
            None => self.buf.extend_from_slice(b"null"),
        }
    }

    /// Starts a member: separator, key and colon.
    fn member(&mut self, key: &[u8]) {
        self.comma_newline_spacing();
        self.key(key);
        self.buf.push(b':');
    }

    /// Writes a member whose value `write` emits, then counts it.
    fn member_with(&mut self, key: &[u8], write: impl FnOnce(&mut Self)) {
        self.member(key);
        write(self);
        self.count_item();
    }

    /// Writes an array item whose value `write` emits, then counts it.
    fn item_with(&mut self, write: impl FnOnce(&mut Self)) {
        self.comma_newline_spacing();
        write(self);
        self.count_item();
    }

    // --------------------------------------------------------------------------------------------
    // objects and arrays

    /// `buffer_json_member_add_object()`.
    pub fn member_add_object(&mut self, key: &[u8]) {
        self.comma_newline_spacing();
        self.key(key);
        self.buf.extend_from_slice(b":{");
        self.count_item();
        self.push(NodeType::Object);
    }

    /// `buffer_json_object_close()`.
    pub fn object_close(&mut self) {
        debug_assert_eq!(
            self.top().kind,
            NodeType::Object,
            "BUFFER JSON: an object is not open to close it"
        );
        if !self.options.contains(JsonOptions::MINIFY) {
            self.buf.push(b'\n');
            self.spaces(self.depth);
        }
        self.buf.push(b'}');
        self.depth -= 1;
    }

    /// `buffer_json_member_add_array()`; `None` writes a bare `[`.
    pub fn member_add_array(&mut self, key: Option<&[u8]>) {
        self.comma_newline_spacing();
        match key {
            Some(key) => {
                self.key(key);
                self.buf.extend_from_slice(b":[");
            }
            None => self.buf.push(b'['),
        }
        self.count_item();
        self.push(NodeType::Array);
    }

    /// `buffer_json_array_close()`.
    pub fn array_close(&mut self) {
        debug_assert_eq!(
            self.top().kind,
            NodeType::Array,
            "BUFFER JSON: an array is not open to close it"
        );
        if self.options.contains(JsonOptions::NEWLINE_ON_ARRAY_ITEMS) {
            self.buf.push(b'\n');
            self.spaces(self.depth);
        }
        self.buf.push(b']');
        self.depth -= 1;
    }

    /// `buffer_json_add_array_item_array()`: nested arrays go on their own
    /// line unless minified.
    pub fn add_array_item_array(&mut self) {
        if !self.options.contains(JsonOptions::MINIFY) && self.top().kind == NodeType::Array {
            self.comma();
            self.buf.push(b'\n');
            self.spaces(self.depth + 1);
        } else {
            self.comma_newline_spacing();
        }
        self.buf.push(b'[');
        self.count_item();
        self.push(NodeType::Array);
    }

    /// `buffer_json_add_array_item_object()`.
    pub fn add_array_item_object(&mut self) {
        self.comma_newline_spacing();
        self.buf.push(b'{');
        self.count_item();
        self.push(NodeType::Object);
    }

    /// `buffer_json_finalize()`: closes everything still open, the root
    /// object only if it was opened, and adds a newline unless minified.
    ///
    /// C loops forever when the writer was initialized at a depth above 0
    /// (the levels below it are empty); this stops at the root instead.
    pub fn finalize(&mut self) {
        while self.depth >= self.root.max(0) {
            match self.stack[self.depth as usize].kind {
                NodeType::Object
                    if self.depth == self.root
                        && self.options.contains(JsonOptions::NON_ANONYMOUS) =>
                {
                    self.depth -= 1;
                }
                NodeType::Object => self.object_close(),
                NodeType::Array => self.array_close(),
                NodeType::Empty => break,
            }
        }
        if !self.options.contains(JsonOptions::MINIFY) {
            self.buf.push(b'\n');
        }
    }

    // --------------------------------------------------------------------------------------------
    // members

    /// `buffer_json_member_add_string()` with a non-NULL value.
    pub fn member_add_string(&mut self, key: &[u8], value: &[u8]) {
        self.member_with(key, |w| w.string_value(Some(value)));
    }

    /// `buffer_json_member_add_string(wb, key, NULL)`.
    pub fn member_add_null(&mut self, key: &[u8]) {
        self.member_with(key, |w| w.string_value(None));
    }

    /// `buffer_json_member_add_string()`: `None` is `null`.
    pub fn member_add_string_opt(&mut self, key: &[u8], value: Option<&[u8]>) {
        self.member_with(key, |w| w.string_value(value));
    }

    /// `buffer_json_member_add_string_or_omit()`: nothing for `None` or empty.
    pub fn member_add_string_or_omit(&mut self, key: &[u8], value: Option<&[u8]>) {
        if let Some(value) = value.filter(|v| !c::c_str(v).is_empty()) {
            self.member_add_string(key, value);
        }
    }

    /// `buffer_json_member_add_string_or_empty()`: `None` becomes `""`.
    pub fn member_add_string_or_empty(&mut self, key: &[u8], value: Option<&[u8]>) {
        self.member_add_string(key, value.unwrap_or_default());
    }

    /// `buffer_json_member_add_quoted_string()`: `None` and `null` are
    /// `null`; otherwise surrounding quotes are dropped and only `"` and `\`
    /// are escaped.
    pub fn member_add_quoted_string(&mut self, key: &[u8], value: Option<&[u8]>) {
        self.member_with(key, |w| match value {
            Some(value) if c::c_str(value) != b"null" => {
                w.buf.extend_from_slice(&w.value_quote);
                json_escape_quoted(&mut w.buf, value);
                w.buf.extend_from_slice(&w.value_quote);
            }
            _ => w.buf.extend_from_slice(b"null"),
        });
    }

    /// `buffer_json_member_add_uuid()`: lowercase with dashes, `null` for the nil UUID.
    pub fn member_add_uuid(&mut self, key: &[u8], uuid: &[u8; 16]) {
        self.member_with(key, |w| w.uuid_value(Some(uuid), print_uuid_lower));
    }

    /// `buffer_json_member_add_uuid_ptr()`: `null` for `None` or the nil UUID.
    pub fn member_add_uuid_ptr(&mut self, key: &[u8], uuid: Option<&[u8; 16]>) {
        self.member_with(key, |w| w.uuid_value(uuid, print_uuid_lower));
    }

    /// `buffer_json_member_add_uuid_compact()`: 32 lowercase hex digits, `null` for the nil UUID.
    pub fn member_add_uuid_compact(&mut self, key: &[u8], uuid: &[u8; 16]) {
        self.member_with(key, |w| w.uuid_value(Some(uuid), print_uuid_lower_compact));
    }

    fn uuid_value(&mut self, uuid: Option<&[u8; 16]>, format: fn(&mut Vec<u8>, &[u8; 16])) {
        match uuid.filter(|u| u.iter().any(|&b| b != 0)) {
            Some(uuid) => {
                let mut text = Vec::with_capacity(36);
                format(&mut text, uuid);
                self.string_value(Some(&text));
            }
            None => self.string_value(None),
        }
    }

    /// `buffer_json_member_add_boolean()`.
    pub fn member_add_boolean(&mut self, key: &[u8], value: bool) {
        self.member_with(key, |w| {
            w.buf
                .extend_from_slice(if value { b"true" } else { b"false" })
        });
    }

    /// `buffer_json_member_add_uint64()`.
    pub fn member_add_uint64(&mut self, key: &[u8], value: u64) {
        self.member_with(key, |w| print_uint64(&mut w.buf, value));
    }

    /// `buffer_json_member_add_int64()`.
    pub fn member_add_int64(&mut self, key: &[u8], value: i64) {
        self.member_with(key, |w| print_int64(&mut w.buf, value));
    }

    /// `buffer_json_member_add_double()`: `null` for NaN and infinities.
    pub fn member_add_double(&mut self, key: &[u8], value: f64) {
        self.member_with(key, |w| print_netdata_double_or_null(&mut w.buf, value));
    }

    /// `buffer_json_member_add_time_t()`.
    pub fn member_add_time_t(&mut self, key: &[u8], value: i64) {
        self.member_add_int64(key, value);
    }

    /// `buffer_json_member_add_time_t_formatted()`: with `rfc3339`, 0 is
    /// `null` and absolute times (above [`API_RELATIVE_TIME_MAX`]) are UTC
    /// RFC 3339 strings; everything else is the plain number.
    pub fn member_add_time_t_formatted(&mut self, key: &[u8], value: i64, rfc3339: bool) {
        if rfc3339 && value == 0 {
            self.member_add_null(key);
        } else if rfc3339 && value > API_RELATIVE_TIME_MAX {
            self.member_add_datetime_rfc3339_utc(key, (value as u64).wrapping_mul(1_000_000));
        } else {
            self.member_add_time_t(key, value);
        }
    }

    /// `buffer_json_member_add_datetime_rfc3339(wb, key, datetime_ut, true)`.
    pub fn member_add_datetime_rfc3339_utc(&mut self, key: &[u8], datetime_ut: u64) {
        let text = rfc3339_datetime_utc(datetime_ut, 2);
        debug_assert!(text.len() < RFC3339_MAX_LENGTH);
        self.member_add_string(key, text.as_bytes());
    }

    /// `buffer_json_member_add_duration_ut()`: e.g. `1h30m`, `off` for 0.
    pub fn member_add_duration_ut(&mut self, key: &[u8], duration_ut: i64) {
        let text = duration_to_string(duration_ut, "us", true).unwrap_or_default();
        self.member_add_string(key, text.as_bytes());
    }

    // --------------------------------------------------------------------------------------------
    // array items

    /// `buffer_json_add_array_item_string()` with a non-NULL value.
    pub fn add_array_item_string(&mut self, value: &[u8]) {
        self.item_with(|w| w.string_value(Some(value)));
    }

    /// `buffer_json_add_array_item_string(wb, NULL)`.
    pub fn add_array_item_null(&mut self) {
        self.item_with(|w| w.string_value(None));
    }

    /// `buffer_json_add_array_item_string()`: `None` is `null`.
    pub fn add_array_item_string_opt(&mut self, value: Option<&[u8]>) {
        self.item_with(|w| w.string_value(value));
    }

    /// `buffer_json_add_array_item_uuid()`: `null` for `None` or the nil UUID.
    pub fn add_array_item_uuid(&mut self, uuid: Option<&[u8; 16]>) {
        self.item_with(|w| w.uuid_value(uuid, print_uuid_lower));
    }

    /// `buffer_json_add_array_item_uuid_compact()`: `null` for `None` or the nil UUID.
    pub fn add_array_item_uuid_compact(&mut self, uuid: Option<&[u8; 16]>) {
        self.item_with(|w| w.uuid_value(uuid, print_uuid_lower_compact));
    }

    /// `buffer_json_add_array_item_double()`: `null` for NaN and infinities.
    pub fn add_array_item_double(&mut self, value: f64) {
        self.item_with(|w| print_netdata_double_or_null(&mut w.buf, value));
    }

    /// `buffer_json_add_array_item_int64()`.
    pub fn add_array_item_int64(&mut self, value: i64) {
        self.item_with(|w| print_int64(&mut w.buf, value));
    }

    /// `buffer_json_add_array_item_uint64()`.
    pub fn add_array_item_uint64(&mut self, value: u64) {
        self.item_with(|w| print_uint64(&mut w.buf, value));
    }

    /// `buffer_json_add_array_item_boolean()`.
    pub fn add_array_item_boolean(&mut self, value: bool) {
        self.item_with(|w| {
            w.buf
                .extend_from_slice(if value { b"true" } else { b"false" })
        });
    }

    /// `buffer_json_add_array_item_time_t()`.
    pub fn add_array_item_time_t(&mut self, value: i64) {
        self.add_array_item_int64(value);
    }

    /// `buffer_json_add_array_item_time_ms()`: seconds printed as milliseconds.
    pub fn add_array_item_time_ms(&mut self, value: i64) {
        self.item_with(|w| {
            print_int64(&mut w.buf, value);
            w.buf.extend_from_slice(b"000");
        });
    }

    /// `buffer_json_add_array_item_time_t_formatted()`; see
    /// [`JsonWriter::member_add_time_t_formatted`].
    pub fn add_array_item_time_t_formatted(&mut self, value: i64, rfc3339: bool) {
        if rfc3339 && value == 0 {
            self.add_array_item_null();
        } else if rfc3339 && value > API_RELATIVE_TIME_MAX {
            self.add_array_item_datetime_rfc3339_utc((value as u64).wrapping_mul(1_000_000));
        } else {
            self.add_array_item_time_t(value);
        }
    }

    /// `buffer_json_add_array_item_datetime_rfc3339(wb, datetime_ut, true)`.
    pub fn add_array_item_datetime_rfc3339_utc(&mut self, datetime_ut: u64) {
        let text = rfc3339_datetime_utc(datetime_ut, 2);
        self.add_array_item_string(text.as_bytes());
    }
}
