//! The parent side of stream paths, ported from `src/streaming/stream-path.c`: the child's `JSON STREAM_PATH` is
//! parsed and stored on its host, and this agent's own entry is added when the path goes back to the child.
//! Decisions D46 in the status repository (strict JSON, change detection by content, one message per retention
//! change).

use netdata_agent_log::{Priority, Source, nd_log};
use netdata_agent_pluginsd_proto::caps;
use netdata_agent_rrd::host::{Host, netdata_start_time};
use netdata_agent_rrd::stream_path::{
    FLAG_ACLK, FLAG_EPHEMERAL, FLAG_HEALTH, FLAG_ML, FLAG_VIRTUAL, PathEntry,
};
use netdata_agent_text::c::c_str;
use netdata_agent_text::json::{JsonOptions, JsonWriter};
use serde_json::{Map, Value};

use crate::jsonc;

/// `STREAM_PATH_JSON_MEMBER`.
const MEMBER: &[u8] = b"streaming_path";

/// `STREAM_PATH_MAX_ENTRIES`.
const MAX_ENTRIES: usize = u16::MAX as usize;

/// `STREAM_PATH_FLAGS` names, in map order.
const FLAG_NAMES: [(u8, &str); 5] = [
    (FLAG_ACLK, "aclk"),
    (FLAG_HEALTH, "health"),
    (FLAG_ML, "ml"),
    (FLAG_EPHEMERAL, "ephemeral"),
    (FLAG_VIRTUAL, "virtual"),
];

/// `STREAM_PATH_FLAGS_2id_one()`.
fn flag_parse_one(name: &[u8]) -> u32 {
    FLAG_NAMES
        .iter()
        .find(|(_, n)| n.as_bytes() == name)
        .map_or(0, |&(flag, _)| u32::from(flag))
}

/// `JSON_TOKENER_DEFAULT_DEPTH`: json-c refuses values nested deeper (serde_json allows 127).
const TOKENER_DEPTH: usize = 32;

/// How deep values nest, the root counting as 1 (keys do not count).
fn depth(value: &Value) -> usize {
    1 + match value {
        Value::Array(items) => items.iter().map(depth).max().unwrap_or(0),
        Value::Object(members) => members.values().map(depth).max().unwrap_or(0),
        _ => 0,
    }
}

/// `nd_time_t_max()` on a 64-bit `time_t`.
const TIME_T_MAX: u64 = i64::MAX as u64;

/// `parse_single_path()`: the members of one entry, then its checks. Messages go to `error`, which C shares by
/// every item of a payload and never empties.
fn parse_single_path(obj: &Map<String, Value>, error: &mut String) -> Option<PathEntry> {
    use jsonc::Presence::{Optional, Required};
    let mut p = PathEntry::default();
    jsonc::uint64(obj, "version", Optional, error)?;
    let hostname = jsonc::txt(obj, "hostname", Required, error)?;
    p.host_id = jsonc::uuid(obj, "host_id", Required, error)?;
    p.node_id = jsonc::uuid(obj, "node_id", Required, error)?;
    p.claim_id = jsonc::uuid(obj, "claim_id", Required, error)?;
    let hops = jsonc::int64(obj, "hops", Required, error)?;
    let since = jsonc::uint64(obj, "since", Required, error)?;
    let first_time_t = jsonc::uint64(obj, "first_time_t", Required, error)?;
    let start_time_ms = jsonc::int64(obj, "start_time", Required, error)?;
    let shutdown_time_ms = jsonc::int64(obj, "shutdown_time", Required, error)?;
    p.flags = jsonc::bitmap(obj, "flags", flag_parse_one, Optional, error)? as u8;
    p.capabilities = jsonc::bitmap(obj, "capabilities", caps::parse_one, Optional, error)?;

    let Some(hostname) = hostname else {
        error.push_str("hostname cannot be empty");
        return None;
    };
    if p.host_id == [0; 16] {
        error.push_str("host_id cannot be zero");
        return None;
    }
    if hops < 0 {
        error.push_str(
            "hops cannot be negative (probably the child disconnected from the Netdata before us",
        );
        return None;
    }
    if hops > i64::from(i16::MAX) {
        error.push_str(&format!("hops cannot exceed {}", i16::MAX));
        return None;
    }
    if p.capabilities == 0 {
        error.push_str("capabilities cannot be empty");
        return None;
    }
    if since == 0 {
        error.push_str("since cannot be <= 0");
        return None;
    }
    if since > TIME_T_MAX {
        error.push_str(&format!("since cannot exceed {TIME_T_MAX}"));
        return None;
    }
    if first_time_t > TIME_T_MAX {
        error.push_str(&format!("first_time_t cannot exceed {TIME_T_MAX}"));
        return None;
    }
    if !(0..=i64::from(u32::MAX)).contains(&start_time_ms) {
        error.push_str(&format!("start_time must be between 0 and {}", u32::MAX));
        return None;
    }
    if !(0..=i64::from(u32::MAX)).contains(&shutdown_time_ms) {
        error.push_str(&format!("shutdown_time must be between 0 and {}", u32::MAX));
        return None;
    }
    p.hostname = hostname;
    p.hops = hops as i16;
    p.since = since as i64;
    p.first_time_t = first_time_t as i64;
    p.start_time_ms = start_time_ms as u32;
    p.shutdown_time_ms = shutdown_time_ms as u32;
    Some(p)
}

