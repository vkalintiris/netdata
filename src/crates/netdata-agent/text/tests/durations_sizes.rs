//! Durations, sizes and entry counts against the C golden vectors, plus the
//! ported `libnetdata/parsers/duration-unittest.c`.

mod common;

use common::{Row, check};
use netdata_agent_text::duration::{
    duration_parse, duration_parse_seconds, duration_round_to_resolution, duration_to_string,
};
use netdata_agent_text::size::{entries_parse, entries_to_string, size_parse, size_to_string};

/// C leaves 0 in `*result` on failure.
fn parsed<T: Default + ToString>(value: Option<T>) -> (bool, String) {
    (value.is_some(), value.unwrap_or_default().to_string())
}

/// `(return, text)` of a C `*_snprintf()` into a large buffer.
fn rendered(text: Option<String>, error: i64) -> (i64, String) {
    match text {
        Some(text) => (text.len() as i64, text),
        None => (error, String::new()),
    }
}

fn expected_parse(row: &Row, ok: usize) -> (bool, String) {
    (row.flag(ok), row.str(ok + 1).to_string())
}

fn expected_render(row: &Row) -> (i64, String) {
    (row.num(3), row.str(4).to_string())
}

#[test]
fn duration_parse_vectors() {
    check(
        "duration_parse.tsv",
        |row| expected_parse(row, 3),
        |row| parsed(duration_parse(row.bytes(0), row.str(1), row.str(2))),
    );
}

#[test]
fn duration_parse_seconds_vectors() {
    check(
        "duration_seconds.tsv",
        |row| {
            // C leaves the destination untouched on failure; the generator seeds it with 12345
            let (ok, value) = expected_parse(row, 1);
            (ok, if ok { value } else { "12345".to_string() })
        },
        |row| {
            let v = duration_parse_seconds(row.bytes(0));
            (v.is_some(), v.unwrap_or(12345).to_string())
        },
    );
}

#[test]
fn duration_format_vectors() {
    check("duration_format.tsv", expected_render, |row| {
        rendered(duration_to_string(row.num(0), row.str(1), row.flag(2)), -3)
    });
}

#[test]
fn duration_round_vectors() {
    check(
        "duration_round.tsv",
        |row| row.num::<i64>(2),
        |row| duration_round_to_resolution(row.num(0), row.num(1)),
    );
}

#[test]
fn size_parse_vectors() {
    check(
        "size_parse.tsv",
        |row| expected_parse(row, 2),
        |row| parsed(size_parse(row.bytes(0), row.str(1))),
    );
}

#[test]
fn size_format_vectors() {
    check("size_format.tsv", expected_render, |row| {
        rendered(size_to_string(row.num(0), row.str(1), row.flag(2)), -3)
    });
}

#[test]
fn entries_parse_vectors() {
    check(
        "entries_parse.tsv",
        |row| expected_parse(row, 2),
        |row| parsed(entries_parse(row.bytes(0), row.str(1))),
    );
}

#[test]
fn entries_format_vectors() {
    check("entries_format.tsv", expected_render, |row| {
        rendered(entries_to_string(row.num(0), row.str(1), row.flag(2)), -3)
    });
}

// ------------------------------------------------------------------------------------------------
// duration-unittest.c

/// (input, default unit, output unit, expected value, should succeed, expected reformat).
type ParseCase = (
    &'static str,
    &'static str,
    &'static str,
    i64,
    bool,
    Option<&'static str>,
);

