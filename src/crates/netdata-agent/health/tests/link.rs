//! Matching rules against hosts and charts, and the copy an alert takes of its rule, against C
//! (`tests/oracle/gen-link-vectors.c`: `labels.tsv`, `link.tsv`; `gen-health-vectors.c`: `copy.tsv`).

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use common::{CRecord, c_record, expression, hex, items, number, oracle_config, rows, same_records, show, show_records};
use netdata_agent_health::Health;
use netdata_agent_health::alert::copy_expressions;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::matching::{ChartKey, label_patterns, rules_for_chart};
use netdata_agent_health::readfile::health_readfile;
use netdata_agent_rrd::labels::{Labels, SRC_CONFIG};
use netdata_agent_text::c::take_errno;

/// A label set's names and values, in name order: a `set` row's fields.
fn stored(labels: &Labels) -> Vec<Vec<u8>> {
    let mut pairs: Vec<(&[u8], &[u8])> = labels.iter().map(|label| (&label.name[..], &label.value[..])).collect();
    pairs.sort();
    pairs.into_iter().flat_map(|(name, value)| [name.to_vec(), value.to_vec()]).collect()
}

fn split_once(text: &[u8], at: u8) -> (&[u8], &[u8]) {
    match text.iter().position(|&c| c == at) {
        Some(i) => (&text[..i], &text[i + 1..]),
        None => (text, &[]),
    }
}

/// C's verdict on each pattern text against each label set. The sets are rebuilt from what C holds, which the
/// sanitizer leaves as it is.
#[test]
fn labels_match_c() {
    let mut sets: HashMap<String, Labels> = HashMap::new();
    let (mut matches, mut failures) = (0, Vec::new());
    for row in rows("labels.tsv") {
        match row.str(0) {
            "set" => {
                let mut labels = Labels::default();
                for pair in row.fields[2..].chunks(2) {
                    labels.add(&pair[0], &pair[1], SRC_CONFIG);
                }
                assert_eq!(show(&stored(&labels)), show(&row.fields[2..]), "labels.tsv:{}", row.line);
                sets.insert(row.str(1).to_owned(), labels);
            }
            "match" => {
                let labels = &sets[row.str(1)];
                let matched = label_patterns(row.bytes(2)).is_none_or(|array| array.label_match(labels, b'='));
                if matched != row.flag(3) {
                    failures.push(format!(
                        "labels.tsv:{}: {:?} on {}: C says {}",
                        row.line,
                        String::from_utf8_lossy(row.bytes(2)),
                        row.str(1),
                        row.flag(3)
                    ));
                }
                matches += 1;
            }
            other => panic!("labels.tsv:{}: kind {other}", row.line),
        }
    }
    assert!(failures.is_empty(), "{} of {matches} differ:\n{}", failures.len(), failures[..failures.len().min(20)].join("\n"));
    assert_eq!(matches, 1014);
}