/// The entries of a payload C would store, logging what it rejects. `None` when the text is not JSON (the stored
/// path is then kept).
fn parse(hostname: &str, json: &[u8]) -> Option<Vec<PathEntry>> {
    let shown = || String::from_utf8_lossy(json);
    let root = serde_json::from_slice::<Value>(json)
        .ok()
        .filter(|root| depth(root) <= TOKENER_DEPTH);
    let Some(root) = root else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM PATH '{hostname}': Cannot parse json: {}",
            shown()
        );
        return None;
    };
    let mut path = Vec::new();
    // a missing member, or one that is not an array, clears the path silently
    let Some(Value::Array(items)) = root.get("streaming_path") else {
        return Some(path);
    };
    if items.len() > MAX_ENTRIES {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM PATH '{hostname}': Array has {} items, but the maximum is {MAX_ENTRIES}",
            items.len()
        );
        return Some(path);
    }
    let mut error = String::new();
    for (i, item) in items.iter().enumerate() {
        let Value::Object(obj) = item else {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM PATH '{hostname}': Array item No {i} is not an object: {}",
                shown()
            );
            continue;
        };
        match parse_single_path(obj, &mut error) {
            Some(entry) => path.push(entry),
            None => nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM PATH '{hostname}': Array item No {i} cannot be parsed: {error}: {}",
                shown()
            ),
        }
    }
    // stable, as glibc's qsort() of these entries (a merge sort)
    path.sort_by_key(|p| p.hops);
    Some(path)
}

/// `stream_path_set_from_json()` for a path from a child, without the sends: true when the stored path changed, so
/// the child is owed this agent's view of it (C returns whether anything is stored, which its callers ignore).
pub fn set_from_json(host: &Host, json: &[u8]) -> bool {
    // C's body is a C string
    let json = c_str(json);
    if json.is_empty() {
        return false;
    }
    match parse(&host.hostname(), json) {
        Some(path) => host.replace_stream_path(path),
        None => false,
    }
}

/// `rrdhost_stream_path_self()`: this agent's entry in `host`'s path. `first_time_t` replaces the host's current
/// retention start (a message owed for an earlier change carries the value of that change).
pub fn self_entry(host: &Host, localhost: &Host, first_time_t: Option<i64>) -> PathEntry {
    let mut flags = 0;
    if host.is_ephemeral() {
        flags |= FLAG_EPHEMERAL;
    }
    if host.info().health_enabled {
        flags |= FLAG_HEALTH;
    }
    // no claiming (ACLK), virtual hosts or ML here
    let receiver = host.receiver();
    let (hops, since) = match &receiver {
        Some(slot) => (slot.link.hops, slot.link.connected_since_s),
        None => (
            if host.is_localhost() { 0 } else { -1 },
            netdata_start_time(),
        ),
    };
    PathEntry {
        hostname: localhost.hostname(),
        host_id: machine_guid_bytes(localhost),
        node_id: localhost.node_id(),
        claim_id: [0; 16],
        hops,
        since,
        first_time_t: first_time_t.unwrap_or_else(|| host.contexts().retention().0),
        flags,
        capabilities: our_capabilities(receiver.map_or(0, |slot| slot.link.capabilities)),
        // the medians of the agent-event log, 0 on a fresh database (D46 point 8)
        start_time_ms: 0,
        shutdown_time_ms: 0,
    }
}