/// `test_cases[]`.
const TEST_CASES: &[ParseCase] = &[
    ("5m", "s", "s", 300, true, Some("5m")),
    ("2h", "s", "s", 7200, true, Some("2h")),
    ("7d", "s", "s", 604800, true, Some("7d")),
    ("1w", "s", "d", 7, true, Some("7d")),
    ("30s", "s", "s", 30, true, Some("30s")),
    ("7 days", "s", "s", 604800, true, Some("7d")),
    ("2 hours", "s", "s", 7200, true, Some("2h")),
    ("30 seconds", "s", "s", 30, true, Some("30s")),
    ("5 minutes", "s", "s", 300, true, Some("5m")),
    ("1 week", "s", "d", 7, true, Some("7d")),
    ("2 months", "s", "d", 60, true, Some("2mo")),
    ("1 year", "s", "d", 365, true, Some("1y")),
    ("7 DAYS", "s", "s", 604800, true, Some("7d")),
    ("2 Hours", "s", "s", 7200, true, Some("2h")),
    ("30 SECONDS", "s", "s", 30, true, Some("30s")),
    ("5 Minutes", "s", "s", 300, true, Some("5m")),
    ("7days", "s", "s", 604800, true, Some("7d")),
    ("2hours", "s", "s", 7200, true, Some("2h")),
    ("30seconds", "s", "s", 30, true, Some("30s")),
    ("5minutes", "s", "s", 300, true, Some("5m")),
    ("1 day", "s", "s", 86400, true, Some("1d")),
    ("1 hour", "s", "s", 3600, true, Some("1h")),
    ("1 second", "s", "s", 1, true, Some("1s")),
    ("1 minute", "s", "s", 60, true, Some("1m")),
    ("1 week", "s", "d", 7, true, Some("7d")),
    ("1 month", "s", "d", 30, true, Some("1mo")),
    ("1 year", "s", "d", 365, true, Some("1y")),
    ("2 hours 30 minutes", "s", "s", 9000, true, Some("2h30m")),
    ("1 day 12 hours", "s", "s", 129600, true, Some("1d12h")),
    ("1 week 2 days", "s", "d", 9, true, Some("9d")),
    (
        "1 year 2 months 3 days",
        "s",
        "d",
        428,
        true,
        Some("1y2mo3d"),
    ),
    ("2h 30 minutes", "s", "s", 9000, true, Some("2h30m")),
    ("1d 12 hours", "s", "s", 129600, true, Some("1d12h")),
    ("1 week 2d", "s", "d", 9, true, Some("9d")),
    ("100 milliseconds", "s", "ms", 100, true, Some("100ms")),
    ("50 microseconds", "s", "us", 50, true, Some("50us")),
    ("25 nanoseconds", "s", "ns", 25, true, Some("25ns")),
    ("100 MILLISECONDS", "s", "ms", 100, true, Some("100ms")),
    ("50 Microseconds", "s", "us", 50, true, Some("50us")),
    ("1.5 days", "s", "h", 36, true, Some("1d12h")),
    ("2.5 hours", "s", "m", 150, true, Some("2h30m")),
    ("0.5 minutes", "s", "s", 30, true, Some("30s")),
    ("30 sec", "s", "s", 30, true, Some("30s")),
    ("30 secs", "s", "s", 30, true, Some("30s")),
    ("2 hr", "s", "m", 120, true, Some("2h")),
    ("2 hrs", "s", "m", 120, true, Some("2h")),
    ("never", "s", "s", 0, true, Some("off")),
    ("NEVER", "s", "s", 0, true, Some("off")),
    ("Never", "s", "s", 0, true, Some("off")),
    ("off", "s", "s", 0, true, Some("off")),
    ("OFF", "s", "s", 0, true, Some("off")),
    ("Off", "s", "s", 0, true, Some("off")),
    ("-5 minutes", "s", "s", -300, true, Some("-5m")),
    ("-2 hours", "s", "s", -7200, true, Some("-2h")),
    ("-1 day", "s", "s", -86400, true, Some("-1d")),
    ("7 days ago", "s", "s", -604800, true, Some("-7d")),
    ("7d ago", "s", "s", -604800, true, Some("-7d")),
    ("2 hours ago", "s", "s", -7200, true, Some("-2h")),
    ("2h ago", "s", "s", -7200, true, Some("-2h")),
    ("30 minutes ago", "s", "s", -1800, true, Some("-30m")),
    ("30m ago", "s", "s", -1800, true, Some("-30m")),
    ("1 year ago", "s", "d", -365, true, Some("-1y")),
    ("1y ago", "s", "d", -365, true, Some("-1y")),
    (
        "2 hours 30 minutes ago",
        "s",
        "s",
        -9000,
        true,
        Some("-2h30m"),
    ),
    ("2h30m ago", "s", "s", -9000, true, Some("-2h30m")),
    (
        "1 day 12 hours ago",
        "s",
        "s",
        -129600,
        true,
        Some("-1d12h"),
    ),
    ("1d12h ago", "s", "s", -129600, true, Some("-1d12h")),
    ("7 days AGO", "s", "s", -604800, true, Some("-7d")),
    ("7 days Ago", "s", "s", -604800, true, Some("-7d")),
    ("7daysago", "s", "s", -604800, true, Some("-7d")),
    ("-7 days ago", "s", "s", -604800, true, Some("-7d")),
    ("-2h ago", "s", "s", -7200, true, Some("-2h")),
    ("invalid", "s", "s", 0, false, None),
    ("5 invalidunit", "s", "s", 0, false, None),
    ("abc days", "s", "s", 0, false, None),
    ("", "s", "s", 0, false, None),
    ("7 days ago extra", "s", "s", 0, false, None),
    ("7 days agooo", "s", "s", 0, false, None),
    ("ago", "s", "s", 0, false, None),
    ("7 ago days", "s", "s", 0, false, None),
    ("7 days ago 1 hour", "s", "s", 0, false, None),
    ("7d ago 1h", "s", "s", 0, false, None),
    ("-7d+1h", "s", "s", -608400, true, Some("-7d1h")),
    ("1d-12h", "s", "s", 43200, true, Some("12h")),
    ("2h-3h", "s", "s", -3600, true, Some("-1h")),
    ("3661s", "s", "s", 3661, true, Some("1h1m1s")),
    ("90000s", "s", "s", 90000, true, Some("1d1h")),
    ("31536000s", "s", "s", 31536000, true, Some("1y")),
    ("366d", "s", "d", 366, true, Some("1y1d")),
    ("100000000ns", "s", "ns", 100000000, true, Some("100ms")),
    ("3600000ms", "s", "ms", 3600000, true, Some("1h")),
    ("0.001s", "s", "ms", 1, true, Some("1ms")),
    ("1440m", "s", "h", 24, true, Some("1d")),
    ("10080m", "s", "d", 7, true, Some("7d")),
    ("0s", "s", "s", 0, true, Some("off")),
    ("0d", "s", "d", 0, true, Some("off")),
    ("60", "s", "s", 60, true, Some("1m")),
    ("3600", "s", "s", 3600, true, Some("1h")),
    ("86400", "s", "s", 86400, true, Some("1d")),
    ("7", "d", "d", 7, true, Some("7d")),
    ("24", "h", "h", 24, true, Some("1d")),
    ("60", "m", "m", 60, true, Some("1h")),
    ("1000", "ms", "ms", 1000, true, Some("1s")),
    ("1000000", "us", "us", 1000000, true, Some("1s")),
    ("1000000000", "ns", "ns", 1000000000, true, Some("1s")),
    ("-60", "s", "s", -60, true, Some("-1m")),
    ("-3600", "s", "s", -3600, true, Some("-1h")),
    ("-86400", "s", "s", -86400, true, Some("-1d")),
    ("-7", "d", "d", -7, true, Some("-7d")),
    ("-24", "h", "h", -24, true, Some("-1d")),
    ("-60", "m", "m", -60, true, Some("-1h")),
    ("1.5", "d", "d", 2, true, Some("2d")),
    ("2.5", "h", "h", 3, true, Some("3h")),
    ("0.5", "m", "m", 1, true, Some("1m")),
    ("1.5", "s", "s", 2, true, Some("2s")),
    ("-1.5", "h", "h", -2, true, Some("-2h")),
    ("0", "s", "s", 0, true, Some("off")),
    ("-0", "s", "s", 0, true, Some("off")),
    ("+60", "s", "s", 60, true, Some("1m")),
    (" 60 ", "s", "s", 60, true, Some("1m")),
    ("1705318200", "s", "s", 1705318200, true, None),
    ("1609459200", "s", "s", 1609459200, true, None),
    ("946684800", "s", "s", 946684800, true, None),
    ("0", "s", "s", 0, true, Some("off")),
    ("-86400", "s", "s", -86400, true, Some("-1d")),
    ("2147483647", "s", "s", 2147483647, true, None),
    ("4102444800", "s", "s", 4102444800, true, None),
    ("1705318200s", "s", "s", 1705318200, true, None),
    ("1705318200 seconds", "s", "s", 1705318200, true, None),
];

