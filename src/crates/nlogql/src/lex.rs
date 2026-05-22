//! Leaf parsers for LogQL lexical primitives.
//!
//! chumsky parses straight from chars, so there is no separate
//! token-producing lexer. The functions here return parsers that
//! consume one primitive's worth of input and yield its decoded
//! value (a `String`, `f64`, `i64`, `u64`, etc.). Higher-level
//! productions in [`crate::parser`] compose them.
//!
//! Each parser is whitespace-greedy *only inside its own syntax*
//! (e.g. a duration can be `1h30m` with no internal spaces, but
//! `1h 30m` is two tokens). Outer-level whitespace handling lives
//! in the caller; [`ws`] consumes whitespace and `#`-comments.
//!
//! Spec reference: `~/.cache/nlogql-loki-reference/lex.go`.

use chumsky::prelude::*;

use crate::Extra;

// ---------------------------------------------------------------
// Whitespace + comments

/// Consume zero or more whitespace characters and `#`-comments.
/// A comment runs from `#` to the next newline (or EOF).
pub fn ws<'a>() -> impl Parser<'a, &'a str, (), Extra<'a>> + Clone {
    let space = any().filter(|c: &char| c.is_whitespace()).ignored();
    let comment = just('#')
        .then(any().and_is(just('\n').not()).repeated())
        .ignored();
    choice((space, comment)).repeated().ignored()
}

// ---------------------------------------------------------------
// Identifiers

/// LogQL identifier: a letter or `_`, then any number of letters,
/// digits, or `_`. Matches Go's `text/scanner` identifier rule
/// (Unicode-aware via Rust's `char::is_alphabetic`).
pub fn identifier<'a>() -> impl Parser<'a, &'a str, &'a str, Extra<'a>> + Clone {
    let first = any().filter(|c: &char| c.is_alphabetic() || *c == '_');
    let rest = any()
        .filter(|c: &char| c.is_alphanumeric() || *c == '_')
        .repeated();
    first.then(rest).to_slice()
}

// ---------------------------------------------------------------
// String literals

/// String literal — either double-quoted with Go-style escapes or
/// backtick-delimited raw. Returns the decoded contents.
///
/// LogQL has no single-quoted form (Loki's lexer uses Go's
/// `text/scanner` which recognizes only `scanner.String` and
/// `scanner.RawString`).
pub fn string_literal<'a>() -> impl Parser<'a, &'a str, String, Extra<'a>> + Clone {
    let escape = just('\\')
        .ignore_then(any())
        .map(|c: char| match c {
            'n' => '\n',
            't' => '\t',
            'r' => '\r',
            '"' => '"',
            '\\' => '\\',
            '\'' => '\'',
            '0' => '\0',
            'a' => '\x07',
            'b' => '\x08',
            'f' => '\x0c',
            'v' => '\x0b',
            // Unknown escape: pass through. Loki's strconv.Unquote
            // would error here; we're lenient for now and will
            // tighten when wiring error messages in SOW-15.
            other => other,
        });
    let normal = any().filter(|c: &char| *c != '"' && *c != '\\');
    let double_quoted = choice((escape, normal))
        .repeated()
        .collect::<String>()
        .delimited_by(just('"'), just('"'));

    let raw = any()
        .filter(|c: &char| *c != '`')
        .repeated()
        .collect::<String>()
        .delimited_by(just('`'), just('`'));

    choice((double_quoted, raw))
}

// ---------------------------------------------------------------
// Numbers

