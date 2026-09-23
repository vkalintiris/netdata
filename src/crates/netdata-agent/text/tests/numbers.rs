//! Number printing and parsing against the C golden vectors, plus the ported
//! C unit tests (`buffer_unittest()`, `check_number_printing()`,
//! `unit_test_str2ld()`, `storage_number/tests/test_storage_number.c`).

mod common;

use common::{Row, check, text};
use netdata_agent_text::parse::{
    str2i, str2l, str2ll, str2ll_encoded, str2ndd, str2ndd_encoded, str2u, str2uint32,
    str2uint32_hex, str2uint64, str2uint64_base64, str2uint64_hex, str2ul, str2ull_encoded, strtod,
};
use netdata_agent_text::print::{
    NumberEncoding, netdata_double_to_string, print_int64_encoded, print_netdata_double,
    print_netdata_double_encoded, print_uint64_encoded, print_uint64_hex_full,
};

fn printed(f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::new();
    f(&mut out);
    out
}

fn expected_fields(row: &Row) -> Vec<Vec<u8>> {
    row.fields[1..].to_vec()
}

#[test]
fn print_double_vectors() {
    check("print_double.tsv", expected_fields, |row| {
        let v = f64::from_bits(row.bits(0));
        vec![
            printed(|o| print_netdata_double(o, v)),
            printed(|o| print_netdata_double_encoded(o, NumberEncoding::Decimal, v)),
            printed(|o| print_netdata_double_encoded(o, NumberEncoding::Hex, v)),
            printed(|o| print_netdata_double_encoded(o, NumberEncoding::Base64, v)),
        ]
    });
}

#[test]
fn print_uint64_vectors() {
    check("print_uint64.tsv", expected_fields, |row| {
        let v: u64 = row.num(0);
        vec![
            printed(|o| print_uint64_encoded(o, NumberEncoding::Decimal, v)),
            printed(|o| print_uint64_encoded(o, NumberEncoding::Hex, v)),
            printed(|o| print_uint64_hex_full(o, v)),
            printed(|o| print_uint64_encoded(o, NumberEncoding::Base64, v)),
        ]
    });
}

#[test]
fn print_int64_vectors() {
    check("print_int64.tsv", expected_fields, |row| {
        let v: i64 = row.num(0);
        vec![
            printed(|o| print_int64_encoded(o, NumberEncoding::Decimal, v)),
            printed(|o| print_int64_encoded(o, NumberEncoding::Hex, v)),
            printed(|o| print_int64_encoded(o, NumberEncoding::Base64, v)),
        ]
    });
}

#[test]
fn parse_number_vectors() {
    check(
        "parse_numbers.tsv",
        |row| row.fields[1..].iter().map(|f| text(f)).collect::<Vec<_>>(),
        |row| {
            let s = row.bytes(0);
            let double = |(v, n): (f64, usize)| [format!("{:016x}", v.to_bits()), n.to_string()];
            let int = |(v, n): (u64, usize)| [v.to_string(), n.to_string()];
            let mut out = Vec::new();
            out.extend(double(str2ndd(s)));
            out.extend(double(str2ndd_encoded(s)));
            out.extend(double(strtod(s)));
            out.extend(int(str2uint64(s)));
            let (v, n) = str2uint32(s);
            out.extend([v.to_string(), n.to_string()]);
            let (v, n) = str2ll(s);
            out.extend([v.to_string(), n.to_string()]);
            out.extend([
                str2u(s).to_string(),
                str2i(s).to_string(),
                str2ul(s).to_string(),
                str2l(s).to_string(),
            ]);
            out.extend([
                str2ull_encoded(s).to_string(),
                str2ll_encoded(s).to_string(),
            ]);
            out.extend(int(str2uint64_hex(s)));
            let (v, n) = str2uint32_hex(s);
            out.extend([v.to_string(), n.to_string()]);
            out.extend(int(str2uint64_base64(s)));
            out
        },
    );
}

