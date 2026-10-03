//! C's own unit vectors (`eval-unittest.c`, `eval-unittest-hardcoding.c`), dumped by the C compiler while C's
//! tests ran and passed (`tests/oracle/eval-unittest-dump.inc`): C's hand-written expectations, asserted the way C
//! asserts them, and beside them what C actually returned, asserted byte for byte.

mod common;

use std::collections::VecDeque;

use common::{Bytes, Row, bytes, double, evaluation_fields, fields, parse_fields, rows};
use netdata_agent_eval::{ERROR_OK, Expression, NoVariables, Resolver};

/// `test_groups[]`: the groups in run order, with the number of cases the production build compiles in each.
const GROUPS: [(&str, usize); 17] = [
    ("Arithmetic Tests", 16),
    ("Comparison Tests", 13),
    ("Logical Tests", 36),
    ("Variable Tests", 22),
    ("Variable Space Tests", 22),
    ("Function Tests", 8),
    ("Special Value Tests", 60),
    ("Complex Expression Tests", 10),
    ("Edge Case Tests", 10),
    ("Operator Precedence Tests", 16),
    ("Parentheses Tests", 25),
    ("Nested Unary Tests", 41),
    ("Real-World Expression Tests", 44),
    ("API Function Tests", 4),
    ("Number Overflow Tests", 11),
    ("Combined Complex Expressions", 18),
    ("Crash Tests", 9),
];

/// The lookups C's test callback answered during one evaluation, replayed in order: C's 71-name table stays in C.
struct Replay {
    recorded: VecDeque<(Vec<u8>, Option<f64>)>,
    line: usize,
}

impl Replay {
    /// A row's lookups: joined with 0x1f, each its found flag, 16 hex digits of the value, then the name.
    fn new(row: &Row, field: usize) -> Self {
        let recorded = row
            .bytes(field)
            .split(|&b| b == 0x1f)
            .filter(|lookup| !lookup.is_empty())
            .map(|lookup| {
                let bits = u64::from_str_radix(std::str::from_utf8(&lookup[1..17]).expect("hex"), 16).expect("hex");
                (lookup[17..].to_vec(), (lookup[0] == b'1').then(|| f64::from_bits(bits)))
            })
            .collect();
        Replay { recorded, line: row.line }
    }

    fn finished(&self) {
        assert!(self.recorded.is_empty(), "c_unittest.tsv:{}: C made more lookups: {:?}", self.line, self.recorded);
    }
}

impl Resolver for Replay {
    fn lookup(&mut self, name: &[u8]) -> Option<f64> {
        let (expected, value) = self.recorded.pop_front().unwrap_or_else(|| {
            panic!("c_unittest.tsv:{}: a lookup C did not make: {}", self.line, String::from_utf8_lossy(name))
        });
        assert_eq!(bytes(name), bytes(&expected), "c_unittest.tsv:{}: lookups in C's order", self.line);
        value
    }
}

/// C's comparison of a result with its expectation (`run_test_group()`).
fn result_as_expected(expected: f64, actual: f64) -> bool {
    if expected.is_nan() {
        return actual.is_nan();
    }
    if expected.is_infinite() {
        return actual.is_infinite();
    }
    actual.is_nan() || actual.is_infinite() || (expected - actual).abs() <= 0.000001
}

#[test]
fn c_unit_vectors_are_all_there() {
    let rows = rows("c_unittest.tsv");
    assert_eq!(rows.len(), 365);
    let mut at = 0;
    for (group, count) in GROUPS {
        for index in 0..count {
            assert_eq!((rows[at].str(0), rows[at].num::<usize>(1)), (group, index), "c_unittest.tsv:{}", rows[at].line);
            at += 1;
        }
    }
}

#[test]
fn c_unit_vectors_pass_as_c_asserts() {
    for row in rows("c_unittest.tsv") {
        let at = format!("c_unittest.tsv:{}: {}", row.line, row.str(2));
        let (expected_result, expected_error, should_parse) = (double(&row, 3), row.num::<i32>(4), row.flag(5));

        let Ok(mut expression) = Expression::parse(row.bytes(2)) else {
            assert!(!should_parse, "{at}: expected parsing to succeed");
            continue;
        };
        assert!(should_parse, "{at}: expected parsing to fail");

        let mut replay = Replay::new(&row, 11);
        expression.evaluate(&mut replay);
        replay.finished();

        assert_eq!(expression.error(), expected_error, "{at}");
        if expected_error == ERROR_OK {
            assert!(
                result_as_expected(expected_result, expression.result()),
                "{at}: expected {expected_result}, got {}",
                expression.result()
            );
        }
    }
}