/// Numeric literal: decimal int/float (with optional scientific
/// exponent), hex (`0x…`), or binary (`0b…`). Returned as `f64` —
/// Loki stores numbers as strings and parses to `float64` at AST
/// construction time; we collapse the same way.
///
/// Does **not** consume a leading sign. Per Loki, `-` is a separate
/// `SUB` token when followed by a number.
pub fn number<'a>() -> impl Parser<'a, &'a str, f64, Extra<'a>> + Clone {
    let digits = any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1);
    let hex_digits = any()
        .filter(|c: &char| c.is_ascii_hexdigit())
        .repeated()
        .at_least(1);
    let bin_digits = any()
        .filter(|c: &char| *c == '0' || *c == '1')
        .repeated()
        .at_least(1);

    // Order matters: try hex/binary (which start with "0x"/"0b")
    // before the decimal path, otherwise "0" would consume the
    // leading zero of "0x10".
    let hex = just("0x")
        .or(just("0X"))
        .then(hex_digits)
        .to_slice()
        .map(|s: &str| {
            let body = &s[2..];
            i64::from_str_radix(body, 16).unwrap_or(0) as f64
        });

    let binary = just("0b")
        .or(just("0B"))
        .then(bin_digits)
        .to_slice()
        .map(|s: &str| {
            let body = &s[2..];
            i64::from_str_radix(body, 2).unwrap_or(0) as f64
        });

    let exponent = one_of("eE")
        .then(one_of("+-").or_not())
        .then(digits)
        .ignored();
    let frac = just('.').then(digits).ignored();
    let decimal = digits
        .then(frac.or_not())
        .then(exponent.or_not())
        .to_slice()
        .map(|s: &str| s.parse::<f64>().unwrap_or(0.0));

    choice((hex, binary, decimal))
}

// ---------------------------------------------------------------
// Durations

/// Duration literal: optional `-` sign, then one or more
/// `<number><unit>` segments with no whitespace between them.
/// Returns the total in nanoseconds (signed `i64`).
///
/// Units: `ns`, `us`, `µs`, `ms`, `s`, `m`, `h`, `d`, `w`, `y`.
/// `d`/`w`/`y` use Prometheus's calendar-unit definitions
/// (1d = 24h, 1w = 7d, 1y = 365d).
pub fn duration<'a>() -> impl Parser<'a, &'a str, i64, Extra<'a>> + Clone {
    const NS: i64 = 1;
    const US: i64 = 1_000;
    const MS: i64 = 1_000_000;
    const S: i64 = 1_000_000_000;
    const M: i64 = 60 * S;
    const H: i64 = 60 * M;
    const D: i64 = 24 * H;
    const W: i64 = 7 * D;
    const Y: i64 = 365 * D;

    // Longer suffixes first so they win the alt.
    let unit = choice((
        just("ns").to(NS),
        just("us").to(US),
        just("µs").to(US),
        just("ms").to(MS),
        just("s").to(S),
        just("m").to(M),
        just("h").to(H),
        just("d").to(D),
        just("w").to(W),
        just("y").to(Y),
    ));

    let digits_then_frac = any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1)
        .then(
            just('.')
                .then(any().filter(|c: &char| c.is_ascii_digit()).repeated())
                .or_not(),
        )
        .to_slice();

    let segment = digits_then_frac
        .then(unit)
        .map(|(num_str, mult): (&str, i64)| {
            let n: f64 = num_str.parse().unwrap_or(0.0);
            (n * mult as f64) as i64
        });

    let sign = just('-').or_not();
    sign.then(segment.repeated().at_least(1).collect::<Vec<i64>>())
        .map(|(sign, parts)| {
            let total: i64 = parts.into_iter().sum();
            if sign.is_some() { -total } else { total }
        })
}

// ---------------------------------------------------------------
// Bytes

/// Byte literal: a non-negative number followed by a binary
/// (`KiB`/`MiB`/…) or decimal (`KB`/`MB`/…) suffix. Returns the
/// value in bytes.
///
/// Loki disallows negative byte literals (per `lex_test.go`).
pub fn bytes<'a>() -> impl Parser<'a, &'a str, u64, Extra<'a>> + Clone {
    const KIB: u64 = 1 << 10;
    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    const TIB: u64 = 1 << 40;
    const PIB: u64 = 1 << 50;
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    const TB: u64 = 1_000_000_000_000;
    const PB: u64 = 1_000_000_000_000_000;

    // Longer / case-disambiguating suffixes first.
    let suffix = choice((
        just("KiB").to(KIB),
        just("MiB").to(MIB),
        just("GiB").to(GIB),
        just("TiB").to(TIB),
        just("PiB").to(PIB),
        just("kB").to(KB),
        just("KB").to(KB),
        just("MB").to(MB),
        just("GB").to(GB),
        just("TB").to(TB),
        just("PB").to(PB),
        just("B").to(1_u64),
    ));

    let number_chars = any()
        .filter(|c: &char| c.is_ascii_digit())
        .repeated()
        .at_least(1)
        .then(
            just('.')
                .then(any().filter(|c: &char| c.is_ascii_digit()).repeated())
                .or_not(),
        )
        .to_slice();

    number_chars
        .then(suffix)
        .map(|(n_str, mult): (&str, u64)| {
            let n: f64 = n_str.parse().unwrap_or(0.0);
            (n * mult as f64) as u64
        })
}

