//! The differential corpus: every expression of `tests/vectors/corpus.tsv` and `hardcode.tsv` gives what C's
//! production evaluator gave (`tests/oracle/gen-eval-vectors.c`).

mod common;

use std::collections::{BTreeSet, HashMap};

use common::{Bytes, Row, bytes, check, double, evaluation_fields, fields, parse_fields, rows};
use netdata_agent_eval::{Expression, NoVariables, Resolver};

/// A binding set of `bindings.tsv`; every other name is unknown.
struct Bindings(HashMap<Vec<u8>, f64>);

impl Resolver for &Bindings {
    fn lookup(&mut self, name: &[u8]) -> Option<f64> {
        self.0.get(name).copied()
    }
}

fn bindings(set: u32) -> Bindings {
    Bindings(
        rows("bindings.tsv")
            .iter()
            .filter(|row| row.num::<u32>(0) == set)
            .map(|row| (row.bytes(1).to_vec(), double(row, 2)))
            .collect(),
    )
}

#[test]
fn parse_matches_c() {
    // outcome, code, failed_at, source, parsed_as
    let count = check(
        "corpus.tsv",
        |row| fields(row, 2, 5),
        |row| {
            let parsed = Expression::parse(row.bytes(1));
            let mut actual = parse_fields(&parsed);
            match &parsed {
                Ok(expression) => actual.extend([bytes(expression.source()), bytes(expression.parsed_as())]),
                Err(_) => actual.extend([bytes(b"-"), bytes(b"-")]),
            }
            actual
        },
    );
    assert_eq!(count, 37463);

    let families: BTreeSet<String> = rows("corpus.tsv").iter().map(|row| row.str(0).to_owned()).collect();
    let expected = ["depth", "err", "eval", "kw", "mut", "num", "pair", "rand", "seq", "spell", "var", "ws"];
    assert_eq!(families, expected.iter().map(|family| family.to_string()).collect());
}

#[test]
fn parse_failures_all_report_code_5() {
    for row in rows("corpus.tsv").iter().filter(|row| row.str(2) == "F") {
        assert_eq!(row.str(3), "5", "corpus.tsv:{}", row.line);
    }
}

#[test]
fn evaluation_matches_c() {
    let (finite, special) = (bindings(1), bindings(2));
    // the three evaluations run on one expression, as in C, so each must replace the state the last one left
    check(
        "corpus.tsv",
        |row| fields(row, 7, 12),
        |row| match Expression::parse(row.bytes(1)) {
            Ok(mut expression) => {
                let mut actual = evaluation_fields(&mut expression, &mut NoVariables);
                actual.extend(evaluation_fields(&mut expression, &mut &finite));
                actual.extend(evaluation_fields(&mut expression, &mut &special));
                actual
            }
            Err(_) => vec![bytes(b"-"); 12],
        },
    );
}

fn hardcode(expression: &mut Expression, row: &Row, name: usize) {
    expression.hardcode_variable(row.bytes(name), double(row, name + 1));
}

#[test]
fn hardcode_matches_c() {
    let finite = bindings(1);
    // source and parsed_as after the hardcodes, then one evaluation
    let count = check(
        "hardcode.tsv",
        |row| fields(row, 6, 6),
        |row| {
            let mut expression = Expression::parse(row.bytes(0)).expect("hardcode inputs parse");
            hardcode(&mut expression, row, 2);
            if row.num::<u32>(1) == 2 {
                hardcode(&mut expression, row, 4);
            }
            let mut actual: Vec<Bytes> = vec![bytes(expression.source()), bytes(expression.parsed_as())];
            actual.extend(evaluation_fields(&mut expression, &mut &finite));
            actual
        },
    );
    assert_eq!(count, 593);
}