#[test]
fn c_duration_parsing_tests() {
    for &(input, default_unit, output_unit, value, succeed, reformat) in TEST_CASES {
        let result = duration_parse(input.as_bytes(), default_unit, output_unit);
        let expected = if succeed { Some(value) } else { None };
        let text = result.and_then(|v| reformat.and(duration_to_string(v, output_unit, false)));
        assert_eq!(
            (result, text.as_deref()),
            (expected, reformat.filter(|_| succeed)),
            "{input:?}"
        );
    }
}

#[test]
fn c_duration_generation_tests() {
    let cases: &[(i64, &str, &str)] = &[
        (300, "s", "5m"),
        (7200, "s", "2h"),
        (86400, "s", "1d"),
        (604800, "s", "7d"),
        (2592000, "s", "1mo"),
        (31536000, "s", "1y"),
        (9000, "s", "2h30m"),
        (129600, "s", "1d12h"),
        (i64::MAX, "y", "9223372036854775807y"),
        (2372, "q", "584y4q"),
        (0, "s", "off"),
        (-300, "s", "-5m"),
        (i64::MIN, "y", "-9223372036854775808y"),
        (i64::MIN, "ns", "-292y5mo21d23h47m16s854ms775us808ns"),
    ];
    for &(value, unit, expected) in cases {
        assert_eq!(
            duration_to_string(value, unit, false).as_deref(),
            Some(expected),
            "{value} {unit}"
        );
    }
}

