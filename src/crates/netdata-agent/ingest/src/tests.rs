use std::sync::{Arc, Mutex};

use netdata_agent_rrd::host::{Host, HostInfo};
use netdata_agent_rrd::mode::DbMode;

use super::*;

/// The fixed wall clock of these tests.
const NOW: i64 = 1_700_000_000;

fn host() -> Arc<Host> {
    let mut info = HostInfo {
        hostname: "child".into(),
        registry_hostname: "child".into(),
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
    };
    info.set_replication(true, 86400, 3600);
    Arc::new(Host::new("guid", false, info))
}

fn parser(host: &Arc<Host>) -> (Parser, Arc<Mutex<Vec<String>>>) {
    let logs = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&logs);
    let parser = Parser::new(
        Arc::clone(host),
        Config {
            capabilities: 0,
            update_every: 1,
            page_size: 4096,
            now: || (NOW, 0),
            gap_when_lost_iterations_above: 3,
        },
        Box::new(move |_, m| sink.lock().unwrap().push(m.to_string())),
    );
    (parser, logs)
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
    let (mut p, _) = parser(&h);
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
    let (mut p, _) = parser(&h);
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

#[test]
fn replication_rows_are_stored_and_rend_finishes() {
    let h = host();
    let (mut p, _) = parser(&h);
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
        let (mut p, logs) = parser(&h);
        let results = feed_all(&mut p, lines);
        assert_eq!(results.last(), Some(&false), "{lines:?}");
        assert!(
            logs.lock()
                .unwrap()
                .last()
                .unwrap()
                .contains("parser_action("),
            "{lines:?}"
        );
    }
    // Blank lines and deferred JSON bodies never reach the keyword table.
    let h = host();
    let (mut p, _) = parser(&h);
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

/// The wall clock of `v1_collection_times_keep_their_microseconds`, in microseconds, advanced by the test.
static CLOCK_UT: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

#[test]
fn v1_collection_times_keep_their_microseconds() {
    use std::sync::atomic::Ordering::Relaxed;
    let h = host();
    let (mut p, _) = parser(&h);
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
    let (mut p, _) = parser(&h);
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
    let (mut p, logs) = parser(&h);
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
    assert!(feed_all(&mut p, &lines).iter().all(|&ok| ok));
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
    let logs = logs.lock().unwrap();
    assert!(
        logs.iter()
            .any(|l| l.contains("refusing to unregister dyncfg method 'config'"))
    );
    assert!(
        logs.iter().any(|l| l
            == "got a FUNCTION_PROGRESS for transaction 'abc', but the transaction is not found.")
    );

    // The name, timeout and help are required.
    let (mut p, logs) = parser(&h);
    assert!(!p.feed(b"FUNCTION GLOBAL \"x\" 10\n"));
    assert!(logs.lock().unwrap()[0].contains("without providing the required data (global = 'yes', name = 'x', timeout = '10', priority = '(unset)', version = '(unset)', help = '(unset)')"));
}

#[test]
fn a_label_change_resyncs_the_instance_hidden_flag() {
    let h = host();
    let (mut p, _) = parser(&h);
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
