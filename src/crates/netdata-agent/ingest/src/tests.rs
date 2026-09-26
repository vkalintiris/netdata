use std::sync::Arc;

use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;

use super::*;

/// The fixed wall clock of these tests.
const NOW: i64 = 1_700_000_000;

fn host() -> Arc<Host> {
    named_host("child", "guid", false)
}

fn named_host(hostname: &str, guid: &str, is_localhost: bool) -> Arc<Host> {
    let mut info = HostInfo {
        hostname: hostname.into(),
        registry_hostname: hostname.into(),
        os: "linux".into(),
        timezone: "UTC".into(),
        abbrev_timezone: "UTC".into(),
        utc_offset: 0,
        program_name: "p".into(),
        program_version: "1".into(),
        update_every: 1,
        db_mode: DbMode::Ram,
        history_entries: 4096,
        health_enabled: false,
        system_info: Default::default(),
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        stream_send: None,
        cache_dir: None,
    };
    info.set_replication(true, 86400, 3600);
    Arc::new(Host::new(guid, is_localhost, info))
}

fn parser(host: &Arc<Host>) -> Parser {
    Parser::new(
        Arc::clone(host),
        named_host("parent", "5a1e0000-0000-4000-8000-0000000000aa", true),
        Config {
            capabilities: 0,
            update_every: 1,
            page_size: 4096,
            now: || (NOW, 0),
            gap_when_lost_iterations_above: 3,
        },
    )
}

/// Feeds the lines and returns the messages the parser logged meanwhile.
fn feed_logged(p: &mut Parser, lines: &[&str]) -> (Vec<bool>, Vec<String>) {
    let (results, records) = netdata_agent_log::capture(|| feed_all(p, lines));
    (
        results,
        records.into_iter().filter_map(|r| r.message).collect(),
    )
}

fn feed_all(p: &mut Parser, lines: &[&str]) -> Vec<bool> {
    lines
        .iter()
        .map(|l| p.feed(format!("{l}\n").as_bytes()))
        .collect()
}

const DEFINE: [&str; 5] = [
    "CHART 'test.c1' '' 'title' 'units' 'family' 'ctx.c1' line 1000 1 '' fixture-pusher corpus",
    "DIMENSION 'd1' '' absolute 1 1 ''",
    "DIMENSION 'd2' 'second' absolute 1 1 ''",
    "CLABEL 'k' 'v' 2",
    "CLABEL_COMMIT",
];

#[test]
fn a_chart_is_defined_and_collected_with_v2() {
    let h = host();
    let mut p = parser(&h);
    assert!(feed_all(&mut p, &DEFINE).iter().all(|&ok| ok));
    let chart = h.charts().find("test.c1").unwrap();
    let meta = chart.meta();
    assert_eq!(
        (
            meta.context.as_str(),
            meta.family.as_str(),
            meta.plugin.as_str()
        ),
        ("ctx.c1", "family", "fixture-pusher")
    );
    assert_eq!(meta.labels.get(b"k"), Some(&b"v"[..]));
    assert_eq!(chart.dim("d2").unwrap().meta().name, "second");
    let t = NOW - 10;
    let lines = [
        format!("BEGIN2 'test.c1' 1 {t} #"),
        "SET2 'd1' 5 5 A".to_string(),
        "SET2 'd2' 0 NAN E".to_string(),
        "END2".to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    let d1 = chart.dim("d1").unwrap();
    let ring = d1.ring().unwrap();
    assert_eq!(ring.latest_time_s(), t);
    let mut q = ring.query(t, t);
    let point = q.next_metric();
    assert_eq!((point.sum, point.anomaly_count), (5.0, 0));
    let d2 = chart.dim("d2").unwrap();
    let mut q = d2.ring().unwrap().query(t, t);
    assert!(q.next_metric().is_gap());
    assert_eq!(chart.collection().last_updated, (t, 0));
    assert_eq!(p.data_collections_count, 1);
}

#[test]
fn chart_definition_end_asks_for_replication() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    // The child has the last 100 seconds; this host has nothing, so it asks from now - 3600 (step) onwards.
    let first = NOW - 100;
    assert!(p.feed(format!("CHART_DEFINITION_END {first} {NOW} {NOW}\n").as_bytes()));
    let out = String::from_utf8(p.take_output()).unwrap();
    assert_eq!(
        out,
        format!("REPLAY_CHART \"test.c1\" \"true\" {first} {NOW}\n")
    );
    // A second one in the same round asks nothing.
    assert!(p.feed(format!("CHART_DEFINITION_END {first} {NOW} {NOW}\n").as_bytes()));
    assert!(p.take_output().is_empty());
}

