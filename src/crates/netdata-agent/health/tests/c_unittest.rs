//! C's own unit tables (`health-config-unittest.c`), dumped while C's test ran and passed
//! (`tests/oracle/health-unittest-dump.inc`): every call it made to a ported function, with what C returned.

mod common;

use std::os::unix::ffi::OsStrExt;

use common::{Row, float, nullable, rows};
use netdata_agent_eval::Expression;
use netdata_agent_health::Health;
use netdata_agent_health::config::HealthConfig;
use netdata_agent_health::keywords::{delay_apply_multiplier, parse_db_lookup, parse_delay};
use netdata_agent_health::prototype::{AlertConfig, Rule};
use netdata_agent_health::readfile::health_readfile;

fn kind(kind: &str) -> Vec<Row> {
    rows("c_unittest.tsv").into_iter().filter(|row| row.str(1) == kind).collect()
}

/// C's test parses into a zeroed config whose value is NaN (`run_db_lookup_test()`).
fn lookup(input: &[u8]) -> (bool, AlertConfig) {
    let mut ac = AlertConfig { time_group_value: f64::NAN, ..AlertConfig::default() };
    let ok = parse_db_lookup(1, b"unittest", input, &mut ac);
    (ok, ac)
}

#[test]
fn lookups_equal_c_outputs() {
    let rows = kind("lookup");
    assert_eq!(rows.len(), 158);
    for row in rows {
        let at = format!("c_unittest.tsv:{}: {}", row.line, row.str(0));
        let (ok, ac) = lookup(row.bytes(0));
        assert_eq!(i32::from(ok), row.num::<i32>(2), "{at}: return value");
        assert_eq!(ac.time_group_name(), row.str(3), "{at}: time_group");
        assert_eq!(ac.time_group_condition as i32, row.num::<i32>(4), "{at}: condition");
        assert_eq!(ac.time_group_value.to_bits(), row.bits(5), "{at}: value");
        assert_eq!(
            (ac.after, ac.before, ac.update_every),
            (row.num(6), row.num(7), row.num(8)),
            "{at}: after, before, update_every"
        );
        assert_eq!(ac.options, u64::from_str_radix(row.str(9), 16).unwrap(), "{at}: options");
        assert_eq!(ac.dimensions.as_deref(), nullable(&row, 10), "{at}: dimensions");
    }
}

/// The table's hand-written expectations, checked the way `run_db_lookup_test()` checks them: the outcome always,
/// the rest only on a success.
#[test]
fn lookups_pass_as_c_asserts() {
    let rows = kind("expect");
    assert_eq!(rows.len(), 149);
    for row in rows {
        let at = format!("c_unittest.tsv:{}: {}", row.line, row.str(0));
        let (ok, ac) = lookup(row.bytes(0));
        assert_eq!(ok, row.flag(2), "{at}: outcome");
        if !ok {
            continue;
        }
        assert_eq!(ac.time_group_name(), row.str(3), "{at}: time_group");
        assert_eq!((ac.after, ac.before), (row.num(6), row.num(7)), "{at}: after, before");

        // the condition only where the table names one other than `=`, the value only where it gives one
        let condition = row.num::<i32>(4);
        if condition != 0 {
            assert_eq!(ac.time_group_condition as i32, condition, "{at}: condition");
        }
        let expected = f64::from_bits(row.bits(5));
        if !expected.is_nan() {
            let actual = if ac.time_group_value.is_nan() { 0.0 } else { ac.time_group_value };
            assert!((actual - expected).abs() <= 0.0001, "{at}: value {actual}, expected {expected}");
        }
    }
}

#[test]
fn delays_equal_c_outputs() {
    let rows = kind("delay");
    assert_eq!(rows.len(), 21);
    for row in rows {
        let at = format!("c_unittest.tsv:{}: {}", row.line, row.str(0));
        let (mut up, mut down, mut max, mut multiplier) = (row.num(2), row.num(3), row.num(4), float(&row, 5));
        assert_eq!(row.str(6), "|");
        let ok = parse_delay(1, b"unittest", row.bytes(0), &mut up, &mut down, &mut max, &mut multiplier);
        assert_eq!(
            (i32::from(ok), up, down, max, multiplier.to_bits()),
            (row.num(7), row.num(8), row.num(9), row.num(10), float(&row, 11).to_bits()),
            "{at}"
        );
    }
}

