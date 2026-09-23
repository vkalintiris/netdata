//! The JSON writer against the C golden vectors (scripts of `buffer_json_*()`
//! calls replayed on [`JsonWriter`]), plus the JSON part of `buffer_unittest()`.

mod common;

use common::{Row, check, text};
use netdata_agent_text::datetime::rfc3339_datetime_utc;
use netdata_agent_text::json::{JsonOptions, JsonWriter, json_escape, json_escape_quoted};

fn uuid(arg: &[u8]) -> [u8; 16] {
    arg.try_into().expect("16 byte uuid")
}

fn num<T: std::str::FromStr>(arg: &[u8]) -> T
where
    T::Err: std::fmt::Debug,
{
    std::str::from_utf8(arg).unwrap().parse().unwrap()
}

fn flag(arg: &[u8]) -> bool {
    num::<u8>(arg) != 0
}

fn options(bits: u64) -> JsonOptions {
    let mut options = JsonOptions::DEFAULT;
    if bits & 1 != 0 {
        options = options | JsonOptions::MINIFY;
    }
    if bits & 2 != 0 {
        options = options | JsonOptions::NEWLINE_ON_ARRAY_ITEMS;
    }
    options
}

fn unhex(arg: &[u8]) -> Vec<u8> {
    arg.chunks(2)
        .map(|h| u8::from_str_radix(std::str::from_utf8(h).unwrap(), 16).unwrap())
        .collect()
}

/// Replays a generator script (operations split by 0x1e, hex arguments by 0x1f).
fn replay(script: &[u8]) -> Vec<u8> {
    let ops: Vec<(&[u8], Vec<Vec<u8>>)> = script
        .split(|&b| b == 0x1e)
        .map(|op| {
            let mut parts = op.split(|&b| b == 0x1f);
            let code = parts.next().unwrap();
            (code, parts.map(unhex).collect())
        })
        .collect();

    let (code, init) = &ops[0];
    assert_eq!(*code, b"I");
    let mut w = JsonWriter::with_quotes(
        &init[0],
        &init[1],
        num(&init[2]),
        flag(&init[3]),
        options(num(&init[4])),
    );

    for (code, args) in &ops[1..] {
        let a: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
        match *code {
            b"IA" => w.add_array_item_array(),
            b"IO" => w.add_array_item_object(),
            b"IS" => w.add_array_item_string(a[0]),
            b"IN" => w.add_array_item_null(),
            b"IU" => w.add_array_item_uuid(Some(&uuid(a[0]))),
            b"IUC" => w.add_array_item_uuid_compact(Some(&uuid(a[0]))),
            b"IUN" => w.add_array_item_uuid(None),
            b"ID" => w.add_array_item_double(f64::from_bits(num(a[0]))),
            b"II" => w.add_array_item_int64(num(a[0])),
            b"IU64" => w.add_array_item_uint64(num(a[0])),
            b"IB" => w.add_array_item_boolean(flag(a[0])),
            b"IT" => w.add_array_item_time_t(num(a[0])),
            b"ITM" => w.add_array_item_time_ms(num(a[0])),
            b"ITF" => w.add_array_item_time_t_formatted(num(a[0]), flag(a[1])),
            b"IDT" => w.add_array_item_datetime_rfc3339_utc(num(a[0])),
            b"R" => w.raw(a[0]),
            b"MO" => w.member_add_object(a[0]),
            b"MA" => w.member_add_array(Some(a[0])),
            b"MAN" => w.member_add_array(None),
            b"MS" => w.member_add_string(a[0], a[1]),
            b"MN" => w.member_add_null(a[0]),
            b"MSO" => w.member_add_string_or_omit(a[0], Some(a[1])),
            b"MSON" => w.member_add_string_or_omit(a[0], None),
            b"MSE" => w.member_add_string_or_empty(a[0], Some(a[1])),
            b"MSEN" => w.member_add_string_or_empty(a[0], None),
            b"MQ" => w.member_add_quoted_string(a[0], Some(a[1])),
            b"MQN" => w.member_add_quoted_string(a[0], None),
            b"MU" => w.member_add_uuid(a[0], &uuid(a[1])),
            b"MUP" => w.member_add_uuid_ptr(a[0], Some(&uuid(a[1]))),
            b"MUPN" => w.member_add_uuid_ptr(a[0], None),
            b"MUC" => w.member_add_uuid_compact(a[0], &uuid(a[1])),
            b"MB" => w.member_add_boolean(a[0], flag(a[1])),
            b"MU64" => w.member_add_uint64(a[0], num(a[1])),
            b"MI" => w.member_add_int64(a[0], num(a[1])),
            b"MD" => w.member_add_double(a[0], f64::from_bits(num(a[1]))),
            b"MT" => w.member_add_time_t(a[0], num(a[1])),
            b"MTF" => w.member_add_time_t_formatted(a[0], num(a[1]), flag(a[2])),
            b"MDT" => w.member_add_datetime_rfc3339_utc(a[0], num(a[1])),
            b"MDU" => w.member_add_duration_ut(a[0], num(a[1])),
            b"OC" => w.object_close(),
            b"AC" => w.array_close(),
            b"F" => w.finalize(),
            other => panic!("unknown operation {}", text(other)),
        }
    }
    w.into_bytes()
}