/// `check_number_printing()` in `daemon/unit_test.c` and
/// `test_number_printing()` in `storage_number/tests/test_storage_number.c`
/// (without its storage-number round trip, which belongs to the storage crate).
#[test]
#[allow(clippy::excessive_precision)] // the literals are copied verbatim from the C table
fn c_check_number_printing() {
    let cases: &[(f64, &str)] = &[
        (0.0, "0"),
        (0.0000001, "0.0000001"),
        (0.00000009, "0.0000001"),
        (0.000000001, "0"),
        (99.999_999_999_999_999_99, "100"),
        (-99.999_999_999_999_999_99, "-100"),
        (123.456_789_912_345_678_9, "123.4567899"),
        (123.456_789_012_345_678_9, "123.456789"),
        (123.456_780_012_345_678_9, "123.45678"),
        (123.456_700_012_345_678_9, "123.4567"),
        (123.456_000_012_345_678_9, "123.456"),
        (123.450_000_012_345_678_9, "123.45"),
        (123.400_000_012_345_678_9, "123.4"),
        (123.000_000_012_345_678_9, "123"),
        (123.000_000_092_345_678_9, "123.0000001"),
        (4_294_967_295.123_456_789, "4294967295.123457"),
        (8_294_967_295.123_456_789, "8294967295.123457"),
        (1.000000000000002e+19, "1.000000000000001998e+19"),
        (9.2233720368547676e+18, "9.223372036854767584e+18"),
        (18_446_744_073_709_541_376.0, "1.84467440737095424e+19"),
        (18_446_744_073_709_551_616.0, "1.844674407370955136e+19"),
        (12_318_446_744_073_710_600_192.0, "1.231844674407371008e+22"),
        (1_677_721_499_999_999_885_312.0, "1.677721499999999872e+21"),
        (
            -1_677_721_499_999_999_885_312.0,
            "-1.677721499999999872e+21",
        ),
        (-1.677721499999999885312e40, "-1.677721499999999872e+40"),
        (
            -16_777_214_999_999_997_337_621_690_403_742_592_008_192.0,
            "-1.677721499999999616e+40",
        ),
        (9999.9999999, "9999.9999999"),
        (-9999.9999999, "-9999.9999999"),
    ];
    for &(n, correct) in cases {
        let printed = netdata_double_to_string(n);
        let parsed_netdata = str2ndd(printed.as_bytes()).0;
        let parsed_system = strtod(printed.as_bytes()).0;
        assert_eq!(
            (printed.as_str(), parsed_netdata),
            (correct, parsed_system),
            "value {n:e}"
        );
    }
}

/// `unit_test_str2ld()` in `daemon/unit_test.c`: `str2ndd()` agrees with the
/// system parser on the value (within 1e-6) and the end pointer.
#[test]
fn c_unit_test_str2ld() {
    let values = [
        "1.2345678",
        "-35.6",
        "0.00123",
        "23842384234234.2",
        ".1",
        "1.2e-10",
        "18446744073709551616.0",
        "18446744073709551616123456789123456789123456789123456789123456789123456789123456789.0",
        "1.8446744073709551616123456789123456789123456789123456789123456789123456789123456789e+300",
        "9.",
        "9.e2",
        "1.2e",
        "1.2e+",
        "1.2e-",
        "1.2e0",
        "1.2e-0",
        "1.2e+0",
        "-1.2e+1",
        "-1.2e-1",
        "1.2e1",
        "1.2e400",
        "hello",
        "1wrong",
        "nan",
        "inf",
    ];
    for value in values {
        let (mine, mine_end) = str2ndd(value.as_bytes());
        let (sys, sys_end) = strtod(value.as_bytes());
        let same = if mine.is_nan() {
            sys.is_nan()
        } else if mine.is_infinite() {
            sys.is_infinite()
        } else {
            mine == sys || (mine - sys).abs() <= 0.000001
        };
        assert_eq!(
            (same, mine_end),
            (true, sys_end),
            "{value}: {mine} vs {sys}"
        );
    }
}

fn encodings() -> [NumberEncoding; 3] {
    [
        NumberEncoding::Decimal,
        NumberEncoding::Hex,
        NumberEncoding::Base64,
    ]
}

