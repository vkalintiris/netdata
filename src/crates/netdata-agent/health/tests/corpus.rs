//! The configuration corpus: every item of `tests/vectors/rules.tsv` and `records.tsv` gives what C's reader,
//! prototype store and hash gave (`tests/oracle/gen-health-vectors.c`).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::{Row, rows};
use netdata_agent_eval::Expression;
use netdata_agent_health::Health;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::json::prototype_to_json;
use netdata_agent_health::prototype::Rule;
use netdata_agent_health::readfile::health_readfile;
use netdata_agent_log::Captured;

/// A field as the vectors hold it, shown as text.
fn show(fields: &[Vec<u8>]) -> Vec<String> {
    fields.iter().map(|field| String::from_utf8_lossy(field).into_owned()).collect()
}

/// C's NULL string in a vector.
fn text(value: &Option<Vec<u8>>) -> Vec<u8> {
    value.clone().unwrap_or_else(|| vec![0])
}

fn number(n: impl ToString) -> Vec<u8> {
    n.to_string().into_bytes()
}

/// An expression's source and parsed_as, NULL for both when absent.
fn expression(e: &Option<Expression>) -> [Vec<u8>; 2] {
    match e {
        Some(e) => [e.source().to_vec(), e.parsed_as().to_vec()],
        None => [vec![0], vec![0]],
    }
}

/// A rule as its `rule` row, from the name on: what C's dumper writes when the rule is stored.
fn rule_fields(rule: &Rule) -> Vec<Vec<u8>> {
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

fn hex(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().flat_map(|b| format!("{b:02x}").into_bytes()).collect()
}

/// What C's logfmt printed of a message, back as the message: its escapes are JSON's.
fn unescape_logfmt(printed: &[u8]) -> String {
    let printed = String::from_utf8_lossy(printed);
    let mut out = String::new();
    let mut chars = printed.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('b') => out.push('\u{8}'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let code: String = chars.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&code, 16).expect("hex")).expect("a character"));
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// A captured record as its row: level, errno number (0 for none), message.
fn record_fields(record: &Captured) -> (String, i32, String) {
    use netdata_agent_log::Priority;
    // the first name of each priority in C's `nd_log_priorities[]`
    let level = match record.priority {
        Priority::Emerg => "emergency",
        Priority::Alert => "alert",
        Priority::Crit => "critical",
        Priority::Err => "error",
        Priority::Warning => "warning",
        Priority::Notice => "notice",
        Priority::Info => "info",
        Priority::Debug => "debug",
    };
    (level.to_owned(), record.errno, record.message.clone().unwrap_or_default())
}

struct Item {
    name: Vec<u8>,
    rules: Vec<Row>,
    files: Vec<Row>,
    entries: Vec<Row>,
}

fn items() -> Vec<Item> {
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
fn config() -> HealthConfig {
    HealthConfig {
        default_exec: b"/oracle/plugins.d/alarm-notify.sh".to_vec(),
        default_recipient: b"root".to_vec(),
        ..HealthConfig::default()
    }
}

/// Reads an item's files as C did; the rules as they were stored, the returns, the store afterwards, the records.
#[allow(clippy::type_complexity)]
fn read(item: &Item) -> (Vec<Vec<Vec<u8>>>, Vec<bool>, Vec<Vec<Vec<u8>>>, Vec<Captured>) {
    let stored: Arc<Mutex<Vec<Vec<Vec<u8>>>>> = Arc::default();
    let sink = Arc::clone(&stored);
    let health = Health::init(config(), Box::new(move |rule| sink.lock().unwrap().push(rule_fields(rule))));

    let (returns, records) = netdata_agent_log::capture(|| {
        item.files.iter().map(|file| health_readfile(&health, file.bytes(2), file.flag(3))).collect::<Vec<bool>>()
    });

    let mut entries = Vec::new();
    for (name, prototype) in health.prototypes().iter() {
        for (index, rule) in prototype.rules().iter().enumerate() {
            entries.push(vec![
                name.to_vec(),
                number(index),
                number(u8::from(prototype.enabled())),
                number(u8::from(rule.r#match.enabled)),
                text(&rule.config.exec),
                text(&rule.config.recipient),
                hex(&rule.config.hash_id),
            ]);
        }
    }
    let stored = std::mem::take(&mut *stored.lock().unwrap());
    (stored, returns, entries, records)
}

#[test]
fn rules_store_and_records_match_c() {
    let mut expected_records: HashMap<Vec<u8>, Vec<(String, i32, String)>> = HashMap::new();
    let mut record_rows = 0;
    for row in rows("records.tsv") {
        let errno = match row.str(2) {
            "-" => 0,
            text => text.split(',').next().and_then(|n| n.parse().ok()).expect("an errno number"),
        };
        expected_records.entry(row.bytes(0).to_vec()).or_default().push((row.str(1).to_owned(), errno, unescape_logfmt(row.bytes(3))));
        record_rows += 1;
    }
    assert_eq!(record_rows, 741);

    let items = items();
    assert_eq!(items.len(), 203);
    let (mut rules, mut entries, mut failures) = (0, 0, Vec::new());
    for item in &items {
        let at = String::from_utf8_lossy(&item.name).into_owned();
        let (stored, returns, store, records) = read(item);
        rules += item.rules.len();
        entries += item.entries.len();

        let expected: Vec<Vec<String>> = item.rules.iter().map(|row| show(&row.fields[2..])).collect();
        let actual: Vec<Vec<String>> = stored.iter().map(|fields| show(fields)).collect();
        if expected != actual {
            let first = expected.iter().zip(&actual).position(|(e, a)| e != a).unwrap_or(expected.len().min(actual.len()));
            failures.push(format!(
                "{at}: the stored rules differ ({} against {}), first at rule {first}:\n  expected {:?}\n  actual   {:?}",
                expected.len(),
                actual.len(),
                expected.get(first),
                actual.get(first)
            ));
        }

        let expected: Vec<bool> = item.files.iter().map(|row| row.flag(4)).collect();
        if expected != returns {
            failures.push(format!("{at}: the files' returns differ: expected {expected:?}, actual {returns:?}"));
        }

        let expected: Vec<Vec<String>> = item.entries.iter().map(|row| show(&row.fields[2..])).collect();
        let actual: Vec<Vec<String>> = store.iter().map(|fields| show(fields)).collect();
        if expected != actual {
            failures.push(format!("{at}: the store differs:\n  expected {expected:?}\n  actual   {actual:?}"));
        }

        let expected = expected_records.remove(&item.name).unwrap_or_default();
        let actual: Vec<(String, i32, String)> = records.iter().map(record_fields).collect();
        if expected != actual {
            let first = expected.iter().zip(&actual).position(|(e, a)| e != a).unwrap_or(expected.len().min(actual.len()));
            failures.push(format!(
                "{at}: the records differ ({} against {}), first at record {first}:\n  expected {:?}\n  actual   {:?}",
                expected.len(),
                actual.len(),
                expected.get(first),
                actual.get(first)
            ));
        }
    }

    assert!(expected_records.is_empty(), "records of items that are not in rules.tsv: {:?}", expected_records.keys());
    assert!(failures.is_empty(), "{} of {} items differ:\n{}", failures.len(), items.len(), failures[..failures.len().min(12)].join("\n"));
    assert_eq!((rules, entries), (2512, 2507));
}
