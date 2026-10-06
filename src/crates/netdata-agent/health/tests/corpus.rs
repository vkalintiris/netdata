//! The configuration corpus: every item of `tests/vectors/rules.tsv` and `records.tsv` gives what C's reader,
//! prototype store and hash gave (`tests/oracle/gen-health-vectors.c`).

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use common::{CRecord, Item, c_record, hex, items, number, oracle_config, rows, rule_fields, same_records, show, show_records, text};
use netdata_agent_health::Health;
use netdata_agent_health::readfile::health_readfile;
use netdata_agent_log::Captured;

type Fields = Vec<Vec<u8>>;

/// Reads an item's files as C did; the rules as they were stored, the returns, the store afterwards, the records.
fn read(item: &Item) -> (Vec<Fields>, Vec<bool>, Vec<Fields>, Vec<Captured>) {
    let stored: Arc<Mutex<Vec<Fields>>> = Arc::default();
    let sink = Arc::clone(&stored);
    let health = Health::init(oracle_config(), Box::new(move |rule| sink.lock().unwrap().push(rule_fields(rule))));

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

/// The first place two lists of rows differ, as text.
fn first_difference(expected: &[Fields], actual: &[Fields]) -> String {
    let first = expected.iter().zip(actual).position(|(e, a)| e != a).unwrap_or(expected.len().min(actual.len()));
    format!(
        "{} against {}, first at {first}:\n  C    {:?}\n  Rust {:?}",
        expected.len(),
        actual.len(),
        expected.get(first).map(|fields| show(fields)),
        actual.get(first).map(|fields| show(fields))
    )
}

#[test]
fn rules_store_and_records_match_c() {
    let mut expected_records: HashMap<Vec<u8>, Vec<CRecord>> = HashMap::new();
    let mut record_rows = 0;
    for row in rows("records.tsv") {
        expected_records.entry(row.bytes(0).to_vec()).or_default().push(c_record(row.str(1), row.str(2), row.bytes(3)));
        record_rows += 1;
    }
    assert_eq!(record_rows, RECORDS);

    let items = items();
    assert_eq!(items.len(), ITEMS);
    let (mut rules, mut entries, mut not_utf8, mut failures) = (0, 0, 0, Vec::new());
    for item in &items {
        let at = String::from_utf8_lossy(&item.name).into_owned();
        let (stored, returns, store, records) = read(item);
        rules += item.rules.len();
        entries += item.entries.len();

        // bytes against bytes: a rule's fields are the file's bytes
        let expected: Vec<Fields> = item.rules.iter().map(|row| row.fields[2..].to_vec()).collect();
        if expected != stored {
            failures.push(format!("{at}: the stored rules differ: {}", first_difference(&expected, &stored)));
        }

        let expected: Vec<bool> = item.files.iter().map(|row| row.flag(4)).collect();
        if expected != returns {
            failures.push(format!("{at}: the files' returns differ: expected {expected:?}, actual {returns:?}"));
        }

        let expected: Vec<Fields> = item.entries.iter().map(|row| row.fields[2..].to_vec()).collect();
        if expected != store {
            failures.push(format!("{at}: the store differs: {}", first_difference(&expected, &store)));
        }

        let expected = expected_records.remove(&item.name).unwrap_or_default();
        if !same_records(&expected, &records, &mut not_utf8) {
            failures.push(format!("{at}: the records differ: {}", show_records(&expected, &records)));
        }
    }

    assert!(expected_records.is_empty(), "records of items that are not in rules.tsv: {:?}", expected_records.keys());
    assert!(failures.is_empty(), "{} of {} items differ:\n{}", failures.len(), items.len(), failures[..failures.len().min(12)].join("\n"));
    assert_eq!((rules, entries), (RULES, ENTRIES));
    // records whose message holds bytes that are not UTF-8: Rust writes U+FFFD for each (D195 F5)
    assert_eq!(not_utf8, NOT_UTF8);
}

/// The size of the vectors: a guard against a comparison that compared nothing.
const ITEMS: usize = 250;
const RECORDS: usize = 815;
const RULES: usize = 2719;
const ENTRIES: usize = 2714;
const NOT_UTF8: usize = 2;
