//! Shared by the vector tests: the reader of `netdata-agent-text` (the vectors use its field encoding), and what
//! a parse and an evaluation look like as vector fields.

#![allow(dead_code, unused_imports)]

#[path = "../../../text/tests/common/mod.rs"]
mod reader;

use std::fmt;

use netdata_agent_eval::{Expression, ParseError, Resolver};

pub use reader::{Row, check, rows};

/// A field's bytes: compared exactly, shown as text.
#[derive(PartialEq, Eq, Clone)]
pub struct Bytes(pub Vec<u8>);

impl fmt::Debug for Bytes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", String::from_utf8_lossy(&self.0))
    }
}

pub fn bytes(b: &[u8]) -> Bytes {
    Bytes(b.to_vec())
}

/// The fields `n..n + count` of a row.
pub fn fields(row: &Row, n: usize, count: usize) -> Vec<Bytes> {
    (n..n + count).map(|i| bytes(row.bytes(i))).collect()
}

/// A parse as the vectors record it: outcome (E, F or K), code, failed_at; `-` where a field does not apply.
pub fn parse_fields(parsed: &Result<Expression, ParseError>) -> Vec<Bytes> {
    let number = |n: Option<String>| Bytes(n.map_or(b"-".to_vec(), String::into_bytes));
    match parsed {
        Ok(_) => vec![bytes(b"K"), bytes(b"0"), bytes(b"-")],
        Err(error) => vec![
            bytes(if *error == ParseError::Empty { b"E" } else { b"F" }),
            number(error.code().map(|code| code.to_string())),
            number(error.failed_at().map(|at| at.to_string())),
        ],
    }
}

/// An evaluation as the vectors record it: return value, error code, result bits, error_msg.
pub fn evaluation_fields(expression: &mut Expression, vars: &mut dyn Resolver) -> Vec<Bytes> {
    let ok = expression.evaluate(vars);
    vec![
        bytes(if ok { b"1" } else { b"0" }),
        Bytes(expression.error().to_string().into_bytes()),
        Bytes(format!("{:016x}", expression.result().to_bits()).into_bytes()),
        bytes(expression.error_msg()),
    ]
}

/// An f64 from its bits in a field.
pub fn double(row: &Row, i: usize) -> f64 {
    f64::from_bits(row.bits(i))
}
