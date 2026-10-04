//! Shared by the vector tests: the reader of `netdata-agent-text` (the vectors use its field encoding).

#![allow(dead_code, unused_imports)]

#[path = "../../../text/tests/common/mod.rs"]
mod reader;

pub use reader::{Row, check, rows};

/// A string field: C's NULL is written as a lone NUL byte.
pub fn nullable(row: &Row, i: usize) -> Option<&[u8]> {
    (row.bytes(i) != [0]).then(|| row.bytes(i))
}

/// An `f32` from its bits in a field.
pub fn float(row: &Row, i: usize) -> f32 {
    f32::from_bits(u32::from_str_radix(row.str(i), 16).expect("hex bits"))
}

// ------------------------------------------------------------------------------------------------
// shared by the corpus tests: the items of rules.tsv, C's records, a rule as its row

use netdata_agent_eval::Expression;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::json::prototype_to_json;
use netdata_agent_health::prototype::Rule;
use netdata_agent_log::Captured;

/// Fields as the vectors hold them, shown as text in a failure's message. Comparisons are made on the bytes.
pub fn show(fields: &[Vec<u8>]) -> Vec<String> {
    fields.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect()
}

/// C's NULL string in a vector.
pub fn text(value: &Option<Vec<u8>>) -> Vec<u8> {
    value.clone().unwrap_or_else(|| vec![0])
}

pub fn number(n: impl ToString) -> Vec<u8> {
    n.to_string().into_bytes()
}

pub fn hex(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect()
}

/// An expression's source and parsed_as, NULL for both when absent.
pub fn expression(e: &Option<Expression>) -> [Vec<u8>; 2] {
    match e {
        Some(e) => [e.source().to_vec(), e.parsed_as().to_vec()],
        None => [vec![0], vec![0]],
    }
}