/// The scenario file played as the C generator plays it: for every chart, the rules C would link, in C's order.
#[test]
fn links_match_c() {
    let c_sets: HashMap<Vec<u8>, Vec<Vec<u8>>> = rows("labels.tsv")
        .into_iter()
        .filter(|row| row.str(0) == "set")
        .map(|row| (row.bytes(1).to_vec(), row.fields[2..].to_vec()))
        .collect();
    let mut expected = rows("link.tsv").into_iter().peekable();

    let scenarios = std::fs::read("tests/corpus/match/scenarios.txt").expect("the scenario file");
    let mut sets: HashMap<Vec<u8>, Labels> = HashMap::new();
    let mut health: Option<Arc<Health>> = None;
    let mut enabled = HealthConfig::enabled_alerts_pattern(b"*");
    let mut scenario: Vec<u8> = Vec::new();
    let mut host: Option<(Vec<u8>, Option<Labels>)> = None;
    let (mut charts, mut links, mut failures) = (0, 0, Vec::new());

    let set_of = |sets: &HashMap<Vec<u8>, Labels>, name: &[u8]| -> Option<Labels> {
        (name != b"nolabels").then(|| sets.get(name).unwrap_or_else(|| panic!("label set {}", String::from_utf8_lossy(name))).clone())
    };

    for line in scenarios.split(|&c| c == b'\n') {
        if line.is_empty() || line[0] == b'#' {
            continue;
        }
        let (directive, rest) = split_once(line, b' ');
        match directive {
            b"labelset" => {
                let (name, text) = split_once(rest, b' ');
                let mut labels = Labels::default();
                if text != b"-" {
                    for label in text.split(|&c| c == b'|') {
                        let (label_name, value) = split_once(label, b'=');
                        labels.add(label_name, value, SRC_CONFIG);
                    }
                }
                // the labels as a collector's would be: sanitized as C sanitizes them
                assert_eq!(show(&stored(&labels)), show(&c_sets[name]), "label set {}", String::from_utf8_lossy(name));
                sets.insert(name.to_vec(), labels);
            }
            b"pattern" => {}
            b"scenario" => {
                scenario = rest.to_vec();
                health = Some(Health::init(oracle_config(), Box::new(|_| {}), false));
                enabled = HealthConfig::enabled_alerts_pattern(b"*");
                host = None;
            }
            b"enabled" => enabled = HealthConfig::enabled_alerts_pattern(rest),
            b"load" => {
                let health = health.as_ref().expect("a scenario");
                assert!(health_readfile(health, &rest[2..], rest[0] == b'1'), "{}", String::from_utf8_lossy(rest));
            }
            b"host" => {
                let (name, set) = split_once(rest, b' ');
                host = Some((name.to_vec(), set_of(&sets, set)));
            }
            b"chart" => {
                let tokens: Vec<&[u8]> = rest.split(|&c| c == b' ').collect();
                let [id, name, context, set] = tokens[..] else {
                    panic!("a chart line: {}", String::from_utf8_lossy(line));
                };
                let (host_name, host_labels) = host.as_ref().expect("a host");
                let chart_labels = set_of(&sets, set);
                let chart = ChartKey { id, name, context, labels: chart_labels.as_ref() };
                let prototypes = health.as_ref().expect("a scenario").prototypes();
                let actual: Vec<Vec<String>> = rules_for_chart(&prototypes, &enabled, host_labels.as_ref(), &chart)
                    .into_iter()
                    .map(|(index, rule)| {
                        show(&[
                            rule.config.name.clone().unwrap_or_default(),
                            number(index),
                            number(u8::from(rule.r#match.is_template)),
                            hex(&rule.config.hash_id),
                        ])
                    })
                    .collect();

                // the chart's rows: its links, then its end row
                let at = [&scenario[..], &host_name[..], id];
                let mut c_links: Vec<Vec<String>> = Vec::new();
                loop {
                    let row = expected.next().expect("link.tsv ends before the scenario file");
                    assert_eq!(show(&row.fields[..3]), show(&at.map(<[u8]>::to_vec)), "link.tsv:{}", row.line);
                    match row.str(3) {
                        "link" => c_links.push(show(&row.fields[4..])),
                        "end" => {
                            assert_eq!(row.num::<usize>(4), c_links.len(), "link.tsv:{}", row.line);
                            break;
                        }
                        other => panic!("link.tsv:{}: kind {other}", row.line),
                    }
                }
                if actual != c_links {
                    failures.push(format!(
                        "{:?} with {}:\n  C    {c_links:?}\n  Rust {actual:?}",
                        show(&at.map(<[u8]>::to_vec)),
                        String::from_utf8_lossy(set)
                    ));
                }
                charts += 1;
                links += c_links.len();
            }
            other => panic!("directive {}", String::from_utf8_lossy(other)),
        }
    }

    assert!(expected.next().is_none(), "link.tsv holds more charts than the scenario file");
    assert!(failures.is_empty(), "{} of {charts} charts differ:\n{}", failures.len(), failures[..failures.len().min(10)].join("\n"));
    assert_eq!((charts, links), (5020, 8196));
}

/// Every stored rule of every corpus item, copied as an alert copies it: the three expressions parsed again, and
/// what that logs.
#[test]
fn copies_match_c() {
    type Fields = Vec<Vec<u8>>;
    let mut expected: HashMap<Vec<u8>, Vec<(Fields, Vec<CRecord>)>> = HashMap::new();
    let (mut copy_rows, mut record_rows) = (0, 0);
    for row in rows("copy.tsv") {
        let item = expected.entry(row.bytes(0).to_vec()).or_default();
        match row.str(1) {
            "copy" => {
                item.push((row.fields[2..].to_vec(), Vec::new()));
                copy_rows += 1;
            }
            "record" => {
                let copy = item.last_mut().unwrap_or_else(|| panic!("copy.tsv:{}: a record before a copy", row.line));
                copy.1.push(c_record(row.str(2), row.str(3), row.bytes(4)));
                record_rows += 1;
            }
            other => panic!("copy.tsv:{}: kind {other}", row.line),
        }
    }
    assert_eq!((copy_rows, record_rows), (COPIES, COPY_RECORDS));

    let (mut not_utf8, mut failures) = (0, Vec::new());
    for item in items() {
        let at = String::from_utf8_lossy(&item.name).into_owned();
        let health = Health::init(oracle_config(), Box::new(|_| {}), false);
        netdata_agent_log::capture(|| {
            for file in &item.files {
                health_readfile(&health, file.bytes(2), file.flag(3));
            }
        });

        let expected = expected.remove(&item.name).unwrap_or_default();
        let mut copies = expected.iter();
        let mut stored = 0;
        for (name, prototype) in health.prototypes().iter() {
            for (index, rule) in prototype.rules().iter().enumerate() {
                stored += 1;
                // C's generator clears errno before each copy
                take_errno();
                let (expressions, records) = netdata_agent_log::capture(|| copy_expressions(&rule.config));
                let mut fields = vec![name.to_vec(), number(index)];
                for copied in &expressions {
                    fields.extend(expression(copied));
                }
                let Some((c_fields, c_records)) = copies.next() else {
                    continue;
                };
                if *c_fields != fields {
                    failures.push(format!("{at}: the copy differs:\n  C    {:?}\n  Rust {:?}", show(c_fields), show(&fields)));
                } else if !same_records(c_records, &records, &mut not_utf8) {
                    failures.push(format!("{at}: the records of {:?} differ: {}", show(&fields[..2]), show_records(c_records, &records)));
                }
            }
        }
        if stored != expected.len() {
            failures.push(format!("{at}: {stored} rules stored, C copied {}", expected.len()));
        }
    }
    assert!(expected.is_empty(), "copies of items that are not in rules.tsv: {:?}", expected.keys());
    assert!(failures.is_empty(), "{} differ:\n{}", failures.len(), failures[..failures.len().min(10)].join("\n"));
    assert_eq!(not_utf8, 0);
}

/// The size of `copy.tsv`: a guard against a comparison that compared nothing.
const COPIES: usize = 2667;
const COPY_RECORDS: usize = 42;
