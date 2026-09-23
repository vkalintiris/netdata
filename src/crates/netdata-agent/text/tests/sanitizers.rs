//! Sanitizers against the C golden vectors, plus the ported
//! `libnetdata/sanitizers/utf8-sanitizer-unittest.c`.

mod common;

use common::{Row, check, rows, text};
use netdata_agent_text::sanitize::{
    CHART_NAMES_ALLOWED_CHARS, CharMap, RRD_STRING_ALLOWED_CHARS, is_netdata_api_valid_character,
    netdata_fix_chart_name, nrpc_sanitize_name, prometheus_rrdlabels_sanitize_name,
    rrd_string_sanitize, rrdlabels_sanitize_name, rrdlabels_sanitize_value, rrdset_strncpyz_name,
    rrdvar_fix_name, text_sanitize,
};

fn identity_map() -> CharMap {
    std::array::from_fn(|i| i as u8)
}

/// `test_rrd_char_map` of the C unit test.
fn test_rrd_map() -> CharMap {
    let mut map: CharMap = std::array::from_fn(|i| match i {
        1..=31 | 127.. => b' ',
        _ => i as u8,
    });
    map[usize::from(b'"')] = b'\'';
    map[usize::from(b'\\')] = b'/';
    map
}

fn sanitize_row(row: &Row) -> (String, usize, String) {
    let input = row.bytes(1);
    let func = row.str(0);
    let size = || row.num::<usize>(2);
    let plain = |out: Vec<u8>| {
        let len = out.len();
        (text(&out), len, "-".to_string())
    };

    if let Some(spec) = func.strip_prefix("text_sanitize:") {
        let parts: Vec<&str> = spec.split(':').collect();
        let map = match parts[0] {
            "identity" => identity_map(),
            "rrd" => RRD_STRING_ALLOWED_CHARS,
            "chart" => CHART_NAMES_ALLOWED_CHARS,
            other => panic!("unknown map {other}"),
        };
        let empty: &[u8] = [b"".as_slice(), b"[none]", b"-"][parts[2].parse::<usize>().unwrap()];
        let out = text_sanitize(input, size(), &map, parts[1] == "1", empty);
        return (text(&out.text), out.text.len(), out.chars.to_string());
    }

    match func {
        "rrdlabels_sanitize_name" => plain(rrdlabels_sanitize_name(input, size())),
        "rrdlabels_sanitize_value" => plain(rrdlabels_sanitize_value(input, size())),
        "prometheus_rrdlabels_sanitize_name" => {
            plain(prometheus_rrdlabels_sanitize_name(input, size()))
        }
        "nrpc_sanitize_name" => plain(nrpc_sanitize_name(input, size())),
        "rrdset_strncpyz_name" => plain(rrdset_strncpyz_name(input, size() - 1)),
        "netdata_fix_chart_name" | "netdata_fix_chart_id" => plain(netdata_fix_chart_name(input)),
        "rrd_string_sanitize" => plain(rrd_string_sanitize(input)),
        "rrdvar_fix_name" => {
            let (out, changed) = rrdvar_fix_name(input);
            let len = out.len();
            (text(&out), len, u8::from(changed).to_string())
        }
        other => panic!("unknown function {other}"),
    }
}

#[test]
fn sanitize_vectors() {
    check(
        "sanitize.tsv",
        |row| {
            (
                text(row.bytes(3)),
                row.num::<usize>(4),
                row.str(5).to_string(),
            )
        },
        sanitize_row,
    );
}

#[test]
fn char_map_vectors() {
    let maps: Vec<(String, Vec<u8>)> = rows("char_maps.tsv")
        .iter()
        .map(|r| (r.str(0).to_string(), r.bytes(1).to_vec()))
        .collect();
    let valid: Vec<u8> = (0..=255u8)
        .map(|c| {
            if is_netdata_api_valid_character(c) {
                b'1'
            } else {
                b'0'
            }
        })
        .collect();
    let ours = vec![
        (
            "rrd_string_allowed_chars".to_string(),
            RRD_STRING_ALLOWED_CHARS.to_vec(),
        ),
        (
            "chart_names_allowed_chars".to_string(),
            CHART_NAMES_ALLOWED_CHARS.to_vec(),
        ),
        ("is_netdata_api_valid_character".to_string(), valid),
    ];
    assert_eq!(maps, ours);
}

// ------------------------------------------------------------------------------------------------
// utf8-sanitizer-unittest.c

#[derive(Debug, Clone, Copy)]
enum Map {
    Identity,
    TestRrd,
    RrdString,
}

impl Map {
    fn table(self) -> CharMap {
        match self {
            Map::Identity => identity_map(),
            Map::TestRrd => test_rrd_map(),
            Map::RrdString => RRD_STRING_ALLOWED_CHARS,
        }
    }
}

struct Case {
    name: &'static str,
    input: &'static [u8],
    dst_size: usize,
    map: Map,
    utf: bool,
    empty: &'static [u8],
    expected: &'static [u8],
    len: usize,
    /// checked only when non-zero, like the C test
    mblen: usize,
}

include!("vectors/utf8_unittest_cases.rs");

/// Every `sanitize_test_t` case of the C unit test (extracted by `tests/oracle/gen-vectors.sh`).
#[test]
fn c_utf8_sanitizer_cases() {
    for case in CASES {
        let out = text_sanitize(
            case.input,
            case.dst_size,
            &case.map.table(),
            case.utf,
            case.empty,
        );
        let mblen = if case.mblen > 0 { out.chars } else { 0 };
        assert_eq!(
            (out.text.as_slice(), out.text.len(), mblen),
            (case.expected, case.len, case.mblen),
            "{}",
            case.name
        );
    }
}