// ---------------------------------------------------------------
// Tests

#[cfg(test)]
mod tests {
    use super::*;

    fn run<'a, T>(p: impl Parser<'a, &'a str, T, Extra<'a>>, input: &'a str) -> Option<T> {
        p.then_ignore(end()).parse(input).into_result().ok()
    }

    // --- identifier ---------------------------------------------

    #[test]
    fn identifier_basic() {
        assert_eq!(run(identifier(), "foo"), Some("foo"));
        assert_eq!(run(identifier(), "latency"), Some("latency"));
        assert_eq!(run(identifier(), "IPAddress"), Some("IPAddress"));
        assert_eq!(run(identifier(), "_underscore"), Some("_underscore"));
        assert_eq!(run(identifier(), "a1"), Some("a1"));
    }

    #[test]
    fn identifier_rejects_leading_digit() {
        assert!(run(identifier(), "1foo").is_none());
    }

    #[test]
    fn identifier_rejects_empty() {
        assert!(run(identifier(), "").is_none());
    }

    // --- string_literal -----------------------------------------

    #[test]
    fn string_double_quoted() {
        assert_eq!(run(string_literal(), r#""hello""#), Some("hello".to_string()));
        assert_eq!(run(string_literal(), r#""""#), Some(String::new()));
    }

    #[test]
    fn string_escapes() {
        assert_eq!(run(string_literal(), r#""a\nb""#), Some("a\nb".to_string()));
        assert_eq!(
            run(string_literal(), r#""tab\there""#),
            Some("tab\there".to_string()),
        );
        assert_eq!(
            run(string_literal(), r#""quote: \"x\"""#),
            Some(r#"quote: "x""#.to_string()),
        );
        assert_eq!(
            run(string_literal(), r#""back\\slash""#),
            Some(r"back\slash".to_string()),
        );
    }

    #[test]
    fn string_raw() {
        assert_eq!(run(string_literal(), r"`raw \w+`"), Some(r"raw \w+".to_string()));
        assert_eq!(run(string_literal(), "`with\nnewline`"), Some("with\nnewline".to_string()));
    }

    #[test]
    fn string_rejects_unclosed() {
        assert!(run(string_literal(), r#""abc"#).is_none());
        assert!(run(string_literal(), r"`abc").is_none());
    }

    // --- number -------------------------------------------------

    #[test]
    fn number_int() {
        assert_eq!(run(number(), "0"), Some(0.0));
        assert_eq!(run(number(), "123"), Some(123.0));
    }

    #[test]
    fn number_float() {
        assert_eq!(run(number(), "1.5"), Some(1.5));
        assert_eq!(run(number(), "4.00"), Some(4.0));
        assert_eq!(run(number(), "0.99998"), Some(0.99998));
    }

    #[test]
    fn number_scientific() {
        assert_eq!(run(number(), "1e3"), Some(1000.0));
        assert_eq!(run(number(), "1.5e-2"), Some(0.015));
        assert_eq!(run(number(), "2E10"), Some(2e10));
    }

    #[test]
    fn number_hex() {
        assert_eq!(run(number(), "0x10"), Some(16.0));
        assert_eq!(run(number(), "0xFF"), Some(255.0));
    }

    #[test]
    fn number_binary() {
        // From lex_test.go: 0b01, 0b10.
        assert_eq!(run(number(), "0b01"), Some(1.0));
        assert_eq!(run(number(), "0b10"), Some(2.0));
    }

    #[test]
    fn number_does_not_consume_leading_sign() {
        // `-123` should not parse as a number; the `-` is SUB.
        assert!(run(number(), "-123").is_none());
    }

    // --- duration -----------------------------------------------

    #[test]
    fn duration_units() {
        const NS: i64 = 1;
        const S: i64 = 1_000_000_000;
        assert_eq!(run(duration(), "1ns"), Some(NS));
        assert_eq!(run(duration(), "1s"), Some(S));
        assert_eq!(run(duration(), "1ms"), Some(1_000_000));
        assert_eq!(run(duration(), "1us"), Some(1_000));
        assert_eq!(run(duration(), "1µs"), Some(1_000));
        assert_eq!(run(duration(), "1m"), Some(60 * S));
        assert_eq!(run(duration(), "1h"), Some(60 * 60 * S));
        assert_eq!(run(duration(), "1d"), Some(24 * 60 * 60 * S));
        assert_eq!(run(duration(), "1w"), Some(7 * 24 * 60 * 60 * S));
        assert_eq!(run(duration(), "1y"), Some(365 * 24 * 60 * 60 * S));
    }

    #[test]
    fn duration_multi_segment() {
        const S: i64 = 1_000_000_000;
        // From lex_test.go: 1h15m30.918273645s
        let expect = 60 * 60 * S + 15 * 60 * S + 30 * S + 918_273_645;
        assert_eq!(run(duration(), "1h15m30.918273645s"), Some(expect));
        // 1h0.0m0s — degenerate but valid.
        assert_eq!(run(duration(), "1h0.0m0s"), Some(60 * 60 * S));
    }

    #[test]
    fn duration_negative() {
        const S: i64 = 1_000_000_000;
        assert_eq!(run(duration(), "-1s"), Some(-S));
        assert_eq!(run(duration(), "-123ms"), Some(-123_000_000));
    }

    // --- bytes --------------------------------------------------

    #[test]
    fn bytes_units() {
        assert_eq!(run(bytes(), "1B"), Some(1));
        assert_eq!(run(bytes(), "1KB"), Some(1_000));
        assert_eq!(run(bytes(), "1kB"), Some(1_000));
        assert_eq!(run(bytes(), "1KiB"), Some(1024));
        assert_eq!(run(bytes(), "1MB"), Some(1_000_000));
        assert_eq!(run(bytes(), "1MiB"), Some(1024 * 1024));
        assert_eq!(run(bytes(), "1GiB"), Some(1024 * 1024 * 1024));
    }

    #[test]
    fn bytes_decimal_value() {
        // 250kB and 200MiB from lex_test.go
        assert_eq!(run(bytes(), "250kB"), Some(250_000));
        assert_eq!(run(bytes(), "200MiB"), Some(200 * 1024 * 1024));
    }

    #[test]
    fn bytes_zero_and_one() {
        assert_eq!(run(bytes(), "0B"), Some(0));
        assert_eq!(run(bytes(), "1B"), Some(1));
    }

    #[test]
    fn bytes_no_negative() {
        // Loki disallows negative bytes — our parser should refuse a
        // leading minus too.
        assert!(run(bytes(), "-1B").is_none());
    }

    // --- ws -----------------------------------------------------

    #[test]
    fn ws_empty() {
        assert!(ws().then_ignore(end()).parse("").into_result().is_ok());
    }

    #[test]
    fn ws_spaces_and_tabs() {
        assert!(ws().then_ignore(end()).parse("   \t\n  ").into_result().is_ok());
    }

    #[test]
    fn ws_comment() {
        // Comment runs to EOL.
        assert!(ws().then_ignore(end()).parse("# a comment").into_result().is_ok());
    }

    #[test]
    fn ws_comment_then_more() {
        // From lex_test.go: "{foo=\"bar\"} #|~ \"\\w+\"" — comment
        // tail discarded.
        assert!(
            ws()
                .then_ignore(end())
                .parse("  # comment 1\n# comment 2\n   ")
                .into_result()
                .is_ok()
        );
    }
}