/// `stream_our_capabilities(host, true)` without ML and without a sender for the host: MLMODELS stays only when
/// the child's receiver negotiated it.
fn our_capabilities(negotiated: u32) -> u32 {
    let mut ours = caps::ours(caps::GLOBALLY_DISABLED);
    if negotiated & caps::ML_MODELS == 0 {
        ours &= !caps::ML_MODELS;
    }
    ours
}

fn machine_guid_bytes(host: &Host) -> [u8; 16] {
    netdata_agent_text::parse::uuid_parse_flexi(host.machine_guid().as_bytes()).unwrap_or_default()
}

/// `stream_path_to_json_object()`.
fn entry_to_json(w: &mut JsonWriter, p: &PathEntry) {
    w.add_array_item_object();
    w.member_add_uint64("version", 1);
    w.member_add_string("hostname", &p.hostname);
    w.member_add_uuid("host_id", &p.host_id);
    w.member_add_uuid("node_id", &p.node_id);
    w.member_add_uuid("claim_id", &p.claim_id);
    w.member_add_int64("hops", i64::from(p.hops));
    w.member_add_uint64("since", p.since as u64);
    w.member_add_uint64("first_time_t", p.first_time_t as u64);
    w.member_add_uint64("start_time", u64::from(p.start_time_ms));
    w.member_add_uint64("shutdown_time", u64::from(p.shutdown_time_ms));
    caps::to_json_array(w, p.capabilities, Some(b"capabilities"));
    // STREAM_PATH_FLAGS_2json()
    w.member_add_array(Some(b"flags"));
    for (flag, name) in FLAG_NAMES {
        if p.flags & flag != 0 {
            w.add_array_item_string(name);
        }
    }
    w.array_close();
    w.object_close();
}

/// `rrdhost_stream_path_to_json()`: the stored entries in order, this agent's one replaced by its current view (or
/// appended when the path does not have it).
pub fn to_json(
    w: &mut JsonWriter,
    host: &Host,
    localhost: &Host,
    key: &[u8],
    add_version: bool,
    first_time_t: Option<i64>,
) {
    if add_version {
        w.member_add_uint64("version", 1);
    }
    let me = self_entry(host, localhost, first_time_t);
    w.member_add_array(Some(key));
    let mut found_self = false;
    for p in &host.stream_path() {
        if p.host_id == me.host_id {
            found_self = true;
            entry_to_json(w, &me);
        } else {
            entry_to_json(w, p);
        }
    }
    if !found_self {
        entry_to_json(w, &me);
    }
    w.array_close();
}

