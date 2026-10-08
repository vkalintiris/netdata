//! Simple patterns against the C golden vectors, plus the examples of
//! `netdata -W simple-pattern`.

mod common;

use common::{Row, check, text};
use netdata_agent_text::simple_pattern::{
    Separators, SimplePattern, SimplePatternMode, SimplePatternResult, contains_wildcards,
};

fn separators(code: &[u8]) -> Separators<'_> {
    match code {
        b"W" => Separators::Whitespace,
        b"N" => Separators::None,
        _ => Separators::Bytes(code.strip_prefix(b"B:").expect("separators code")),
    }
}

fn mode(code: &str) -> SimplePatternMode {
    match code {
        "E" => SimplePatternMode::Exact,
        "P" => SimplePatternMode::Prefix,
        "S" => SimplePatternMode::Suffix,
        "U" => SimplePatternMode::Substring,
        other => panic!("bad mode {other}"),
    }
}

fn result_code(result: SimplePatternResult) -> u8 {
    match result {
        SimplePatternResult::NotMatched => 0,
        SimplePatternResult::MatchedNegative => 1,
        SimplePatternResult::MatchedPositive => 2,
    }
}

fn compile(row: &Row) -> SimplePattern {
    SimplePattern::new(
        row.bytes(0),
        separators(row.bytes(1)),
        mode(row.str(2)),
        row.flag(3),
    )
}

#[test]
fn match_vectors() {
    check(
        "simple_pattern.tsv",
        |row| (row.num::<u8>(6), text(row.bytes(7))),
        |row| {
            let (result, wildcarded) = compile(row).matches_extract(row.bytes(4), row.num(5));
            (result_code(result), text(&wildcarded))
        },
    );
}

#[test]
fn info_vectors() {
    check(
        "simple_pattern_info.tsv",
        |row| (row.flag(4), row.flag(5), text(row.bytes(6))),
        |row| {
            let pattern = compile(row);
            let words: Vec<Vec<u8>> = pattern
                .words()
                .map(|w| match w {
                    Some(m) => [b"+".as_slice(), m].concat(),
                    None => b"-".to_vec(),
                })
                .collect();
            (
                contains_wildcards(row.bytes(0), separators(row.bytes(1))),
                pattern.is_potential_name(),
                text(&words.join(&0x1f)),
            )
        },
    );
}

/// The examples of the `-W simple-pattern` usage text (`daemon/main.c`),
/// which uses whitespace separators, exact matches and case sensitivity.
#[test]
fn c_simple_pattern_usage_examples() {
    let cases: &[(&str, &str, SimplePatternResult, &str)] = &[
        (
            "!veth0 veth*",
            "veth12",
            SimplePatternResult::MatchedPositive,
            "12",
        ),
        (
            "!veth0 veth*",
            "veth0",
            SimplePatternResult::MatchedNegative,
            "",
        ),
        ("!veth0 veth*", "eth0", SimplePatternResult::NotMatched, ""),
        (
            "!/path/*/*.ext /path/*.ext",
            "/path/test.ext",
            SimplePatternResult::MatchedPositive,
            "test",
        ),
        (
            "!/path/*/*.ext /path/*.ext",
            "/path/dir/test.ext",
            SimplePatternResult::MatchedNegative,
            "dirtest",
        ),
    ];
    for &(pattern, needle, result, wildcarded) in cases {
        let p = SimplePattern::new(
            pattern.as_bytes(),
            Separators::Whitespace,
            SimplePatternMode::Exact,
            true,
        );
        let (r, w) = p.matches_extract(needle.as_bytes(), needle.len() + 1);
        assert_eq!(
            (r, w.as_slice()),
            (result, wildcarded.as_bytes()),
            "{pattern:?} {needle:?}"
        );
    }
}

/// Only the first segment of a word honours case sensitivity (C sets the flag
/// on the root node only).
#[test]
fn case_sensitivity_applies_to_the_first_segment() {
    let p = SimplePattern::new(
        b"abc*def",
        Separators::Whitespace,
        SimplePatternMode::Exact,
        true,
    );
    let cases: &[(&str, bool)] = &[("abcXdef", true), ("abcXDEF", true), ("ABCXdef", false)];
    for &(s, expected) in cases {
        assert_eq!(p.matches(s.as_bytes()), expected, "{s:?}");
    }
}

/// `string_to_simple_pattern()` and its two kin give NULL for a text with no word in it (only separators, a lone
/// `!`, a lone `\`), as for an empty text and a lone `*`, and C's callers read NULL as "no pattern". A text with a
/// word beside the separators is a pattern.
#[test]
fn a_web_list_with_no_word_is_no_pattern() {
    type FromWeb = fn(&[u8]) -> Option<SimplePattern>;
    let constructors: [(&str, FromWeb); 3] = [
        ("exact", SimplePattern::from_web),
        ("nocase", SimplePattern::from_web_nocase),
        ("nocase substring", SimplePattern::from_web_nocase_substring),
    ];
    for (name, from_web) in constructors {
        for text in [&b""[..], b"*", b"|", b",", b"!", b"\\", b"|,|", b",\t\r\n"] {
            assert!(from_web(text).is_none(), "{name}: {:?}", String::from_utf8_lossy(text));
        }
        let pattern = from_web(b"|abc,").expect("a word between separators");
        assert!(pattern.matches(b"abc") && !pattern.matches(b"x"), "{name}");
    }
}