/// `stream_receiver_replication_reset()`: a chart still replicating when its child disconnects asks again after the
/// reconnect (C resets the flags when a receiver attaches and when it detaches).
#[test]
fn replication_is_asked_again_after_a_reconnect() {
    let h = host();
    let attach = |h: &Arc<Host>| {
        let slot = Arc::new(netdata_agent_rrd::host::ReceiverSlot::new(
            0,
            Default::default(),
            netdata_agent_rrd::host::ReceiverLink::default(),
            Box::new(|| {}),
        ));
        assert!(h.set_receiver(Arc::clone(&slot)));
        slot
    };
    let first = NOW - 100;
    let end = format!("CHART_DEFINITION_END {first} {NOW} {NOW}");
    let slot = attach(&h);
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    feed_all(&mut p, &[&end]);
    assert!(!p.take_output().is_empty());
    // the child goes away mid-replication and comes back
    h.clear_receiver(&slot);
    attach(&h);
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    feed_all(&mut p, &[&end]);
    assert_eq!(
        String::from_utf8(p.take_output()).unwrap(),
        format!("REPLAY_CHART \"test.c1\" \"true\" {first} {NOW}\n")
    );
}

#[test]
fn replication_rows_are_stored_and_rend_finishes() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let (s, e) = (NOW - 20, NOW - 19);
    let lines = [
        "RBEGIN 'test.c1'".to_string(),
        format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
        "RSET 'd1' 7 A".to_string(),
        "RSET 'd2' '' ''".to_string(),
        format!("REND 1 {} {} true {} {} 0x{:x}", NOW - 100, NOW, s, e, NOW),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    let chart = h.charts().find("test.c1").unwrap();
    let d1 = chart.dim("d1").unwrap();
    let mut q = d1.ring().unwrap().query(e, e);
    assert_eq!(q.next_metric().sum, 7.0);
    // An empty RSET value is "NAN", which str2ndd reads as 0.0: stored as a number, as in C.
    let d2 = chart.dim("d2").unwrap();
    let mut q = d2.ring().unwrap().query(e, e);
    assert_eq!(q.next_metric().sum, 0.0);
    let flags = chart.meta().flags;
    assert_ne!(flags & flags::RECEIVER_REPLICATION_FINISHED, 0);
    assert_eq!(flags & flags::RECEIVER_REPLICATION_IN_PROGRESS, 0);
}

#[test]
fn errors_disconnect() {
    let cases: [&[&str]; 6] = [
        &["NOT_A_KEYWORD"],
        &["HOST_DEFINE a b"],
        &["SET2 'd1' 1 1 A"],
        &["CHART 'test.c1' '' t u f c line 1 1", "CLABEL_COMMIT"],
        &["CHART 'nodot' '' t u f c line 1 1"],
        &[
            "CHART 'test.c1' '' t u f c line 1 1",
            "BEGIN2 'test.c1' 1 10 #",
            "SET2 'missing' 1 1 A",
        ],
    ];
    for lines in cases {
        let h = host();
        let mut p = parser(&h);
        let (results, logs) = feed_logged(&mut p, lines);
        assert_eq!(results.last(), Some(&false), "{lines:?}");
        assert!(logs.last().unwrap().contains("parser_action("), "{lines:?}");
    }
    // Blank lines and deferred JSON bodies never reach the keyword table.
    let h = host();
    let mut p = parser(&h);
    assert!(
        feed_all(
            &mut p,
            &[
                "",
                "JSON STREAM_PATH",
                "{\"x\": 1}",
                "NOT_A_KEYWORD",
                "JSON_PAYLOAD_END"
            ]
        )
        .iter()
        .all(|&ok| ok)
    );
}