/// What `stream_path_send_to_child()` writes: `stream_path_payload()` framed as a `JSON STREAM_PATH` block.
pub fn message(host: &Host, localhost: &Host, first_time_t: Option<i64>) -> Vec<u8> {
    let mut w = JsonWriter::new(JsonOptions::MINIFY);
    to_json(&mut w, host, localhost, MEMBER, true, first_time_t);
    w.finalize();
    let mut out = b"JSON STREAM_PATH\n".to_vec();
    out.extend_from_slice(w.as_bytes());
    out.extend_from_slice(b"\nJSON_PAYLOAD_END\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One item through `parse_single_path()` with a fresh error buffer.
    fn item(json: &str) -> Result<PathEntry, String> {
        let Ok(Value::Object(obj)) = serde_json::from_str::<Value>(json) else {
            panic!("not an object: {json}");
        };
        let mut error = String::new();
        parse_single_path(&obj, &mut error).ok_or(error)
    }

    /// The item of C's `stream_path_json_unittest()` with its numeric members.
    fn c_item(hops: &str, start: &str, shutdown: &str, since: &str, first: &str) -> String {
        format!(
            r#"{{"hostname":"test-host","host_id":"11111111-1111-1111-1111-111111111111","node_id":"00000000-0000-0000-0000-000000000000","claim_id":"00000000-0000-0000-0000-000000000000","hops":{hops},"since":{since},"first_time_t":{first},"start_time":{start},"shutdown_time":{shutdown},"capabilities":["V1"]}}"#
        )
    }

    /// C's 20 scalar cases (`src/streaming/stream-path.c` `stream_path_json_unittest()`), with the error texts the
    /// spec observed (C does not assert them).
    #[test]
    fn the_c_unit_test_table() {
        const TMAX: &str = "9223372036854775807";
        const TMAX1: &str = "9223372036854775808";
        const START: &str = "start_time must be between 0 and 4294967295";
        const SHUTDOWN: &str = "shutdown_time must be between 0 and 4294967295";
        type Want = Result<(i16, u32, u32, i64, i64), &'static str>;
        let tmax1_str = format!("\"{TMAX1}\"");
        let cases: Vec<(&str, [&str; 5], Want)> = vec![
            (
                "exact endpoints",
                ["32767", "4294967295", "4294967295", "1", "1"],
                Ok((32767, u32::MAX, u32::MAX, 1, 1)),
            ),
            (
                "lower endpoints",
                ["0", "0", "0", "1", "0"],
                Ok((0, 0, 0, 1, 0)),
            ),
            (
                "boolean and null coercions",
                ["true", "null", "false", "true", "null"],
                Ok((1, 0, 0, 1, 0)),
            ),
            (
                "double truncation",
                ["1.9", "1.9", "1.9", "1.9", "1.9"],
                Ok((1, 1, 1, 1, 1)),
            ),
            (
                "fractional zero coercion",
                ["0.9", "-0.5", "-0.5", "1", "0.9"],
                Ok((0, 0, 0, 1, 0)),
            ),
            (
                "hops below destination",
                ["-65536", "1", "1", "1", "1"],
                Err(
                    "hops cannot be negative (probably the child disconnected from the Netdata before us",
                ),
            ),
            (
                "hops positive wrap",
                ["65536", "1", "1", "1", "1"],
                Err("hops cannot exceed 32767"),
            ),
            (
                "hops string wrap",
                ["\"65536\"", "1", "1", "1", "1"],
                Err("hops cannot exceed 32767"),
            ),
            (
                "negative start time",
                ["1", "-1", "1", "1", "1"],
                Err(START),
            ),
            (
                "start time above endpoint",
                ["1", "4294967296", "1", "1", "1"],
                Err(START),
            ),
            (
                "start time string wrap",
                ["1", "\"4294967296\"", "1", "1", "1"],
                Err(START),
            ),
            (
                "negative shutdown time",
                ["1", "1", "-1", "1", "1"],
                Err(SHUTDOWN),
            ),
            (
                "shutdown time above endpoint",
                ["1", "1", "4294967296", "1", "1"],
                Err(SHUTDOWN),
            ),
            (
                "shutdown time string wrap",
                ["1", "1", "\"4294967296\"", "1", "1"],
                Err(SHUTDOWN),
            ),
            (
                "time_t upper endpoints",
                ["1", "1", "1", TMAX, TMAX],
                Ok((1, 1, 1, i64::MAX, i64::MAX)),
            ),
            (
                "since above time_t",
                ["1", "1", "1", TMAX1, "1"],
                Err("since cannot exceed 9223372036854775807"),
            ),
            (
                "since string above time_t",
                ["1", "1", "1", &tmax1_str, "1"],
                Err("since cannot exceed 9223372036854775807"),
            ),
            (
                "first_time_t above time_t",
                ["1", "1", "1", "1", TMAX1],
                Err("first_time_t cannot exceed 9223372036854775807"),
            ),
            (
                "first_time_t string above time_t",
                ["1", "1", "1", "1", &tmax1_str],
                Err("first_time_t cannot exceed 9223372036854775807"),
            ),
            (
                "first_time_t negative integer coercion",
                ["1", "1", "1", "1", "-1"],
                Ok((1, 1, 1, 1, 0)),
            ),
        ];
        for (name, [hops, start, shutdown, since, first], want) in cases {
            let got = item(&c_item(hops, start, shutdown, since, first)).map(|p| {
                (
                    p.hops,
                    p.start_time_ms,
                    p.shutdown_time_ms,
                    p.since,
                    p.first_time_t,
                )
            });
            assert_eq!(got, want.map_err(str::to_string), "{name}");
        }
    }

    /// The spec's extra vectors (§7.4): one member of a valid item replaced.
    #[test]
    fn member_coercions_and_errors() {
        let base = c_item("1", "1", "1", "1", "1");
        let with = |member: &str, value: &str| -> Result<PathEntry, String> {
            let mut obj: Map<String, Value> = serde_json::from_str(&base).unwrap();
            match value {
                "<missing>" => {
                    obj.remove(member);
                }
                v => {
                    obj.insert(member.to_string(), serde_json::from_str(v).unwrap());
                }
            }
            item(&Value::Object(obj).to_string())
        };
        let err = |member: &str, value: &str| with(member, value).unwrap_err();
        let ok = |member: &str, value: &str| {
            with(member, value).unwrap_or_else(|e| panic!("{member}={value}: {e}"))
        };
        assert_eq!(
            err("version", r#""12 ""#),
            "cannot convert string '12 ' to uint64 for '.version'"
        );
        assert_eq!(
            err("version", "-0.5"),
            "cannot convert to uint64 for '.version'"
        );
        ok("version", r#"" -1""#);
        ok("version", "{}");
        assert_eq!(ok("hostname", "1.5").hostname, "1.5");
        assert_eq!(ok("hostname", "true").hostname, "true");
        assert_eq!(
            ok("hostname", "9223372036854775808").hostname,
            "9223372036854775807"
        );
        assert_eq!(
            err("hostname", "[]"),
            "cannot convert to string for '.hostname'"
        );
        assert_eq!(err("hostname", r#""\u0000b""#), "hostname cannot be empty");
        assert_eq!(ok("hostname", r#""a\u0000b""#).hostname, "a");
        assert_eq!(
            ok("host_id", r#""AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE""#).host_id[0],
            0xaa
        );
        assert_eq!(
            err("host_id", r#""{11111111-1111-1111-1111-111111111111}""#),
            "invalid UUID '.host_id'"
        );
        assert_eq!(err("host_id", r#""""#), "invalid UUID '.host_id'");
        assert_eq!(err("host_id", "1"), "invalid type for UUID '.host_id'");
        assert_eq!(err("host_id", "null"), "host_id cannot be zero");
        assert_eq!(err("node_id", "<missing>"), "missing UUID '.node_id'");
        assert_eq!(ok("hops", r#""010""#).hops, 10);
        assert_eq!(ok("hops", r#""\t12""#).hops, 12);
        assert_eq!(ok("hops", r#""12\u00003""#).hops, 12);
        assert_eq!(
            err("hops", r#""0x10""#),
            "cannot convert string '0x10' to int64 for '.hops'"
        );
        assert_eq!(
            err("hops", "9.2233720368547758e18"),
            "cannot convert to int64 for '.hops'"
        );
        assert!(err("hops", "-9.2233720368547758e18").starts_with("hops cannot be negative"));
        assert_eq!(
            err("hops", "9223372036854775808"),
            "hops cannot exceed 32767"
        );
        assert_eq!(err("hops", "{}"), "cannot convert to int64 for '.hops'");
        for v in ["-1", "null", "false", "0.9"] {
            assert_eq!(err("since", v), "since cannot be <= 0", "since={v}");
        }
        assert_eq!(
            err("since", r#""-1""#),
            "cannot convert negative string '-1' to uint64 for '.since'"
        );
        assert_eq!(
            err("since", r#"" -1""#),
            "since cannot exceed 9223372036854775807"
        );
        assert_eq!(
            err("since", r#""18446744073709551616""#),
            "cannot convert string '18446744073709551616' to uint64 for '.since'"
        );
        assert_eq!(
            err("since", "1.8446744073709552e19"),
            "cannot convert to uint64 for '.since'"
        );
        assert_eq!(ok("first_time_t", "-9223372036854775808").first_time_t, 0);
        assert_eq!(ok("flags", r#"["bogus","aclk"]"#).flags, FLAG_ACLK);
        assert_eq!(
            err("flags", r#"["aclk",null]"#),
            "invalid type for '.flags' at index 1"
        );
        assert_eq!(ok("flags", r#""aclk""#).flags, 0);
        assert_eq!(
            ok("capabilities", r#"["ML"]"#).capabilities,
            caps::DATA_WITH_ML
        );
        assert_eq!(
            ok("capabilities", r#"["V1\u0000X"]"#).capabilities,
            caps::V1
        );
        assert_eq!(
            err("capabilities", r#"["V1",1]"#),
            "invalid type for '.capabilities' at index 1"
        );
        for v in [r#""V1""#, "null", "<missing>"] {
            assert_eq!(
                err("capabilities", v),
                "capabilities cannot be empty",
                "capabilities={v}"
            );
        }
        assert_eq!(
            err("capabilities", r#"["bogus"]"#),
            "unknown option 'bogus' in '.capabilities' at index 0capabilities cannot be empty"
        );
    }

    /// The messages `parse()` logs.
    fn logged(body: &str) -> (Option<Vec<PathEntry>>, Vec<String>) {
        let (path, records) = netdata_agent_log::capture(|| parse("h", body.as_bytes()));
        (
            path,
            records.into_iter().filter_map(|r| r.message).collect(),
        )
    }

    /// C's error buffer is shared by every item of a payload and never emptied: failures log what earlier items
    /// appended, warnings of accepted items included (the spec's worked examples, §6).
    #[test]
    fn errors_accumulate_across_items() {
        let entry = |hops: &str, caps: &str, hostname: &str| {
            format!(
                r#"{{"hostname":"{hostname}","host_id":"11111111-1111-1111-1111-111111111111","node_id":null,"claim_id":null,"hops":{hops},"since":1,"first_time_t":1,"start_time":0,"shutdown_time":0,"capabilities":{caps}}}"#
            )
        };
        let body = format!(
            r#"{{"streaming_path":[{},{},{}]}}"#,
            entry("0", r#"["V1","BOGUS"]"#, "a"),
            entry("1", r#"["V1"]"#, "b"),
            entry("-1", r#"["V1"]"#, "c")
        );
        let (path, messages) = logged(&body);
        assert_eq!(path.map(|p| p.len()), Some(2));
        assert_eq!(
            messages,
            [format!(
                "STREAM PATH 'h': Array item No 2 cannot be parsed: unknown option 'BOGUS' in '.capabilities' at index 1hops cannot be negative (probably the child disconnected from the Netdata before us: {body}"
            )]
        );
        let body = format!(
            r#"{{"streaming_path":[{},7,{},{}]}}"#,
            entry("40000", r#"["V1"]"#, "a"),
            entry("1", r#"["V1"]"#, ""),
            entry("1", "[]", "d")
        );
        let (path, messages) = logged(&body);
        assert_eq!(path.map(|p| p.len()), Some(0));
        let prefix = "STREAM PATH 'h': Array item No";
        assert_eq!(
            messages,
            [
                format!("{prefix} 0 cannot be parsed: hops cannot exceed 32767: {body}"),
                format!("{prefix} 1 is not an object: {body}"),
                format!(
                    "{prefix} 2 cannot be parsed: hops cannot exceed 32767hostname cannot be empty: {body}"
                ),
                format!(
                    "{prefix} 3 cannot be parsed: hops cannot exceed 32767hostname cannot be emptycapabilities cannot be empty: {body}"
                ),
            ]
        );
    }

    /// C's rejected item through the caller, and its oversized array: nothing is stored.
    #[test]
    fn rejected_items_and_oversized_arrays_store_nothing() {
        let body = format!(
            r#"{{"streaming_path":[{}]}}"#,
            c_item("65536", "1", "1", "1", "1")
        );
        let (path, messages) = logged(&body);
        assert_eq!(path, Some(vec![]));
        assert_eq!(
            messages,
            [format!(
                "STREAM PATH 'h': Array item No 0 cannot be parsed: hops cannot exceed 32767: {body}"
            )]
        );
        let body = format!(
            r#"{{"streaming_path":[null{}]}}"#,
            ",null".repeat(MAX_ENTRIES)
        );
        let (path, messages) = logged(&body);
        assert_eq!(path, Some(vec![]));
        assert_eq!(
            messages,
            ["STREAM PATH 'h': Array has 65536 items, but the maximum is 65535"]
        );
    }

    /// json-c refuses values nested deeper than 32; the root and every value count, keys do not.
    #[test]
    fn nesting_deeper_than_json_c_is_not_json() {
        let nested = |n: usize| format!("{}1{}", "[".repeat(n), "]".repeat(n));
        let body = |n: usize| format!(r#"{{"x":{}}}"#, nested(n));
        // the root object is depth 1, so 30 arrays around a scalar make 32
        assert_eq!(logged(&body(30)), (Some(vec![]), vec![]));
        let (path, messages) = logged(&body(31));
        assert_eq!(path, None);
        assert_eq!(messages.len(), 1);
        assert!(messages[0].starts_with("STREAM PATH 'h': Cannot parse json: "));
    }
}