#[test]
fn writer_vectors() {
    check(
        "json.tsv",
        |row: &Row| text(row.bytes(1)),
        |row| text(&replay(row.bytes(0))),
    );
}

#[test]
fn escape_vectors() {
    check(
        "json_escape.tsv",
        |row| (text(row.bytes(1)), text(row.bytes(2))),
        |row| {
            let mut strcat = Vec::new();
            json_escape(&mut strcat, row.bytes(0));
            let mut quoted = Vec::new();
            json_escape_quoted(&mut quoted, row.bytes(0));
            (text(&strcat), text(&quoted))
        },
    );
}

#[test]
fn rfc3339_vectors() {
    check(
        "rfc3339.tsv",
        |row| row.str(2).to_string(),
        |row| rfc3339_datetime_utc(row.num(0), row.num(1)),
    );
}

/// The JSON checks of `buffer_unittest()` in `libnetdata/buffer/buffer.c`.
#[test]
fn c_buffer_unittest_json() {
    let mut empty = JsonWriter::new(JsonOptions::DEFAULT);
    empty.finalize();

    let mut members = JsonWriter::new(JsonOptions::DEFAULT);
    members.member_add_string("hello", "world");
    members.member_add_string("alpha", "this: \" is a double quote");
    members.member_add_object("object1");
    members.member_add_string("hello", "world");
    members.finalize();

    assert_eq!(
        (text(empty.as_bytes()), text(members.as_bytes())),
        (
            "{\n}\n".to_string(),
            "{\n    \"hello\":\"world\",\n    \"alpha\":\"this: \\\" is a double quote\",\n    \"object1\":{\n        \"hello\":\"world\"\n    }\n}\n"
                .to_string()
        )
    );
}

/// Writers started below the root (`depth + 2`, no anonymous object) produce
/// members to paste into a parent, as `api_v1_contexts.c` does.
#[test]
fn nested_writer_pastes_into_parent() {
    let mut parent = JsonWriter::new(JsonOptions::DEFAULT);
    let mut child = JsonWriter::with_quotes(
        b"\"",
        b"\"",
        parent.depth() + 2,
        false,
        JsonOptions::DEFAULT,
    );
    child.member_add_uint64(b"a", 1);
    parent.member_add_object(b"ctx");
    parent.member_add_object(b"dimensions");
    parent.raw(child.as_bytes());
    parent.object_close();
    parent.finalize();
    assert_eq!(
        text(parent.as_bytes()),
        "{\n    \"ctx\":{\n        \"dimensions\":{\n            \"a\":1\n        }\n    }\n}\n"
    );
}
