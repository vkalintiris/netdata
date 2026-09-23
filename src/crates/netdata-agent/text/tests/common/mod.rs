//! Readers for the golden vector files written by `tests/oracle/gen-vectors.c`.

#![allow(dead_code)]

use std::fmt::Debug;
use std::path::PathBuf;

/// Decodes a vector field: `\\` is a backslash, `\xHH` a byte, anything else literal.
pub fn unescape(field: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(field.len());
    let mut i = 0;
    while i < field.len() {
        if field[i] == b'\\' {
            match field.get(i + 1) {
                Some(b'\\') => {
                    out.push(b'\\');
                    i += 2;
                }
                Some(b'x') => {
                    let hex = std::str::from_utf8(&field[i + 2..i + 4]).expect("hex escape");
                    out.push(u8::from_str_radix(hex, 16).expect("hex escape"));
                    i += 4;
                }
                other => panic!("bad escape {other:?} in {}", String::from_utf8_lossy(field)),
            }
        } else {
            out.push(field[i]);
            i += 1;
        }
    }
    out
}

/// One line of a vector file, with its fields decoded.
pub struct Row {
    pub line: usize,
    pub fields: Vec<Vec<u8>>,
}

impl Row {
    pub fn bytes(&self, i: usize) -> &[u8] {
        &self.fields[i]
    }

    pub fn str(&self, i: usize) -> &str {
        std::str::from_utf8(&self.fields[i]).expect("utf-8 field")
    }

    pub fn num<T: std::str::FromStr>(&self, i: usize) -> T
    where
        T::Err: Debug,
    {
        self.str(i).parse().expect("numeric field")
    }

    pub fn flag(&self, i: usize) -> bool {
        match self.str(i) {
            "0" => false,
            "1" => true,
            other => panic!("bad flag {other}"),
        }
    }

    /// An f64 given as its IEEE-754 bits in hex.
    pub fn bits(&self, i: usize) -> u64 {
        u64::from_str_radix(self.str(i), 16).expect("hex bits")
    }
}

/// The decoded rows of `tests/vectors/<name>`. Comments are only allowed in
/// the header, so a data line can never be skipped as one.
pub fn rows(name: &str) -> Vec<Row> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "vectors", name]
        .iter()
        .collect();
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let mut rows = Vec::new();
    for (n, line) in data.split(|&b| b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        if line[0] == b'#' {
            assert!(rows.is_empty(), "{name}:{}: comment after data", n + 1);
            continue;
        }
        rows.push(Row {
            line: n + 1,
            fields: line.split(|&b| b == b'\t').map(unescape).collect(),
        });
    }
    assert!(!rows.is_empty(), "{name} has no vectors");
    rows
}

/// Compares `actual(row)` with `expected(row)` for every row of `name`,
/// reporting the first mismatches; returns the number of rows checked.
pub fn check<T: PartialEq + Debug>(
    name: &str,
    expected: impl Fn(&Row) -> T,
    actual: impl Fn(&Row) -> T,
) -> usize {
    let rows = rows(name);
    let mut failures = Vec::new();
    for row in &rows {
        let (e, a) = (expected(row), actual(row));
        if e != a {
            failures.push(format!(
                "{name}:{}: input {:?}\n  expected {e:?}\n  actual   {a:?}",
                row.line,
                String::from_utf8_lossy(&row.fields[0])
            ));
        }
    }
    let shown: Vec<&String> = failures.iter().take(20).collect();
    assert!(
        failures.is_empty(),
        "{} of {} vectors differ:\n{}",
        failures.len(),
        rows.len(),
        shown
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    rows.len()
}

/// Lossy text of bytes, for readable comparisons.
pub fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}
