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