#[test]
fn c_duration_roundtrip_tests() {
    let cases: &[(&str, &str)] = &[
        ("7d", "7d"),
        ("7 days", "7d"),
        ("2h30m", "2h30m"),
        ("2 hours 30 minutes", "2h30m"),
        ("1d-12h", "12h"),
        ("2h-3h", "-1h"),
        ("1d12h", "1d12h"),
        ("-7d", "-7d"),
        ("-2h30m", "-2h30m"),
        ("7 days ago", "-7d"),
        ("2h30m ago", "-2h30m"),
        ("-7 days ago", "-7d"),
        ("0s", "off"),
        ("never", "off"),
        ("off", "off"),
    ];
    for &(input, expected) in cases {
        let value = duration_parse(input.as_bytes(), "s", "s").expect(input);
        let text = duration_to_string(value, "s", false).expect(input);
        let again = duration_parse(text.as_bytes(), "s", "s");
        assert_eq!((text.as_str(), again), (expected, Some(value)), "{input:?}");
    }
}

#[test]
fn c_duration_parse_seconds_int_range() {
    let cases: &[(&str, Option<i32>)] = &[
        ("2147483647s", Some(i32::MAX)),
        ("-2147483648s", Some(i32::MIN)),
        ("2147483648s", None),
        ("-2147483649s", None),
    ];
    for &(input, expected) in cases {
        assert_eq!(
            duration_parse_seconds(input.as_bytes()),
            expected,
            "{input:?}"
        );
    }
}

#[test]
fn c_parser_fixed_width_conversions() {
    let durations: &[(&str, &str, &str, Option<i64>)] = &[
        ("nan", "ns", "ns", None),
        ("inf", "ns", "ns", None),
        ("-inf", "ns", "ns", None),
        ("1e400ns", "ns", "ns", None),
        ("1e300y", "ns", "ns", None),
        ("9223372036854775808ns", "ns", "ns", None),
        ("-9223372036854775808ns", "ns", "ns", None),
        ("0ns-9223372036854775808ns", "ns", "ns", None),
        ("0ns-9223372036854775808ns1ns", "ns", "ns", Some(-i64::MAX)),
        ("0ns-9223372036854775808ns-1ns", "ns", "ns", None),
        ("9223372036854774784ns1024ns", "ns", "ns", None),
        ("0ns-9223372036854774784ns-1024ns", "ns", "ns", None),
        (
            "9223372036854774784ns",
            "ns",
            "ns",
            Some(9223372036854774784),
        ),
        (
            "-9223372036854774784ns",
            "ns",
            "ns",
            Some(-9223372036854774784),
        ),
        (
            "9223372036854774784ns ago",
            "ns",
            "ns",
            Some(-9223372036854774784),
        ),
        (
            "9223372036854774784ns-1024ns",
            "ns",
            "ns",
            Some(9223372036854773760),
        ),
        ("1.5s", "s", "s", Some(2)),
    ];
    for &(input, default_unit, output_unit, expected) in durations {
        assert_eq!(
            duration_parse(input.as_bytes(), default_unit, output_unit),
            expected,
            "{input:?}"
        );
    }

    let sizes: &[(&str, &str, Option<u64>)] = &[
        ("nanB", "B", None),
        ("infB", "B", None),
        ("1e400B", "B", None),
        ("0x1p64B", "B", None),
        ("18446744073709551616B", "B", None),
        ("18446744073709560KB", "B", None),
        ("17592186044416", "MiB", None),
        ("-1B", "B", None),
        ("18446744073709549568B", "B", Some(18446744073709549568)),
        ("1536B", "KiB", Some(2)),
        ("1535B", "KiB", Some(1)),
        // the representable uint64 maximum rounds up to 2^64 as a double
        ("18446744073709551615B", "B", None),
    ];
    for &(input, default_unit, expected) in sizes {
        assert_eq!(
            size_parse(input.as_bytes(), default_unit),
            expected,
            "{input:?}"
        );
    }

    let entries: &[(&str, &str, Option<u64>)] = &[
        ("nan", "", None),
        ("inf", "", None),
        ("1e400", "", None),
        ("18446744073709551616", "", None),
        ("18446744074G", "", None),
        ("-1", "", None),
        ("18446744073709549568", "", Some(18446744073709549568)),
        ("1.5K", "K", Some(2)),
        ("18446744073709551615", "", None),
    ];
    for &(input, default_unit, expected) in entries {
        assert_eq!(
            entries_parse(input.as_bytes(), default_unit),
            expected,
            "{input:?}"
        );
    }
}

/// Units are C strings: they end at a NUL byte (outputs from the C tree).
#[test]
fn c_units_end_at_nul() {
    assert_eq!(
        (
            duration_parse(b"5", "s\0x", "s"),
            duration_to_string(5, "s\0x", false),
            size_parse(b"5", "B\0x"),
            size_to_string(5, "KiB\0x", true),
        ),
        (
            Some(5),
            Some("5s".to_string()),
            Some(5),
            Some("5KiB".to_string())
        )
    );
}
