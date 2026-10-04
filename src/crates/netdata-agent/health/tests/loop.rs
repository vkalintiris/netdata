//! The evaluation loop against C (`tests/oracle/gen-loop-vectors.c`: `units.tsv`, `delay.tsv`, `loop.tsv`).

mod common;

use common::rows;
use netdata_agent_text::units::format_value_and_unit;

/// A double as the loop's vectors hold it: `nan`, or its bits in hex.
fn double(field: &str) -> f64 {
    match field {
        "nan" => f64::NAN,
        bits => f64::from_bits(u64::from_str_radix(bits, 16).expect("a double's bits")),
    }
}

/// C's text for each value under each unit, as an alert log entry's value texts are made.
#[test]
fn units_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("units.tsv") {
        let (value, units) = (double(row.str(0)), row.bytes(1));
        let text = format_value_and_unit(value, units);
        if text != row.bytes(2) {
            failures.push(format!(
                "units.tsv:{}: {value:?} {:?}: C {:?}, Rust {:?}",
                row.line,
                String::from_utf8_lossy(units),
                String::from_utf8_lossy(row.bytes(2)),
                String::from_utf8_lossy(&text)
            ));
        }
        checked += 1;
    }
    let shown = failures[..failures.len().min(20)].join("\n");
    assert!(failures.is_empty(), "{} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, 2784);
}