/// The round trips of `buffer_unittest()` in `libnetdata/buffer/buffer.c`.
#[test]
#[allow(clippy::excessive_precision)] // the literals are copied verbatim from the C test
fn c_buffer_unittest_roundtrips() {
    let unsigned: &[(u64, [&str; 3])] = &[
        (0, ["0", "0x0", "#A"]),
        (1676071986, ["1676071986", "0x63E6D432", "#Bj5tQy"]),
        (
            u64::MAX,
            ["18446744073709551615", "0xFFFFFFFFFFFFFFFF", "#P//////////"],
        ),
    ];
    for &(value, expected) in unsigned {
        for (encoding, expected) in encodings().into_iter().zip(expected) {
            let text = printed(|o| print_uint64_encoded(o, encoding, value));
            assert_eq!(
                (text.as_slice(), str2ull_encoded(&text)),
                (expected.as_bytes(), value)
            );
        }
    }

    let signed: &[(i64, [&str; 3])] = &[
        (0, ["0", "0x0", "#A"]),
        (-1676071986, ["-1676071986", "-0x63E6D432", "-#Bj5tQy"]),
        (
            -9223372036854775807,
            [
                "-9223372036854775807",
                "-0x7FFFFFFFFFFFFFFF",
                "-#H//////////",
            ],
        ),
        (
            i64::MIN,
            [
                "-9223372036854775808",
                "-0x8000000000000000",
                "-#IAAAAAAAAAA",
            ],
        ),
    ];
    for &(value, expected) in signed {
        for (encoding, expected) in encodings().into_iter().zip(expected) {
            let text = printed(|o| print_int64_encoded(o, encoding, value));
            assert_eq!(
                (text.as_slice(), str2ll_encoded(&text)),
                (expected.as_bytes(), value)
            );
        }
    }

    let doubles: &[(f64, [&str; 3])] = &[
        (0.0, ["0", "%0", "@A"]),
        (1.5, ["1.5", "%3FF8000000000000", "@D/4AAAAAAAA"]),
        (
            1.23e+14,
            ["123000000000000", "%42DBF78AD3AC0000", "@ELb94rTrAAA"],
        ),
        (
            9.12345678901234567890123456789e+45,
            [
                "9.123456789012346128e+45",
                "%497991C25C9E4309",
                "@El5kcJcnkMJ",
            ],
        ),
    ];
    for &(value, expected) in doubles {
        for (encoding, expected) in encodings().into_iter().zip(expected) {
            let text = printed(|o| print_netdata_double_encoded(o, encoding, value));
            // C only checks the parse-back of the exact encodings (the decimal one loses precision)
            let parsed = str2ndd_encoded(&text).0;
            let exact = encoding != NumberEncoding::Decimal
                || value == 0.0
                || value == 1.5
                || value == 1.23e+14;
            assert_eq!(
                (text.as_slice(), !exact || parsed == value),
                (expected.as_bytes(), true)
            );
        }
    }
}

/// The `int64_parse_tests[]` table of `buffer_unittest()`.
#[test]
fn c_buffer_unittest_int64_parse() {
    let cases: &[(&str, i64)] = &[
        ("0", 0),
        ("-0", 0),
        ("9223372036854775807", i64::MAX),
        ("0x7FFFFFFFFFFFFFFF", i64::MAX),
        ("#H//////////", i64::MAX),
        ("-9223372036854775808", i64::MIN),
        ("-0x8000000000000000", i64::MIN),
        ("-#IAAAAAAAAAA", i64::MIN),
        ("9223372036854775808", i64::MIN),
        ("0x8000000000000000", i64::MIN),
        ("#IAAAAAAAAAA", i64::MIN),
        ("-9223372036854775809", i64::MAX),
        ("-0x8000000000000001", i64::MAX),
        ("-#IAAAAAAAAAB", i64::MAX),
        ("18446744073709551615", -1),
        ("-18446744073709551615", 1),
        ("18446744073709551616", 0),
        ("0x10000000000000000", 0),
        ("#QAAAAAAAAAA", 0),
        (" 9223372036854775808", i64::MIN),
        (" -9223372036854775808", 0),
        ("- 9223372036854775808", i64::MIN),
        (" 0x8000000000000000", 0),
        ("- 0x8000000000000000", 0),
        ("-9223372036854775808suffix", i64::MIN),
        ("-0x8000000000000000!suffix", i64::MIN),
        ("-#IAAAAAAAAAA!suffix", i64::MIN),
        ("", 0),
        ("-", 0),
        ("+1", 0),
        ("--1", 0),
        ("#", 0),
        ("0x", 0),
        ("-#", 0),
        ("-0x", 0),
    ];
    for &(encoded, expected) in cases {
        assert_eq!(str2ll_encoded(encoded.as_bytes()), expected, "{encoded:?}");
    }
}

