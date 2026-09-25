//! A record's fields and the three encodings: logfmt (`nd_log-format-logfmt.c`), json (`nd_log-format-json.c`) and
//! the journal's native protocol (`nd_logger_journal_direct()` in `nd_log-to-systemd-journal.c`). Every encoder walks
//! the fields in id order.

use netdata_agent_text::c::c_str;
use netdata_agent_text::datetime::{CivilTime, rfc3339_datetime_local};
use netdata_agent_text::json::{JsonOptions, JsonWriter, json_escape};
use netdata_agent_text::print::{
    print_int64, print_netdata_double_or_null, print_uint64, print_uuid_lower_compact,
};

use crate::frame::{Lazy, Value};
use crate::model::{Annotator, FIELD_TABLE, FIELDS, Priority};

/// One field of a record, borrowed from a frame or set by the logger.
#[derive(Clone, Copy)]
pub(crate) enum Slot<'a> {
    /// Text that prints nothing when empty (automatic text fields are set even when empty).
    Txt(&'a str),
    /// A `STRING *`: `key=""` in logfmt when empty.
    Str(&'a str),
    U64(u64),
    I64(i64),
    Dbl(f64),
    Uuid(&'a [u8; 16]),
    Lazy(&'a Lazy),
}

impl<'a> From<&'a Value> for Slot<'a> {
    fn from(value: &'a Value) -> Self {
        match value {
            Value::Txt(text) => Slot::Txt(text),
            Value::Str(text) => Slot::Str(text),
            Value::U64(v) => Slot::U64(*v),
            Value::I64(v) => Slot::I64(*v),
            Value::Dbl(v) => Slot::Dbl(*v),
            Value::Uuid(v) => Slot::Uuid(v),
            Value::Lazy(f) => Slot::Lazy(f),
        }
    }
}

impl Slot<'_> {
    /// `log_field_to_uint64()`, as the annotators read numbers.
    fn as_u64(&self) -> u64 {
        match *self {
            Slot::U64(v) => v,
            Slot::I64(v) => v as u64,
            Slot::Dbl(v) => v as u64,
            Slot::Txt(text) | Slot::Str(text) => {
                netdata_agent_text::parse::str2uint64(text.as_bytes()).0
            }
            Slot::Uuid(_) | Slot::Lazy(_) => 0,
        }
    }

    /// The text of a text-like field, `None` when it has none (a lazy value returned false).
    fn text<'b>(&'b self, tmp: &'b mut Vec<u8>) -> Option<&'b [u8]> {
        match *self {
            Slot::Txt(text) | Slot::Str(text) => Some(c_str(text.as_bytes())),
            Slot::Lazy(f) => {
                tmp.clear();
                f(tmp).then(|| c_str(tmp))
            }
            _ => None,
        }
    }
}

/// The fields of one record, indexed by field id.
pub(crate) struct Record<'a> {
    pub(crate) slots: [Option<Slot<'a>>; FIELDS],
}

impl<'a> Record<'a> {
    pub(crate) fn new() -> Self {
        Record {
            slots: [None; FIELDS],
        }
    }
}

/// `timestamp_usec_annotator()`: local RFC 3339 with milliseconds; `None` for 0.
fn timestamp(usec: u64) -> Option<String> {
    if usec == 0 {
        return None;
    }
    // a time localtime_r() rejects leaves C's buffer empty, which logfmt prints as ""
    let Some(tm) = netdata_agent_sys::localtime((usec / 1_000_000) as i64) else {
        return Some(String::new());
    };
    let civil = CivilTime {
        year: i64::from(tm.year),
        month: i64::from(tm.month0) + 1,
        day: i64::from(tm.mday),
        hour: i64::from(tm.hour),
        minute: i64::from(tm.min),
        second: i64::from(tm.sec),
    };
    Some(rfc3339_datetime_local(&civil, tm.gmtoff, usec, 3))
}

/// `strerror_r()`'s text for an errno, which is what `std::io::Error` prints before its ` (os error N)`.
pub fn strerror(errno: i32) -> String {
    let text = std::io::Error::from_raw_os_error(errno).to_string();
    let suffix = format!(" (os error {errno})");
    text.strip_suffix(&suffix).unwrap_or(&text).to_string()
}

fn annotate(annotator: Annotator, slot: &Slot<'_>) -> Option<String> {
    match annotator {
        Annotator::None => None,
        Annotator::Timestamp => timestamp(slot.as_u64()),
        Annotator::Priority => Some(Priority::name_of(slot.as_u64()).to_string()),
        Annotator::Errno => {
            // `print_uint64()` of the int64 value, then the text
            let errno = slot.as_u64();
            (errno != 0).then(|| format!("{errno}, {}", strerror(errno as i32)))
        }
    }
}

/// `needs_quotes_for_logfmt()`: empty, or any byte outside printable ASCII (DEL included), `"`, `\` or `=`.
fn needs_quotes(text: &[u8]) -> bool {
    text.is_empty()
        || text
            .iter()
            .any(|&b| !(0x21..=0x7f).contains(&b) || matches!(b, b'"' | b'\\' | b'='))
}

/// `string_to_logfmt()`: quoted when needed, always escaped like JSON.
fn logfmt_string(out: &mut Vec<u8>, text: &[u8]) {
    let quote = needs_quotes(text);
    if quote {
        out.push(b'"');
    }
    json_escape(out, text);
    if quote {
        out.push(b'"');
    }
}

fn key(out: &mut Vec<u8>, key: &str) {
    out.extend_from_slice(key.as_bytes());
    out.push(b'=');
}

/// `nd_logger_logfmt()`: without the trailing newline. A field that is set but prints nothing still costs its
/// separator space (C's double spaces, D27).
pub(crate) fn logfmt(record: &Record<'_>, out: &mut Vec<u8>) {
    let mut tmp = Vec::new();
    for (id, slot) in record.slots.iter().enumerate() {
        let (Some(slot), Some(name)) = (slot, FIELD_TABLE[id].logfmt) else {
            continue;
        };
        let annotator = FIELD_TABLE[id].annotator;
        if annotator != Annotator::None {
            let Some(text) = annotate(annotator, slot) else {
                continue;
            };
            if !out.is_empty() {
                out.push(b' ');
            }
            key(out, name);
            logfmt_string(out, text.as_bytes());
            continue;
        }
        if !out.is_empty() {
            out.push(b' ');
        }
        match *slot {
            Slot::Txt(text) => {
                let text = c_str(text.as_bytes());
                if !text.is_empty() {
                    key(out, name);
                    logfmt_string(out, text);
                }
            }
            Slot::Str(text) => {
                key(out, name);
                logfmt_string(out, c_str(text.as_bytes()));
            }
            Slot::U64(v) => {
                key(out, name);
                print_uint64(out, v);
            }
            Slot::I64(v) => {
                key(out, name);
                print_int64(out, v);
            }
            Slot::Dbl(v) => {
                key(out, name);
                print_netdata_double_or_null(out, v);
            }
            Slot::Uuid(uuid) => {
                if uuid.iter().any(|&b| b != 0) {
                    key(out, name);
                    print_uuid_lower_compact(out, uuid);
                }
            }
            Slot::Lazy(_) => {
                if let Some(text) = slot.text(&mut tmp) {
                    key(out, name);
                    logfmt_string(out, text);
                }
            }
        }
    }
}

/// `nd_logger_json()`: one minified object, raw numbers, no annotators; empty texts are omitted.
pub(crate) fn json(record: &Record<'_>, out: &mut Vec<u8>) {
    let mut wb = JsonWriter::new(JsonOptions::MINIFY);
    let mut tmp = Vec::new();
    for (id, slot) in record.slots.iter().enumerate() {
        let (Some(slot), Some(name)) = (slot, FIELD_TABLE[id].logfmt) else {
            continue;
        };
        match *slot {
            Slot::U64(v) => wb.member_add_uint64(name, v),
            Slot::I64(v) => wb.member_add_int64(name, v),
            Slot::Dbl(v) => wb.member_add_double(name, v),
            Slot::Uuid(uuid) => {
                if uuid.iter().any(|&b| b != 0) {
                    let mut hex = Vec::with_capacity(32);
                    print_uuid_lower_compact(&mut hex, uuid);
                    wb.member_add_string(name, &hex);
                }
            }
            Slot::Txt(_) | Slot::Str(_) | Slot::Lazy(_) => {
                if let Some(text) = slot.text(&mut tmp).filter(|t| !t.is_empty()) {
                    wb.member_add_string(name, text);
                }
            }
        }
    }
    wb.finalize();
    out.extend_from_slice(wb.as_bytes());
}

/// `nd_logger_journal_direct()`'s datagram: `KEY=value\n`, or for text with a newline `KEY\n` + the length as 8
/// little-endian bytes + the bytes + `\n`. Values are not escaped; empty texts are omitted.
pub(crate) fn journal(record: &Record<'_>, out: &mut Vec<u8>) {
    let mut tmp = Vec::new();
    for (id, slot) in record.slots.iter().enumerate() {
        let (Some(slot), Some(name)) = (slot, FIELD_TABLE[id].journal) else {
            continue;
        };
        let number = |out: &mut Vec<u8>, print: &dyn Fn(&mut Vec<u8>)| {
            key(out, name);
            print(out);
            out.push(b'\n');
        };
        match *slot {
            Slot::U64(v) => number(out, &|o| print_uint64(o, v)),
            Slot::I64(v) => number(out, &|o| print_int64(o, v)),
            Slot::Dbl(v) => number(out, &|o| print_netdata_double_or_null(o, v)),
            Slot::Uuid(uuid) => {
                if uuid.iter().any(|&b| b != 0) {
                    number(out, &|o| print_uuid_lower_compact(o, uuid));
                }
            }
            Slot::Txt(_) | Slot::Str(_) | Slot::Lazy(_) => {
                let Some(text) = slot.text(&mut tmp).filter(|t| !t.is_empty()) else {
                    continue;
                };
                out.extend_from_slice(name.as_bytes());
                if text.contains(&b'\n') {
                    out.push(b'\n');
                    out.extend_from_slice(&(text.len() as u64).to_le_bytes());
                    out.extend_from_slice(text);
                } else {
                    out.push(b'=');
                    out.extend_from_slice(text);
                }
                out.push(b'\n');
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Field;

    fn set<'a>(record: &mut Record<'a>, field: Field, slot: Slot<'a>) {
        record.slots[field as usize] = Some(slot);
    }

    /// The automatic fields of a daemon record on the main thread (`knowledge/spec-logging.md` §3.6).
    fn daemon_main_thread<'a>(message: &'a str, errno: i64) -> Record<'a> {
        let mut r = Record::new();
        set(&mut r, Field::SyslogIdentifier, Slot::Txt("netdata"));
        set(&mut r, Field::LogSource, Slot::Txt("daemon"));
        set(&mut r, Field::Priority, Slot::U64(Priority::Info as u64));
        if errno != 0 {
            set(&mut r, Field::Errno, Slot::I64(errno));
        }
        set(&mut r, Field::Tid, Slot::U64(970_822));
        set(&mut r, Field::ThreadTag, Slot::Txt(""));
        set(&mut r, Field::Message, Slot::Txt(message));
        r
    }

    fn text(f: fn(&Record<'_>, &mut Vec<u8>), r: &Record<'_>) -> String {
        let mut out = Vec::new();
        f(r, &mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn logfmt_keeps_the_separator_of_an_empty_thread_tag() {
        let r = daemon_main_thread("NETDATA STARTUP: next: signals", 2);
        assert_eq!(
            text(logfmt, &r),
            "comm=netdata source=daemon level=info errno=\"2, No such file or directory\" tid=970822  \
             msg=\"NETDATA STARTUP: next: signals\""
        );
    }

    #[test]
    fn logfmt_quotes_only_when_needed_and_escapes_like_json() {
        let cases = [
            ("/api/v1/info", "request=/api/v1/info"),
            (
                "/api/v1/data?chart=system.cpu&a\nb=1&x=\"q\"",
                "request=\"/api/v1/data?chart=system.cpu&a\\nb=1&x=\\\"q\\\"\"",
            ),
            ("tab\there", "request=\"tab\\there\""),
            ("\u{1}", "request=\"\\u0001\""),
            ("⚠️", "request=\"⚠️\""),
        ];
        for (value, expected) in cases {
            let mut r = Record::new();
            set(&mut r, Field::Request, Slot::Txt(value));
            assert_eq!(text(logfmt, &r), expected, "{value:?}");
        }
    }

    #[test]
    fn logfmt_prints_empty_strings_and_skips_zero_uuids_but_keeps_their_space() {
        let zero = [0u8; 16];
        let mut r = Record::new();
        set(&mut r, Field::LogSource, Slot::Txt("health"));
        set(&mut r, Field::NidlNode, Slot::Str(""));
        set(&mut r, Field::TransactionId, Slot::Uuid(&zero));
        set(&mut r, Field::AlertValue, Slot::Dbl(0.300_447_7));
        set(&mut r, Field::AlertValueOld, Slot::Dbl(f64::NAN));
        assert_eq!(
            text(logfmt, &r),
            "source=health node=\"\"  alert_value=0.3004477 alert_value_old=null"
        );
    }

    #[test]
    fn json_writes_raw_numbers_and_omits_empty_text() {
        let mut r = daemon_main_thread("Cannot find a status file in any location", 2);
        set(
            &mut r,
            Field::TimestampRealtimeUsec,
            Slot::U64(1_790_238_474_803_004),
        );
        set(&mut r, Field::Priority, Slot::U64(Priority::Err as u64));
        set(&mut r, Field::Tid, Slot::U64(977_466));
        set(&mut r, Field::SrcPort, Slot::Txt("59634"));
        assert_eq!(
            text(json, &r),
            "{\"time\":1790238474803004,\"comm\":\"netdata\",\"source\":\"daemon\",\"level\":3,\"errno\":2,\
             \"tid\":977466,\"src_port\":\"59634\",\"msg\":\"Cannot find a status file in any location\"}"
        );
    }

    #[test]
    fn json_keeps_the_duplicate_alert_value_old_key() {
        let mut r = Record::new();
        set(&mut r, Field::AlertValueOld, Slot::Dbl(f64::NAN));
        set(&mut r, Field::AlertStatusOld, Slot::Txt("UNINITIALIZED"));
        set(&mut r, Field::AlertSource, Slot::Txt("never written"));
        assert_eq!(
            text(json, &r),
            "{\"alert_value_old\":null,\"alert_value_old\":\"UNINITIALIZED\"}"
        );
    }

    #[test]
    fn journal_frames_multi_line_text_and_has_no_timestamp() {
        let uuid = [0xabu8; 16];
        let mut r = daemon_main_thread("line one\nline two", 0);
        set(&mut r, Field::TimestampRealtimeUsec, Slot::U64(1));
        set(&mut r, Field::InvocationId, Slot::Uuid(&uuid));
        set(&mut r, Field::Line, Slot::U64(107));
        let mut out = Vec::new();
        journal(&r, &mut out);
        let mut expected =
            b"SYSLOG_IDENTIFIER=netdata\nND_LOG_SOURCE=daemon\nPRIORITY=6\nINVOCATION_ID=\
            abababababababababababababababab\nCODE_LINE=107\nTID=970822\nMESSAGE\n"
                .to_vec();
        expected.extend_from_slice(&17u64.to_le_bytes());
        expected.extend_from_slice(b"line one\nline two\n");
        assert_eq!(out, expected);
    }

    #[test]
    fn lazy_values_print_only_when_they_have_text() {
        let yes: Lazy = std::sync::Arc::new(|out: &mut Vec<u8>| {
            out.extend_from_slice(b"V1 V2 ");
            true
        });
        let no: Lazy = std::sync::Arc::new(|_: &mut Vec<u8>| false);
        let mut r = Record::new();
        set(&mut r, Field::NidlNode, Slot::Txt("test-host"));
        set(&mut r, Field::NidlInstance, Slot::Lazy(&no));
        set(&mut r, Field::SrcCapabilities, Slot::Lazy(&yes));
        assert_eq!(
            text(logfmt, &r),
            "node=test-host  src_capabilities=\"V1 V2 \""
        );
        assert_eq!(
            text(json, &r),
            "{\"node\":\"test-host\",\"src_capabilities\":\"V1 V2 \"}"
        );
    }

    #[test]
    fn strerror_is_the_libc_text() {
        assert_eq!(strerror(2), "No such file or directory");
        assert_eq!(strerror(11), "Resource temporarily unavailable");
    }
}