#[test]
fn multipliers_equal_c_outputs() {
    let rows = kind("multiplier");
    assert_eq!(rows.len(), 14);
    for row in rows {
        let result = delay_apply_multiplier(row.num(0), float(&row, 2), row.num(3));
        assert_eq!(result, row.num::<i32>(4), "c_unittest.tsv:{}", row.line);
    }
}

/// An expression of a hand-built prototype, parsed as C's test parses it.
fn expression(row: &Row, i: usize) -> Option<Expression> {
    nullable(row, i).map(|source| Expression::parse(source).expect("an expression C's test parsed"))
}

/// The prototype's first rule, from the row: what C's tests fill in (`unittest_prototype_fill_valid_rule()` and
/// the two tests before it).
fn first_rule(row: &Row) -> Rule {
    let mut rule = Rule::default();
    rule.r#match.is_template = row.flag(3);
    rule.r#match.on = nullable(row, 4).map(<[u8]>::to_vec);
    rule.config.name = Some(row.bytes(0).to_vec());
    rule.config.source = Some(b"unittest".to_vec());
    rule.config.update_every = row.num(5);
    rule.config.after = row.num(6);
    rule.config.calculation = expression(row, 7);
    rule.config.warning = expression(row, 8);
    rule.config.critical = expression(row, 9);
    rule.config.delay_multiplier = float(row, 10);
    rule
}

/// `health_prototype_add()` on C's hand-built prototypes: all ten are refused, each for C's reason. The row holds
/// the first rule; where C's test chains a second one, it is the first again with the defect the test is about
/// (`test_prototype_rejects_non_finite_delay_multiplier()`, `test_prototype_rejects_missing_on()`).
#[test]
fn adds_equal_c_outputs() {
    let rows = kind("add");
    assert_eq!(rows.len(), 10);
    let health = Health::init(HealthConfig::default(), Box::new(|rule| panic!("a refused rule was stored: {rule:?}")), false);
    for row in rows {
        let at = format!("c_unittest.tsv:{}", row.line);
        let message = row.str(13);
        let mut rules = vec![first_rule(&row)];
        match row.num::<usize>(2) {
            1 => {}
            2 => {
                let mut second = first_rule(&row);
                match message {
                    "non-finite delay multiplier" => second.config.delay_multiplier = f32::NAN,
                    reason if reason.starts_with("missing match 'on' parameter") => second.r#match.on = None,
                    other => panic!("{at}: a second rule for '{other}'"),
                }
                rules.push(second);
            }
            n => panic!("{at}: {n} rules"),
        }
        assert_eq!(row.str(11), "|", "{at}");
        assert!(!row.flag(12), "{at}: C's test only adds prototypes that are refused");
        assert_eq!(health.add(rules), Err(message), "{at}");
    }
    assert_eq!(health.prototypes().len(), 0);
}

/// `test_file_invalid_same_name_rule_is_skipped()`: the file C's test wrote, read from a file of this test's.
#[test]
fn readfile_equals_c_outputs() {
    let rows = kind("readfile");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let dir = tempfile::tempdir().expect("a temporary directory");
    let path = dir.path().join("unittest.conf");
    std::fs::write(&path, row.bytes(0)).expect("the file");

    let health = Health::init(HealthConfig::default(), Box::new(|_| {}), false);
    let read = health_readfile(&health, path.as_os_str().as_bytes(), false);
    assert_eq!(read, row.flag(2));
    let store: Vec<String> = health
        .prototypes()
        .iter()
        .map(|(name, prototype)| format!("{}={}", String::from_utf8_lossy(name), prototype.rules().len()))
        .collect();
    assert_eq!(store.join("\u{1f}"), row.str(3));
}