/// PLUGINSD_DISABLE_PLUGIN(): the keyword's reason is a collector info record, then the daemon's parser_action error.
#[test]
fn a_refused_keyword_logs_its_reason_as_c() {
    let h = host();
    let mut p = parser(&h);
    let (_, records) =
        netdata_agent_log::capture(|| feed_all(&mut p, &["CHART 'nodot' '' t u f c line 1 1"]));
    let records: Vec<_> = records
        .into_iter()
        .map(|r| (r.source, r.priority, r.message.unwrap_or_default()))
        .collect();
    assert!(
        matches!(&records[..], [
            (Source::Collector, Priority::Info, reason),
            (Source::Daemon, Priority::Err, action),
        ] if reason.starts_with("PLUGINSD: keyword CHART: ") && action.starts_with("PLUGINSD: parser_action(")),
        "{records:?}"
    );
}

/// The wall clock of `v1_collection_times_keep_their_microseconds`, in microseconds, advanced by the test.
static CLOCK_UT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[test]
fn v1_collection_times_keep_their_microseconds() {
    use std::sync::atomic::Ordering::Relaxed;
    let h = host();
    let mut p = parser(&h);
    p.config.now = || {
        let ut = CLOCK_UT.load(Relaxed);
        (ut / 1_000_000, ut % 1_000_000)
    };
    feed_all(&mut p, &DEFINE[..2]);
    // Collections 1.25 s apart, timed by their trusted durations, with the clock at each collection's time. A clock
    // without its microseconds lags the last collection, which rrdset_timed_next() then treats as a database in the
    // future and snaps onto whole seconds. Kept off the grid, the point stored between the 0 and 100 steps blends.
    for (i, value) in [0, 0, 0, 100, 100, 100].into_iter().enumerate() {
        CLOCK_UT.store(
            (NOW - 20) * 1_000_000 + 300_000 + i as i64 * 1_250_000,
            Relaxed,
        );
        let lines = [
            "BEGIN 'test.c1' 1250000".to_string(),
            format!("SET 'd1' = {value}"),
            "END".to_string(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    }
    let d1 = h.charts().find("test.c1").unwrap().dim("d1").unwrap();
    let ring = d1.ring().unwrap();
    let mut q = ring.query(ring.oldest_time_s(), ring.latest_time_s());
    let mut stored = Vec::new();
    while !q.is_finished() {
        stored.push(q.next_metric().sum);
    }
    assert!(stored.iter().any(|&v| v > 0.0 && v < 99.0), "{stored:?}");
}

#[test]
fn v1_collections_are_stored_on_the_grid() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE[..2]);
    // Five collections one second apart, timed by the child (END sec usec); the first only starts the clock.
    for i in 0..5 {
        let t = NOW - 10 + i;
        let lines = [
            "BEGIN 'test.c1' 1000000".to_string(),
            "SET 'd1' = 42".to_string(),
            format!("END {t} 0"),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    }
    let chart = h.charts().find("test.c1").unwrap();
    let d1 = chart.dim("d1").unwrap();
    let ring = d1.ring().unwrap();
    assert!(chart.collection().counter >= 3, "{:?}", chart.collection());
    let latest = ring.latest_time_s();
    let mut q = ring.query(latest, latest);
    assert_eq!(q.next_metric().sum, 42.0);
    assert_eq!(p.data_collections_count, 5);
}

#[test]
fn functions_are_registered_on_the_host() {
    let h = host();
    let mut p = parser(&h);
    let lines = [
        "FUNCTION GLOBAL \"processes\" 10 \"Running processes\" \"top\" \"0x13\" 5",
        "FUNCTION GLOBAL \"config\" 120 \"Dynamic configuration\" \"config\" 0x8 1000",
        "FUNCTION \"gone\" 0 \"h\" \"\" \"member\" 0 7",
        "FUNCTION_DEL GLOBAL \"gone\"",
        "FUNCTION_DEL GLOBAL \"config\"",
        "FUNCTION_PROGRESS abc 1 2",
        "FUNCTION_RESULT_BEGIN abc 200 application/json 0",
        "NOT_A_KEYWORD inside a result body",
        "FUNCTION_RESULT_END",
        "DYNCFG_ENABLE anything",
    ];
    let (results, logs) = feed_logged(&mut p, &lines);
    assert!(results.iter().all(|&ok| ok));
    let names: Vec<_> = h
        .functions()
        .all()
        .into_iter()
        .map(|(k, _)| String::from_utf8(k).unwrap())
        .collect();
    assert_eq!(names, ["processes", "config"]);
    let f = h.functions().get(b"processes").unwrap();
    assert_eq!(
        (
            f.timeout_s,
            f.priority,
            f.access,
            f.flags,
            f.help.as_slice()
        ),
        (10, 5, 0x13, 0, &b"Running processes"[..])
    );
    // Only the daemon removes dynamic configuration.
    assert_eq!(
        h.functions().get(b"config").unwrap().flags,
        nrpc::FLAG_DYNCFG
    );
    // FUNCTION x3, FUNCTION_DEL x2 and the finished result.
    assert_eq!(p.data_collections_count, 6);
    assert!(
        logs.iter()
            .any(|l| l.contains("refusing to unregister dyncfg method 'config'"))
    );
    assert!(
        logs.iter().any(|l| l
            == "got a FUNCTION_PROGRESS for transaction 'abc', but the transaction is not found.")
    );

    // The name, timeout and help are required.
    let mut p = parser(&h);
    let (results, logs) = feed_logged(&mut p, &["FUNCTION GLOBAL \"x\" 10"]);
    assert_eq!(results, [false]);
    assert!(logs[0].contains("without providing the required data (global = 'yes', name = 'x', timeout = '10', priority = '(unset)', version = '(unset)', help = '(unset)')"));
}

#[test]
fn a_label_change_resyncs_the_instance_hidden_flag() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let ri = || {
        h.charts()
            .find("test.c1")
            .unwrap()
            .contexts()
            .instance()
            .unwrap()
    };
    // Re-sent as hidden with nothing else changed: no metadata update reaches the instance yet.
    assert!(p.feed(b"CHART 'test.c1' '' 'title' 'units' 'family' 'ctx.c1' line 1000 1 'hidden' fixture-pusher corpus\n"));
    assert!(!ri().flags.check(netdata_agent_rrd::contexts::flags::HIDDEN));
    // A changed label commits a metadata update, which syncs it.
    assert!(
        feed_all(&mut p, &["CLABEL 'k' 'v2' 2", "CLABEL_COMMIT"])
            .iter()
            .all(|&ok| ok)
    );
    assert!(ri().flags.check(netdata_agent_rrd::contexts::flags::HIDDEN));
}