/// A rule as its `rule` row, from the name on: what C's dumper writes when the rule is stored.
pub fn rule_fields(rule: &Rule) -> Vec<Vec<u8>> {
    let (am, ac) = (&rule.r#match, &rule.config);
    let mut fields = vec![
        text(&ac.name),
        number(u8::from(am.is_template)),
        number(u8::from(am.enabled)),
        text(&am.on),
        text(&am.host_labels),
        text(&am.chart_labels),
        text(&ac.exec),
        text(&ac.recipient),
        text(&ac.classification),
        text(&ac.component),
        text(&ac.r#type),
        number(ac.source_type as u8),
        text(&ac.source),
        text(&ac.units),
        text(&ac.summary),
        text(&ac.info),
        number(ac.update_every),
        number(ac.alert_action_options),
        text(&ac.dimensions),
        ac.time_group_name().as_bytes().to_vec(),
        number(ac.time_group_condition as u8),
        format!("{:016x}", ac.time_group_value.to_bits()).into_bytes(),
        number(ac.dims_group as u8),
        number(ac.data_source as u8),
        number(ac.before),
        number(ac.after),
        format!("{:08x}", ac.options as u32).into_bytes(),
    ];
    fields.extend(expression(&ac.calculation));
    fields.extend(expression(&ac.warning));
    fields.extend(expression(&ac.critical));
    fields.extend([
        number(ac.delay_up_duration),
        number(ac.delay_down_duration),
        number(ac.delay_max_duration),
        format!("{:08x}", ac.delay_multiplier.to_bits()).into_bytes(),
        number(u8::from(ac.has_custom_repeat_config)),
        number(ac.warn_repeat_every),
        number(ac.crit_repeat_every),
        hex(&ac.hash_id),
        prototype_to_json(ac.name.as_deref().unwrap_or(b""), &[rule], true),
    ]);
    fields
}

/// What C's logfmt printed of a message, back as the message's bytes: its escapes are JSON's.
pub fn unescape_logfmt(printed: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(printed.len());
    let mut bytes = printed.iter().copied();
    while let Some(c) = bytes.next() {
        if c != b'\\' {
            out.push(c);
            continue;
        }
        match bytes.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(b'r') => out.push(b'\r'),
            Some(b'b') => out.push(8),
            Some(b'f') => out.push(12),
            Some(b'u') => {
                let code: Vec<u8> = bytes.by_ref().take(4).collect();
                let code = u32::from_str_radix(std::str::from_utf8(&code).expect("hex"), 16).expect("hex");
                let c = char::from_u32(code).expect("a character");
                out.extend_from_slice(c.encode_utf8(&mut [0; 4]).as_bytes());
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// One of C's records: level, errno number (0 for none), the message's bytes.
pub type CRecord = (String, i32, Vec<u8>);

/// A `records.tsv`-shaped triple of fields (level, errno, message as printed) as a record.
pub fn c_record(level: &str, errno: &str, printed: &[u8]) -> CRecord {
    let errno = match errno {
        "-" => 0,
        text => text.split(',').next().and_then(|n| n.parse().ok()).expect("an errno number"),
    };
    (level.to_owned(), errno, unescape_logfmt(printed))
}

/// Whether the captured records are C's. A record of Rust is text, so where C's message is not UTF-8 (a byte of a
/// health file written as it is) Rust's holds U+FFFD for each such byte: those compare after the same replacement,
/// and are counted.
pub fn same_records(expected: &[CRecord], actual: &[Captured], not_utf8: &mut usize) -> bool {
    use netdata_agent_log::Priority;
    expected.len() == actual.len()
        && expected.iter().zip(actual).all(|((level, errno, message), record)| {
            // the first name of each priority in C's `nd_log_priorities[]`
            let actual_level = match record.priority {
                Priority::Emerg => "emergency",
                Priority::Alert => "alert",
                Priority::Crit => "critical",
                Priority::Err => "error",
                Priority::Warning => "warning",
                Priority::Notice => "notice",
                Priority::Info => "info",
                Priority::Debug => "debug",
            };
            let actual_message = record.message.as_deref().unwrap_or("");
            let same_message = match std::str::from_utf8(message) {
                Ok(message) => message == actual_message,
                Err(_) => {
                    *not_utf8 += 1;
                    String::from_utf8_lossy(message) == actual_message
                }
            };
            level == actual_level && *errno == record.errno && same_message
        })
}

/// The records as text, for a failure's message.
pub fn show_records(expected: &[CRecord], actual: &[Captured]) -> String {
    let first = expected
        .iter()
        .zip(actual)
        .position(|(e, a)| !same_records(std::slice::from_ref(e), std::slice::from_ref(a), &mut 0))
        .unwrap_or(expected.len().min(actual.len()));
    format!(
        "{} against {}, first at record {first}:\n  C    {:?}\n  Rust {:?}",
        expected.len(),
        actual.len(),
        expected.get(first).map(|(level, errno, message)| (level, errno, String::from_utf8_lossy(message))),
        actual.get(first).map(|record| (record.priority, record.errno, record.message.clone()))
    )
}

/// One corpus item of `rules.tsv`: its rows by kind.
pub struct Item {
    pub name: Vec<u8>,
    pub rules: Vec<Row>,
    pub files: Vec<Row>,
    pub entries: Vec<Row>,
}

pub fn items() -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    for row in rows("rules.tsv") {
        if items.last().is_none_or(|item| item.name != row.bytes(0)) {
            items.push(Item { name: row.bytes(0).to_vec(), rules: Vec::new(), files: Vec::new(), entries: Vec::new() });
        }
        let item = items.last_mut().expect("just pushed");
        match row.str(1) {
            "rule" => item.rules.push(row),
            "file" => item.files.push(row),
            "entry" => item.entries.push(row),
            other => panic!("rules.tsv:{}: kind {other}", row.line),
        }
    }
    items
}

/// The oracle's `[health]` values (`gen-health-vectors.c` `main()`).
pub fn oracle_config() -> HealthConfig {
    HealthConfig {
        default_exec: b"/oracle/plugins.d/alarm-notify.sh".to_vec(),
        default_recipient: b"root".to_vec(),
        ..HealthConfig::default()
    }
}