#[test]
fn str2ndd_quirks() {
    let cases: &[(&str, u64, usize)] = &[
        ("null", f64::NAN.to_bits(), 3),
        ("-", (-0.0f64).to_bits(), 1),
        (".", 0, 1),
        ("- 5.5", (-5.0f64).to_bits(), 2),
        ("1. 5", 1.5f64.to_bits(), 3),
        ("1e 5", 1e5f64.to_bits(), 3),
        ("-nan", (-0.0f64).to_bits(), 1),
    ];
    for &(input, bits, used) in cases {
        let (v, n) = str2ndd(input.as_bytes());
        assert_eq!((v.to_bits(), n), (bits, used), "{input:?}");
    }
}

/// Inputs too long for the vector files (up to 800 KB): exponents that std's
/// parser caps, cancelled by as many digits. Expected values were produced by
/// glibc `strtod()` and `size_parse(..., "B")` from the C tree.
#[test]
fn c_strtod_long_inputs() {
    let build = |parts: &[(&str, usize)]| -> Vec<u8> {
        parts
            .iter()
            .flat_map(|&(text, repeat)| text.repeat(repeat).into_bytes())
            .collect()
    };
    // (input, f64 bits, bytes used, size_parse(B))
    let cases: Vec<(Vec<u8>, u64, usize, Option<u64>)> = vec![
        (
            build(&[("1", 1), ("0", 655360), ("e-655360", 1)]),
            0x3ff0000000000000,
            655369,
            Some(1),
        ),
        (
            build(&[("0.", 1), ("0", 655359), ("1e655360", 1)]),
            0x3ff0000000000000,
            655369,
            Some(1),
        ),
        (
            build(&[("0.", 1), ("0", 700000), ("15e700001", 1)]),
            0x3ff8000000000000,
            700011,
            Some(2),
        ),
        (
            build(&[
                ("123", 1),
                ("7", 800000),
                (".", 1),
                ("9", 5),
                ("e-800002", 1),
            ]),
            0x3ff3cdf012345679,
            800017,
            Some(1),
        ),
        (
            build(&[("0.", 1), ("0", 5), ("9", 400000), ("e-400000", 1)]),
            0,
            400015,
            Some(0),
        ),
        (build(&[("1e-99999999999999999999", 1)]), 0, 23, Some(0)),
        (
            build(&[("0.", 1), ("0", 330), ("24703282292062328e0", 1)]),
            0,
            351,
            Some(0),
        ),
        (
            build(&[("0.", 1), ("0", 330), ("24703282292062327e0", 1)]),
            0,
            351,
            Some(0),
        ),
        (
            build(&[("1.7976931348623157e308", 1)]),
            0x7fefffffffffffff,
            22,
            None,
        ),
        (build(&[("0", 400), ("1e-400", 1)]), 0, 406, Some(0)),
    ];
    for (input, bits, used, size) in cases {
        let (v, n) = strtod(&input);
        let parsed = netdata_agent_text::size::size_parse(&input, "B");
        assert_eq!(
            (v.to_bits(), n, parsed),
            (bits, used, size),
            "input of {} bytes",
            input.len()
        );
    }
}
