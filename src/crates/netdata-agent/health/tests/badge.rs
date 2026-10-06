//! The badge's pure functions against C (`tests/oracle/gen-loop-vectors.c`, mode `badge`: C's own
//! `web_buffer_svg.c` with its static functions exported by `tests/oracle/badge-splice.inc`): the width of a text,
//! the XML escape, the color a value takes from an expression, a color argument, and the whole SVG text. The value's
//! text with a precision is `values_with_a_precision_match_c` in `tests/loop.rs`.

mod common;

use common::{nullable, rows};
use netdata_agent_health::badge::{Svg, buffer_svg, calc_colorz, escape_xmlz, parse_color_argument, verdana11_width};

/// A double as the vectors hold it: `nan`, or its bits in hex.
fn double(field: &str) -> f64 {
    match field {
        "nan" => f64::NAN,
        bits => f64::from_bits(u64::from_str_radix(bits, 16).expect("a double's bits")),
    }
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn judge(file: &str, checked: usize, want: usize, failures: Vec<String>) {
    let shown = failures[..failures.len().min(12)].join("\n");
    assert!(failures.is_empty(), "{file}: {} of {checked} differ:\n{shown}", failures.len());
    assert_eq!(checked, want, "{file}");
}

/// The width of every byte alone, of multi-byte characters whole and cut, and of whole texts, to the last bit.
#[test]
fn widths_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("badge-width.tsv") {
        let width = verdana11_width(row.bytes(0));
        if width.to_bits() != double(row.str(1)).to_bits() {
            failures.push(format!("line {}: {:?}: C {}, Rust {width}", row.line, row.str(0), double(row.str(1))));
        }
        checked += 1;
    }
    judge("badge-width.tsv", checked, 291, failures);
}

/// The escape at the label's and the value's sizes: each special character around the limit, and how many bytes
/// C says it wrote.
#[test]
fn escapes_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("badge-escape.tsv") {
        let limit: usize = row.str(1).parse().expect("a limit");
        let escaped = escape_xmlz(row.bytes(0), limit);
        let used: usize = row.str(3).parse().expect("a length");
        if escaped != row.bytes(2) || escaped.len() != used {
            failures.push(format!(
                "line {}: at {limit}: C {:?} ({used}), Rust {:?} ({})",
                row.line,
                row.str(2),
                lossy(&escaped),
                escaped.len()
            ));
        }
        checked += 1;
    }
    judge("badge-escape.tsv", checked, 228, failures);
}

/// The text C's `calc_colorz()` chooses for each expression and value, and the color `parse_color_argument()` then
/// makes of it with the SVG's default.
#[test]
fn colors_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("badge-color.tsv") {
        let value = double(row.str(1));
        let chosen = calc_colorz(row.bytes(0), 100, value);
        let color = parse_color_argument(Some(&chosen), b"555");
        if chosen != row.bytes(2) || color != row.bytes(3) {
            failures.push(format!(
                "line {}: {:?} for {value}: C {:?} then {:?}, Rust {:?} then {:?}",
                row.line,
                &row.str(0)[..row.str(0).len().min(60)],
                &row.str(2)[..row.str(2).len().min(40)],
                row.str(3),
                &lossy(&chosen)[..chosen.len().min(40)],
                lossy(color)
            ));
        }
        checked += 1;
    }
    judge("badge-color.tsv", checked, 1612, failures);
}

/// A color argument: names, hex texts of every length, the length limits, no argument, and no default.
#[test]
fn color_arguments_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("badge-color-arg.tsv") {
        let (argument, default, want) = (nullable(&row, 0), nullable(&row, 1), nullable(&row, 2));
        // C's default may be NULL; the port's callers always give one, so an absent default is checked as absent
        const NONE: &[u8] = b"\x00none";
        let color = parse_color_argument(argument, default.unwrap_or(NONE));
        let color = (color != NONE).then_some(color);
        if color != want {
            let (shown, or) = (row.str(0), row.str(1));
            failures.push(format!("line {}: {shown:?} or {or:?}: C {want:?}, Rust {color:?}", row.line));
        }
        checked += 1;
    }
    judge("badge-color-arg.tsv", checked, 210, failures);
}

/// The whole SVG text, byte for byte, over scales, fixed widths, values, units, colors, labels and precisions.
#[test]
fn svgs_match_c() {
    let (mut checked, mut failures) = (0, Vec::new());
    for row in rows("badge-svg.tsv") {
        let number = |i: usize| row.str(i).parse::<i64>().expect("a number");
        let svg = Svg {
            label: row.bytes(0),
            value: double(row.str(1)),
            units: row.bytes(2),
            label_color: nullable(&row, 3),
            value_color: nullable(&row, 4),
            precision: number(5) as i32,
            scale: number(6) as i32,
            options: number(7) as u32,
            fixed_width_lbl: number(8) as i32,
            fixed_width_val: number(9) as i32,
            text_color_lbl: nullable(&row, 10),
            text_color_val: nullable(&row, 11),
        };
        let body = buffer_svg(&svg);
        // CT_IMAGE_SVG_XML
        if body != row.bytes(13) || row.str(12) != "16" {
            let at = body.iter().zip(row.bytes(13)).take_while(|(a, b)| a == b).count();
            let from = at.saturating_sub(40);
            failures.push(format!(
                "line {}: differs at byte {at}: C ...{:?}, Rust ...{:?}",
                row.line,
                lossy(&row.bytes(13)[from..row.bytes(13).len().min(at + 60)]),
                lossy(&body[from..body.len().min(at + 60)])
            ));
        }
        checked += 1;
    }
    judge("badge-svg.tsv", checked, 352, failures);
}