/// The child entry of the C parent's first `JSON STREAM_PATH` reply in the brief's capture (`knowledge/
/// brief-stream-path.md` §2 in the status repository), as the child sent it.
const CAPTURED_CHILD_ENTRY: &str = r#"{"version":1,"hostname":"parity-cchild-none","host_id":"5a1e0000-0000-4000-8000-00000000c004","node_id":null,"claim_id":null,"hops":0,"since":1790360427,"first_time_t":1790360431,"start_time":0,"shutdown_time":0,"capabilities":["V1","V2","VN","VCAPS","HLABELS","CLAIM","CLABELS","FUNCTIONS","FUNCDEL","REPLICATION","BINARY","INTERPOLATED","IEEE754","DYNCFG","SLOTS","PROGRESS","NODEID","PATHS","FLOATBASELINE"],"flags":[]}"#;

/// The parent entry of that reply, with its retention start.
fn captured_parent_entry(first_time_t: i64) -> String {
    format!(
        r#"{{"version":1,"hostname":"parity-parent","host_id":"5a1e0000-0000-4000-8000-0000000000aa","node_id":null,"claim_id":null,"hops":1,"since":1790360450,"first_time_t":{first_time_t},"start_time":0,"shutdown_time":0,"capabilities":["V1","V2","VN","VCAPS","HLABELS","CLAIM","CLABELS","LZ4","FUNCTIONS","FUNCDEL","REPLICATION","BINARY","INTERPOLATED","IEEE754","DYNCFG","SLOTS","ZSTD","GZIP","BROTLI","PROGRESS","NODEID","PATHS","FLOATBASELINE"],"flags":[]}}"#
    )
}