#[test]
fn c_unit_vectors_equal_c_outputs() {
    for row in rows("c_unittest.tsv") {
        let at = format!("c_unittest.tsv:{}: {}", row.line, row.str(2));
        let parsed = Expression::parse(row.bytes(2));
        assert_eq!(parse_fields(&parsed), fields(&row, 6, 3), "{at}");

        let Ok(mut expression) = parsed else {
            continue;
        };
        assert_eq!(vec![bytes(expression.source()), bytes(expression.parsed_as())], fields(&row, 9, 2), "{at}");

        let mut replay = Replay::new(&row, 11);
        assert_eq!(evaluation_fields(&mut expression, &mut replay), fields(&row, 12, 4), "{at}");
        replay.finished();
    }
}

/// The extra checks C makes on the "API Function Tests" group.
#[test]
fn api_function_extras() {
    let mut seen = 0;
    for row in rows("c_unittest.tsv").iter().filter(|row| row.str(0) == "API Function Tests") {
        let Ok(mut expression) = Expression::parse(row.bytes(2)) else {
            continue;
        };
        expression.evaluate(&mut Replay::new(row, 11));

        if row.str(2).contains("hardcoded_var") {
            // expression_hardcode_variable(), then a second evaluation that looks nothing up
            expression.hardcode_variable(b"hardcoded_var", 123.456);
            assert!(expression.evaluate(&mut NoVariables));
            assert_eq!(expression.error(), ERROR_OK);
            assert!((expression.result() - 123.456).abs() <= 0.000001);
            seen += 1;
        } else if row.str(2) == "1 + 2" {
            assert_eq!(bytes(expression.source()), bytes(b"1 + 2"));
            assert!(!expression.parsed_as().is_empty());
            assert!((expression.result() - 3.0).abs() <= 0.000001);
            seen += 1;
        }
    }
    assert_eq!(seen, 2);
}

/// A string field of the hardcode dump: C's NULL is written as a lone NUL byte.
fn nullable(row: &Row, i: usize) -> Option<&[u8]> {
    (row.bytes(i) != [0]).then(|| row.bytes(i))
}

#[test]
fn c_hardcode_vectors() {
    let rows = rows("c_unittest_hardcode.tsv");
    assert_eq!(rows.len(), 13);

    let mut without_expression = 0;
    for row in &rows {
        let at = format!("c_unittest_hardcode.tsv:{}: {}", row.line, row.str(1));
        let Some(input) = nullable(row, 2) else {
            // "NULL expression (shouldn't crash)": an absent expression is the caller's `Option` here
            without_expression += 1;
            continue;
        };

        let parsed = Expression::parse(input);
        assert_eq!(parse_fields(&parsed), fields(row, 8, 3), "{at}");
        let mut expression = parsed.unwrap_or_else(|_| panic!("{at}: C parses it"));

        // a NULL variable is a call that does nothing
        if let Some(variable) = nullable(row, 3) {
            expression.hardcode_variable(variable, double(row, 4));
        }

        // what C asserts
        if let Some(expected_source) = nullable(row, 5) {
            assert_eq!(bytes(expression.source()), bytes(expected_source), "{at}");
        }
        let (expected_result, expected_error) = (double(row, 6), row.num::<i32>(7));

        // what C returned: C's test sets no lookup callback
        let mut actual: Vec<Bytes> = vec![bytes(expression.source()), bytes(expression.parsed_as())];
        actual.extend(evaluation_fields(&mut expression, &mut NoVariables));
        assert_eq!(actual, fields(row, 11, 6), "{at}");

        assert_eq!(expression.error(), expected_error, "{at}");
        if expected_error == ERROR_OK {
            assert!((expression.result() - expected_result).abs() <= 0.000001, "{at}");
        }
    }
    assert_eq!(without_expression, 1);
}