/// `buffer_size_0`, `null_input` (NULL is the empty input here) and the
/// `test_multibyte_length()` cases.
#[test]
fn c_utf8_sanitizer_direct_calls() {
    // (name, input, dst_size, empty, expected text, expected length, expected chars)
    type DirectCase = (
        &'static str,
        &'static [u8],
        usize,
        &'static [u8],
        &'static [u8],
        usize,
        usize,
    );
    let identity = identity_map();
    let cases: &[DirectCase] = &[
        ("buffer_size_0", b"hello", 0, b"", b"", 0, 0),
        ("null_input", b"", 32, b"null_val", b"null_val", 8, 8),
        ("mblen_ascii", b"hello", 32, b"", b"hello", 5, 5),
        ("mblen_2byte", b"\xC2\xB0", 32, b"", b"\xC2\xB0", 2, 1),
        (
            "mblen_3byte",
            b"\xE2\x82\xAC",
            32,
            b"",
            b"\xE2\x82\xAC",
            3,
            1,
        ),
        (
            "mblen_4byte",
            b"\xF0\x9F\x98\x80",
            32,
            b"",
            b"\xF0\x9F\x98\x80",
            4,
            1,
        ),
        (
            "mblen_mixed",
            b"A\xC2\xB0\xE2\x82\xAC\xF0\x9F\x98\x80",
            32,
            b"",
            b"A\xC2\xB0\xE2\x82\xAC\xF0\x9F\x98\x80",
            10,
            4,
        ),
        ("mblen_null_ptr", b"test", 32, b"", b"test", 4, 4),
    ];
    for &(name, input, dst_size, empty, expected, len, mblen) in cases {
        let out = text_sanitize(input, dst_size, &identity, true, empty);
        assert_eq!(
            (out.text.as_slice(), out.text.len(), out.chars),
            (expected, len, mblen),
            "{name}"
        );
    }
}

/// `test_all_control_characters()`: every control byte becomes one space.
#[test]
fn c_utf8_sanitizer_all_control_characters() {
    let map = test_rrd_map();
    for ctrl in 1u8..32 {
        let out = text_sanitize(&[b'A', ctrl, b'B'], 32, &map, true, b"");
        assert_eq!(out.text, b"A B", "ctrl 0x{ctrl:02X}");
    }
}

/// One `rrd_string_strdupz()` pass: `dst_size = 2 * len + 1`.
fn sanitize_once(src: &[u8]) -> Vec<u8> {
    text_sanitize(src, src.len() * 2 + 1, &RRD_STRING_ALLOWED_CHARS, true, b"").text
}

/// `test_rrd_string_allowed_chars_idempotency()`: sanitize(sanitize(x)) == sanitize(x).
#[test]
fn c_rrd_string_allowed_chars_idempotency() {
    let inputs: &[&[u8]] = &[
        b"cpu.user",
        b"\"value\"",
        b"path\\file",
        b"requests/s\xC2\xB2",
        b"\xC2\xB5s",
        b"\xF0\x9F\x98\x80",
        b"bad\xFFbyte",
        b"trunc\xC2",
        b"trunc\xE2\x82",
        b"\x80\x81lone",
        b"___",
        b"  a   b  ",
        b"a\x01\x02b",
        b"test\xC0\x80test",
        b"\xC2\xB0C \xFF A",
        b"",
    ];
    let mut failures = Vec::new();
    let mut probe = |src: &[u8]| {
        let once = sanitize_once(src);
        let twice = sanitize_once(&once);
        if once != twice {
            failures.push((src.to_vec(), once, twice));
        }
    };

    for input in inputs {
        probe(input);
    }
    // exhaustive over every 1- and 2-byte string
    for i in 1..=255u8 {
        probe(&[i]);
        for j in 1..=255u8 {
            probe(&[i, j]);
        }
    }
    // the deterministic random sweep of the C test
    let mut seed: u32 = 0x5eed1234;
    for _ in 0..100_000 {
        seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
        let len = 1 + (seed >> 16) as usize % 24;
        let mut src = Vec::with_capacity(len);
        for _ in 0..len {
            seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
            src.push(((seed >> 16) & 0xFF).max(1) as u8);
        }
        probe(&src);
    }

    assert!(
        failures.is_empty(),
        "not idempotent: {:?}",
        &failures[..failures.len().min(10)]
    );
}

/// `test_stress_and_edge_cases()` checks that are not table rows.
#[test]
fn c_utf8_sanitizer_stress() {
    let identity = identity_map();

    let all_bytes: Vec<u8> = (1..=255u8).collect();
    assert!(
        !text_sanitize(&all_bytes, 512, &identity, false, b"")
            .text
            .is_empty(),
        "stress_all_bytes"
    );

    let input = b"test\xC2\xB0\xE2\x82\xAC\xF0\x9F\x98\x80";
    for size in 1..=20 {
        assert!(
            text_sanitize(input, size, &identity, true, b"").text.len() < size,
            "stress_buffer_sizes {size}"
        );
    }

    let once = text_sanitize(b"test\xC2\xB0C", 32, &identity, true, b"").text;
    let twice = text_sanitize(&once, 32, &identity, true, b"").text;
    assert_eq!(once, twice, "stress_idempotent");
}
