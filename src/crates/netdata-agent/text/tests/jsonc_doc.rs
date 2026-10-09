//! `jsonc_doc` against json-c itself: the vectors of `tests/oracle/gen-jsonc-doc.c`.

mod common;

use netdata_agent_text::jsonc_doc::{Value, parse, parse_errno};

fn printed(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    value.print_spaced(&mut out);
    out
}

/// Every text of the vectors: one flagged `A` must be read, and whatever is read must be what json-c made of it:
/// its spaced print, whether it has a `version` member and that member as an int, and the print after `version`
/// is set to 7 (in place when the member exists, at the end when not), and the `errno` json-c leaves after the parse
/// and the version's read (`-`: untouched), which is compared too when both refuse the text. A text flagged `R` may
/// be refused. Every difference is listed, not only the first.
#[test]
fn documents_are_read_and_printed_as_json_c_does() {
    let rows = common::rows("jsonc_doc.tsv");
    let show = |bytes: &[u8]| String::from_utf8_lossy(bytes).into_owned();
    let (mut accepted, mut refused, mut differences) = (0, 0, Vec::new());
    for row in &rows {
        let (flag, text) = (row.str(0), row.bytes(1));
        let c_parsed = row.flag(2);
        let mut differs = |what: &str, rust: String, c: String| {
            differences.push(format!("line {} ({flag}) {}: {what}: rust {rust} | json-c {c}", row.line, show(text)));
        };
        let (parsed, mut errno) = parse_errno(text);
        let shown_errno = |errno: Option<i32>| errno.map_or_else(|| "-".to_string(), |e| e.to_string());
        let Some(mut value) = parsed else {
            refused += 1;
            if flag == "A" {
                differs("refused", "nothing".into(), show(row.bytes(3)));
            } else if !c_parsed && shown_errno(errno) != row.str(7) {
                differs("errno", shown_errno(errno), row.str(7).to_string());
            }
            continue;
        };
        accepted += 1;
        if !c_parsed {
            differs("json-c refuses it", show(&printed(&value)), "NULL".into());
            continue;
        }
        if printed(&value) != row.bytes(3) {
            differs("print", show(&printed(&value)), show(row.bytes(3)));
        }
        let version = value.object_get(b"version");
        let (has, int) = (version.is_some(), version.map_or(0, Value::get_int));
        if (has, int) != (row.flag(4), row.num::<i32>(5)) {
            differs("version", format!("{has} {int}"), format!("{} {}", row.str(4), row.str(5)));
        }
        if let Some(read) = version.and_then(|v| v.get_int_errno().1) {
            errno = Some(read);
        }
        if shown_errno(errno) != row.str(7) {
            differs("errno", shown_errno(errno), row.str(7).to_string());
        }
        if matches!(value, Value::Object(_)) {
            value.object_set(b"version", Value::Int(7));
            if printed(&value) != row.bytes(6) {
                differs("print after the version is set", show(&printed(&value)), show(row.bytes(6)));
            }
        }
    }
    assert!(differences.is_empty(), "{} differences:\n{}", differences.len(), differences.join("\n"));
    // the vectors are there, and both kinds of text were met
    assert!(accepted >= 100 && refused >= 50, "{accepted} accepted, {refused} refused of {}", rows.len());
}

/// An object's members are indexed while it is read: one of 100,000 members takes a moment, where a search of the
/// earlier members for each takes many seconds. A repeated key still takes its last value at its first place.
#[test]
fn a_large_object_is_read_without_a_search_per_member() {
    let count = 100_000;
    let mut text = String::from(r#"{"version":1"#);
    for i in 0..count {
        text.push_str(&format!(r#","k{i}":{i}"#));
    }
    text.push_str(r#","k0":"again"}"#);
    let started = std::time::Instant::now();
    let value = parse(text.as_bytes()).expect("an object");
    let took = started.elapsed();
    let Value::Object(members) = &value else { panic!("an object") };
    assert_eq!(members.len(), count + 1);
    assert_eq!(members[1], (b"k0".to_vec(), Value::String(b"again".to_vec())));
    assert_eq!(value.object_get(b"k99999"), Some(&Value::Int(99_999)));
    assert!(took < std::time::Duration::from_secs(5), "{took:?} for {count} members");
}

/// What the vectors cannot show: a member is found by its whole key; a set on what is no object does nothing.
#[test]
fn members_are_found_and_set_by_their_key() {
    let mut value = parse(br#"{"a":1,"version":2,"ab":3}"#).expect("an object");
    assert_eq!(value.object_get(b"ab"), Some(&Value::Int(3)));
    assert_eq!(value.object_get(b"b"), None);
    value.object_set(b"a", Value::Null);
    value.object_set(b"new", Value::Bool(true));
    assert_eq!(printed(&value), br#"{ "a": null, "version": 2, "ab": 3, "new": true }"#);
    let mut list = parse(b"[1]").expect("an array");
    list.object_set(b"version", Value::Int(1));
    assert_eq!((list.object_get(b"version"), printed(&list)), (None, b"[ 1 ]".to_vec()));
}