/// A child host with a receiver that negotiated `capabilities`, and its parser.
fn stream_path_parser(capabilities: u32) -> (Arc<Host>, Parser) {
    let h = named_host(
        "parity-cchild-none",
        "5a1e0000-0000-4000-8000-00000000c004",
        false,
    );
    let link = netdata_agent_rrd::host::ReceiverLink {
        hops: 1,
        connected_since_s: 1_790_360_450,
        capabilities,
    };
    assert!(
        h.set_receiver(Arc::new(netdata_agent_rrd::host::ReceiverSlot::new(
            0,
            Default::default(),
            link,
            Box::new(|| {}),
        )))
    );
    let mut p = Parser::new(
        Arc::clone(&h),
        named_host(
            "parity-parent",
            "5a1e0000-0000-4000-8000-0000000000aa",
            true,
        ),
        Config {
            capabilities,
            update_every: 1,
            page_size: 4096,
            now: || (NOW, 0),
            gap_when_lost_iterations_above: 3,
        },
    );
    p.take_output();
    (h, p)
}

fn stream_path_block(body: &str) -> Vec<String> {
    vec![
        "JSON STREAM_PATH".to_string(),
        body.to_string(),
        "JSON_PAYLOAD_END".to_string(),
    ]
}

fn feed_strings(p: &mut Parser, lines: &[String]) -> Vec<bool> {
    let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
    feed_all(p, &lines)
}

/// The C child's negotiated capabilities in the capture (compression off, no ML models).
const CAPTURED_CAPS: u32 = caps::V1
    | caps::V2
    | caps::VN
    | caps::VCAPS
    | caps::HLABELS
    | caps::CLAIM
    | caps::CLABELS
    | caps::FUNCTIONS
    | caps::FUNCTION_DEL
    | caps::REPLICATION
    | caps::BINARY
    | caps::INTERPOLATED
    | caps::IEEE754
    | caps::DYNCFG
    | caps::SLOTS
    | caps::PROGRESS
    | caps::NODE_ID
    | caps::PATHS
    | caps::FLOAT_BASELINE;

/// The C parent's replies byte for byte: the child's entry verbatim, then the parent's own; a retention change sends
/// the same with the new retention start.
#[test]
fn a_changed_stream_path_goes_back_with_this_agent_appended() {
    let (h, mut p) = stream_path_parser(CAPTURED_CAPS);
    let body = format!(r#"{{"version":1,"streaming_path":[{CAPTURED_CHILD_ENTRY}]}}"#);
    assert_eq!(feed_strings(&mut p, &stream_path_block(&body)), [true; 3]);
    let reply = |first: i64| {
        format!(
            "JSON STREAM_PATH\n{{\"version\":1,\"streaming_path\":[{CAPTURED_CHILD_ENTRY},{}]}}\nJSON_PAYLOAD_END\n",
            captured_parent_entry(first)
        )
    };
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(0));
    assert_eq!(h.stream_path().len(), 1);
    // the same path again changes nothing and sends nothing
    feed_strings(&mut p, &stream_path_block(&body));
    assert!(p.take_output().is_empty());
    p.retention_updated(1_790_360_431);
    assert_eq!(
        String::from_utf8(p.take_output()).unwrap(),
        reply(1_790_360_431)
    );
    // the child's copy of this agent's entry is stored but replaced by the current one when sent
    let with_parent = format!(
        r#"{{"version":1,"streaming_path":[{},{CAPTURED_CHILD_ENTRY}]}}"#,
        captured_parent_entry(5)
    );
    feed_strings(&mut p, &stream_path_block(&with_parent));
    assert_eq!(h.stream_path().len(), 2);
    assert_eq!(h.stream_path()[0].hops, 0, "sorted by hops");
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(0));
}

/// A child that did not negotiate paths gets nothing back; the path is stored all the same.
#[test]
fn no_stream_path_goes_to_a_child_without_paths() {
    let (h, mut p) = stream_path_parser(CAPTURED_CAPS & !caps::PATHS);
    let body = format!(r#"{{"version":1,"streaming_path":[{CAPTURED_CHILD_ENTRY}]}}"#);
    feed_strings(&mut p, &stream_path_block(&body));
    assert!(p.take_output().is_empty());
    assert_eq!(h.stream_path().len(), 1);
    p.retention_updated(1);
    assert!(p.take_output().is_empty());
}

/// Text that is not JSON keeps the stored path; JSON without the member clears it (a change); an empty body does
/// nothing and logs nothing.
#[test]
fn stream_path_bodies_that_are_not_paths() {
    let (h, mut p) = stream_path_parser(CAPTURED_CAPS);
    let body = format!(r#"{{"version":1,"streaming_path":[{CAPTURED_CHILD_ENTRY}]}}"#);
    feed_strings(&mut p, &stream_path_block(&body));
    p.take_output();
    let (_, logged) = feed_logged(
        &mut p,
        &[
            "JSON STREAM_PATH",
            "{\"streaming_path\":[",
            "JSON_PAYLOAD_END",
        ],
    );
    assert_eq!(
        logged,
        ["STREAM PATH 'parity-cchild-none': Cannot parse json: {\"streaming_path\":[\n"]
    );
    assert!(p.take_output().is_empty());
    assert_eq!(h.stream_path().len(), 1);
    let (_, logged) = feed_logged(&mut p, &["JSON STREAM_PATH", "JSON_PAYLOAD_END"]);
    assert!(logged.is_empty(), "{logged:?}");
    assert!(p.take_output().is_empty());
    feed_all(
        &mut p,
        &["JSON STREAM_PATH", "{\"other\":1}", "JSON_PAYLOAD_END"],
    );
    assert!(h.stream_path().is_empty());
    assert_eq!(
        String::from_utf8(p.take_output()).unwrap(),
        format!(
            "JSON STREAM_PATH\n{{\"version\":1,\"streaming_path\":[{}]}}\nJSON_PAYLOAD_END\n",
            captured_parent_entry(0)
        )
    );
    // the receiver going away clears the path (stream_path_child_disconnected())
    feed_strings(&mut p, &stream_path_block(&body));
    let slot = h.receiver().unwrap();
    h.clear_receiver(&slot);
    assert!(h.stream_path().is_empty());
}

/// OVERWRITE keeps the ephemeral option in step with the `_is_ephemeral` label; the stream path entry shows it.
#[test]
fn overwrite_sets_the_ephemeral_option() {
    let (h, mut p) = stream_path_parser(CAPTURED_CAPS);
    feed_all(&mut p, &["LABEL '_is_ephemeral' 1 'yes'", "OVERWRITE"]);
    assert!(h.is_ephemeral());
    feed_all(&mut p, &["LABEL '_is_ephemeral' 1 'no'", "OVERWRITE"]);
    assert!(!h.is_ephemeral());
}
