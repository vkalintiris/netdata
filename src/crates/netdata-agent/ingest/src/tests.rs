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
    parser_with(host, 0)
}

fn parser_with(host: &Arc<Host>, capabilities: u32) -> Parser {
    Parser::new(
        Arc::clone(host),
        named_host("parent", "5a1e0000-0000-4000-8000-0000000000aa", true),
        Config {
            capabilities,
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
    let chart = h.charts().find("test.c1", true).unwrap();
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

/// The sender's lines (`pluginsd_proto::emit::stream`), plain, with slots and IEEE754, and with float baselines,
/// define and fill a chart through this parser as a C child's lines do.
#[test]
fn the_senders_lines_round_trip() {
    use netdata_agent_pluginsd_proto::caps;
    use netdata_agent_pluginsd_proto::emit::stream::*;
    for capabilities in [
        caps::INTERPOLATED,
        caps::INTERPOLATED | caps::SLOTS | caps::IEEE754,
        caps::INTERPOLATED | caps::SLOTS | caps::IEEE754 | caps::FLOAT_BASELINE,
    ] {
        let h = host();
        let mut p = parser_with(&h, capabilities);
        let e = Enc::live(capabilities);
        let mut out = Vec::new();
        let def = ChartDef {
            slot: 3,
            id: "test.rt",
            name: chart_name("test.rt", Some("test.named")),
            title: "t t",
            units: "u",
            family: "f",
            context: "ctx.rt",
            chart_type: "stacked",
            priority: 7,
            update_every: 1,
            obsolete: false,
            store_first: false,
            hidden: false,
            plugin: "pl",
            module: "mo",
        };
        chart(&mut out, &e, &def);
        clabel(&mut out, b"k", b"v", 2);
        clabel_commit(&mut out);
        let a = DimDef {
            slot: 1,
            id: "a",
            name: "a",
            algorithm: "absolute",
            multiplier: 1,
            divisor: 1,
            obsolete: false,
            hidden: false,
            noreset: false,
            float: false,
        };
        dimension(&mut out, &e, &a);
        dimension(&mut out, &e, &DimDef { slot: 2, id: "b", name: "bee", ..a });
        let t = NOW - 10;
        let mut block = V2Block::new(e, 3, "test.rt", 1, NOW);
        block.set2(&mut out, t, 1, "a", Baseline::Int(5), 5.0, b"A");
        block.set2(&mut out, t, 2, "b", Baseline::Int(-7), -7.5, b"AR");
        end2(&mut out);
        let text = String::from_utf8(out).unwrap();
        for line in text.lines() {
            assert!(p.feed(format!("{line}\n").as_bytes()), "{capabilities:#x}: {line}");
        }
        let chart = h.charts().find("test.rt", true).unwrap();
        let meta = chart.meta();
        assert_eq!(meta.name.as_deref(), Some("test.named"), "{text}");
        assert_eq!((meta.context.as_str(), meta.priority), ("ctx.rt", 7));
        assert_eq!(meta.labels.get(b"k"), Some(&b"v"[..]));
        assert_eq!(chart.dim("b").unwrap().meta().name, "bee");
        for (id, value) in [("a", 5.0), ("b", -7.5)] {
            let dim = chart.dim(id).unwrap();
            let mut q = dim.ring().unwrap().query(t, t);
            assert_eq!(q.next_metric().sum, value, "{capabilities:#x} {id}: {text}");
        }
    }
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

/// On a parent with the BACKFILL pool running, the first `CHART_DEFINITION_END` of a chart queues its dimensions'
/// backfill and sends nothing; the chart's last job hands the request to the stream thread's sink, which answers as
/// the inline path does. The chart is never queued again: after a reconnect the request goes out at once.
#[test]
fn chart_definition_end_waits_for_the_backfill() {
    let h = host();
    let slot = Arc::new(netdata_agent_rrd::host::ReceiverSlot::new(
        0,
        Default::default(),
        netdata_agent_rrd::host::ReceiverLink::default(),
        Box::new(|| {}),
    ));
    assert_eq!(h.set_receiver(Arc::clone(&slot)), netdata_agent_rrd::host::Attach::Attached);
    let queue = h.storage().backfill_queue();
    queue.start();
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let mut p = parser(&h);
    let sink = Arc::clone(&asked);
    p.set_replay_sink(Arc::new(move |r: ReplayRequest| {
        sink.lock().unwrap().push(r);
        true
    }));
    feed_all(&mut p, &DEFINE);
    let first = NOW - 100;
    assert!(p.feed(format!("CHART_DEFINITION_END {first} {NOW} {NOW}\n").as_bytes()));
    let chart = h.charts().find("test.c1", true).unwrap();
    assert!(p.take_output().is_empty());
    assert_eq!(
        (
            chart.flags() & flags::BACKFILLED_HIGH_TIERS != 0,
            h.backfill_pending(),
            queue.queued()
        ),
        (true, 1, chart.dim_count())
    );
    queue.worker(false, &|| queue.queued() > 0);
    let requests = std::mem::take(&mut *asked.lock().unwrap());
    assert_eq!(requests.len(), 1);
    assert_eq!(h.backfill_pending(), 0);
    p.replay_backfilled(&requests[0]);
    let want = format!("REPLAY_CHART \"test.c1\" \"true\" {first} {NOW}\n");
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), want);
    // the child comes back: the chart was queued once, so the new round asks at once
    h.clear_receiver(&slot, 0);
    assert_eq!(
        h.set_receiver(Arc::new(netdata_agent_rrd::host::ReceiverSlot::new(
            0,
            Default::default(),
            netdata_agent_rrd::host::ReceiverLink::default(),
            Box::new(|| {}),
        ))),
        netdata_agent_rrd::host::Attach::Attached
    );
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    assert!(p.feed(format!("CHART_DEFINITION_END {first} {NOW} {NOW}\n").as_bytes()));
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), want);
    assert_eq!(
        (
            queue.queued(),
            h.backfill_pending(),
            asked.lock().unwrap().len()
        ),
        (0, 0, 0)
    );
    queue.finish();
}

/// A backfill answer for a receiver that went is refused: the request is not handed to the sink.
#[test]
fn a_backfill_answer_after_a_reconnect_is_dropped() {
    let h = host();
    let slot = Arc::new(netdata_agent_rrd::host::ReceiverSlot::new(
        0,
        Default::default(),
        netdata_agent_rrd::host::ReceiverLink::default(),
        Box::new(|| {}),
    ));
    assert_eq!(h.set_receiver(Arc::clone(&slot)), netdata_agent_rrd::host::Attach::Attached);
    let queue = h.storage().backfill_queue();
    queue.start();
    let asked = Arc::new(std::sync::Mutex::new(0));
    let mut p = parser(&h);
    let sink = Arc::clone(&asked);
    p.set_replay_sink(Arc::new(move |_| {
        *sink.lock().unwrap() += 1;
        true
    }));
    feed_all(&mut p, &DEFINE);
    assert!(p.feed(format!("CHART_DEFINITION_END {} {NOW} {NOW}\n", NOW - 100).as_bytes()));
    h.clear_receiver(&slot, 0);
    let ((), records) = netdata_agent_log::capture(|| queue.worker(false, &|| queue.queued() > 0));
    assert_eq!(*asked.lock().unwrap(), 0);
    assert!(records.iter().any(|r| r.message.as_deref()
        == Some("PLUGINSD REPLAY ERROR: 'host:child' failed to acquire host for sending replication command for \
                 'chart:test.c1'")));
    queue.finish();
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
        assert_eq!(h.set_receiver(Arc::clone(&slot)), netdata_agent_rrd::host::Attach::Attached);
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
    h.clear_receiver(&slot, 0);
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
    let chart = h.charts().find("test.c1", true).unwrap();
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

/// SET2 keeps the child's value as the dimension's last collected one, in its type's lane, which END2's reset of the
/// collected values leaves alone; RDSTATE writes the same lanes (C's `rrddim_set_last_collected_*()`, D105.11).
#[test]
fn set2_and_rdstate_keep_the_last_collected_values() {
    let h = host();
    let mut p = parser(&h);
    let define = [DEFINE[0], DEFINE[1], "DIMENSION 'f1' '' absolute 1 1 'type=float'"];
    assert!(feed_all(&mut p, &define).iter().all(|&ok| ok));
    let t = NOW - 10;
    let lines = [
        format!("BEGIN2 'test.c1' 1 {t} #"),
        "SET2 'd1' 5 5 A".to_string(),
        "SET2 'f1' 7 7 A".to_string(),
        "END2".to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    let chart = h.charts().find("test.c1", true).unwrap();
    let lanes = |id: &str| {
        let c = chart.dim(id).unwrap().collection();
        (c.last_collected_value, c.last_collected_value_float, c.collected_value, c.collected_value_float)
    };
    assert_eq!((lanes("d1"), lanes("f1")), ((5, 0.0, 0, 0.0), (0, 7.0, 0, 0.0)));
    let (s, e) = (NOW - 20, NOW - 19);
    let lines = [
        "RBEGIN 'test.c1'".to_string(),
        format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
        format!("RDSTATE 'd1' {} 9 9 9", e * 1_000_000),
        format!("RDSTATE 'f1' {} 11 11 11", e * 1_000_000),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    assert_eq!((lanes("d1"), lanes("f1")), ((9, 0.0, 0, 0.0), (0, 11.0, 0, 0.0)));
}

/// `rrdhost_receiver_replicating_charts()` in the ingest: a chart's first `CHART_DEFINITION_END` of a round counts it
/// and puts the host's receiver in replicating; the REND that starts its streaming takes it back, puts the receiver
/// in running and the completion at 100%; a REND on a chart not replicating takes nothing back.
#[test]
fn chart_replications_are_counted_on_the_host() {
    use netdata_agent_rrd::pulse::host_status::{RCV_REPLICATING, RCV_RUNNING, RECEIVER};
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let counted = |h: &Host| (h.replicating_charts(), h.pulse_state() & RECEIVER);
    let end = format!("CHART_DEFINITION_END {} {NOW} {NOW}", NOW - 100);
    feed_all(&mut p, &[&end, &end]);
    assert_eq!(counted(&h), (1, RCV_REPLICATING));
    h.set_replication_percent(42.0);
    let (s, e) = (NOW - 20, NOW - 19);
    let lines = [
        "RBEGIN 'test.c1'".to_string(),
        format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
        format!("REND 1 {} {} true {} {} 0x{:x}", NOW - 100, NOW, s, e, NOW),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    assert_eq!(
        (counted(&h), h.replication_percent()),
        ((0, RCV_RUNNING), 100.0)
    );
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    assert_eq!(counted(&h), (0, RCV_RUNNING));
}

/// The stuck replication loop: a parent that holds the child's last entry and keeps getting RENDs that bring nothing
/// (no RBEGIN window) forces the chart's replication to finish at the third in a row, taking it back from the
/// host's count, as a REND that starts its streaming does.
#[test]
fn a_stuck_replication_is_taken_back() {
    use netdata_agent_rrd::pulse::host_status::{RCV_RUNNING, RECEIVER};
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    feed_all(
        &mut p,
        &[&format!("CHART_DEFINITION_END {} {NOW} {NOW}", NOW - 100)],
    );
    let (s, e) = (NOW - 20, NOW - 19);
    let rend = format!("REND 1 {} {e} false {s} {e} 0x{:x}", NOW - 100, NOW);
    let data = [
        "RBEGIN 'test.c1'".to_string(),
        format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
        "RSET 'd1' 7 A".to_string(),
        format!("RSSTATE {0} {0}", e * 1_000_000),
        rend.clone(),
    ];
    let empty = ["RBEGIN 'test.c1'".to_string(), rend];
    let feed = |p: &mut Parser, lines: &[String]| {
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(p, &refs).iter().all(|&ok| ok));
    };
    feed(&mut p, &data);
    feed(&mut p, &empty);
    assert_eq!(h.replicating_charts(), 1);
    h.set_replication_percent(42.0);
    feed(&mut p, &empty);
    assert_eq!(
        (
            h.replicating_charts(),
            h.pulse_state() & RECEIVER,
            h.replication_percent()
        ),
        (0, RCV_RUNNING, 100.0)
    );
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
    let d1 = h.charts().find("test.c1", true).unwrap().dim("d1").unwrap();
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
    let chart = h.charts().find("test.c1", true).unwrap();
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
            .find("test.c1", true)
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
    assert_eq!(
        h.set_receiver(Arc::new(netdata_agent_rrd::host::ReceiverSlot::new(
            0,
            Default::default(),
            link,
            Box::new(|| {}),
        ))),
        netdata_agent_rrd::host::Attach::Attached
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
    h.clear_receiver(&slot, 0);
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

/// A BEGIN2 that follows a BEGIN2 without its END2, of the same chart and then of another, finds the collection lock
/// still held: it is released with C's record naming the chart it was taken for; END2 releases it without one.
#[test]
fn a_begin2_without_end2_reports_the_stale_lock() {
    let h = host();
    let mut p = parser(&h);
    let define = [DEFINE[0], DEFINE[1], "CHART 'test.c2' '' 't' 'u' 'f' 'ctx.c2' line 1000 1 '' p m", DEFINE[1]];
    assert!(feed_all(&mut p, &define).iter().all(|&ok| ok));
    let t = NOW - 10;
    let (ok, records) = feed_logged(
        &mut p,
        &[
            &format!("BEGIN2 'test.c1' 1 {t} #"),
            "SET2 'd1' 1 1 A",
            &format!("BEGIN2 'test.c1' 1 {} #", t + 1),
            "SET2 'd1' 2 2 A",
            "END2",
            &format!("BEGIN2 'test.c1' 1 {} #", t + 2),
            &format!("BEGIN2 'test.c2' 1 {} #", t + 2),
            "END2",
        ],
    );
    assert!(ok.iter().all(|&ok| ok));
    let stale = "PLUGINSD: 'host:child/chart:test.c1/' stale data collection lock found during BEGIN2; it has been \
                 unlocked";
    assert_eq!(records, [stale, stale]);
}

/// The collection lock is held from BEGIN2 to END2 across reads: a replication answer for the chart waits for the
/// block's end (D106.4).
#[test]
fn the_collection_lock_spans_a_block() {
    let h = host();
    let mut p = parser(&h);
    assert!(feed_all(&mut p, &DEFINE).iter().all(|&ok| ok));
    let chart = h.charts().find("test.c1", true).unwrap();
    assert!(feed_all(&mut p, &[&format!("BEGIN2 'test.c1' 1 {} #", NOW - 10), "SET2 'd1' 1 1 A"])[0]);
    let acquired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let contender = {
        let (chart, acquired) = (Arc::clone(&chart), Arc::clone(&acquired));
        std::thread::spawn(move || {
            let _guard = Chart::lock_collection(chart.as_ref());
            acquired.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    };
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(!acquired.load(std::sync::atomic::Ordering::SeqCst), "taken inside the block");
    assert!(feed_all(&mut p, &["END2"])[0]);
    contender.join().unwrap();
    assert!(acquired.load(std::sync::atomic::Ordering::SeqCst));
}

// ---- the proxy (milestone 7 commit 8f, D117) ----

use netdata_agent_rrd::host::{StreamSend, sender_flags};
use netdata_agent_rrd::testing::Recorder;
use netdata_agent_rrd::upstream::{Traffic, Upstream};

/// The proxied chart, as a child without SLOTS defines it.
const PROXIED: [&str; 3] = [
    "CHART 'proxy.gauge' '' 'title' 'units' 'family' 'proxy.gauge' line 1000 1 '' fixture-pusher corpus",
    "DIMENSION 'g1' '' absolute 1 1 ''",
    "DIMENSION 'g2' '' absolute 1 1 ''",
];

/// A child without IEEE754.
const CHILD: u32 = caps::INTERPOLATED | caps::FLOAT_BASELINE;
/// A parent that refused IEEE754.
const PARENT: u32 = caps::INTERPOLATED | caps::SLOTS | caps::FLOAT_BASELINE;

/// A child host whose proxy settings stream it with `pattern`, its sender (a recorder) ready with `parent_caps`, and
/// its receiver's parser with `child_caps`.
fn proxied(pattern: &str, child_caps: u32, parent_caps: u32) -> (Arc<Host>, Arc<Recorder>, Parser) {
    let h = named_host("child", "guid", false);
    let mut info = h.info();
    info.stream_send = StreamSend::new(true, "grandparent:19999", "key", pattern);
    let h = Arc::new(Host::new("guid", false, info));
    let r = Arc::new(Recorder::with_capabilities(parent_caps));
    h.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
    h.sender_flags_set(sender_flags::ADDED | sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
    let mut p = parser_with(&h, child_caps);
    assert!(feed_all(&mut p, &PROXIED).iter().all(|&ok| ok));
    (h, r, p)
}

fn feed_ok(p: &mut Parser, lines: &[String]) {
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(p, &refs).iter().all(|&ok| ok), "{lines:?}");
}

/// What reached the parent so far, after a marker committed through the batch sends what it holds.
fn upstream_bytes(p: &mut Parser, r: &Recorder) -> Vec<(Traffic, String)> {
    p.forward.start(r).extend_from_slice(b"M\n");
    p.forward.commit(r, Traffic::Metadata);
    r.take()
}

fn block(t: i64) -> Vec<String> {
    vec![format!("BEGIN2 'proxy.gauge' 1 {t} #"), "SET2 'g1' 1 1 A".into(), "SET2 'g2' 2 # RA".into(), "END2".into()]
}

/// A child's block goes on in the parent's slots, the words copied when both links read numbers alike, else
/// re-encoded; the definition goes first and the block waits in the batch.
#[test]
fn a_proxied_block_goes_on_as_a_c_proxy() {
    let t = NOW - 10;
    let (_h, r, mut p) = proxied("*", CHILD, PARENT);
    feed_ok(&mut p, &block(t));
    let commits = r.take();
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert!(commits[0].1.starts_with("CHART SLOT:0x1 \"proxy.gauge\" "), "{commits:?}");
    assert_eq!(
        upstream_bytes(&mut p, &r),
        vec![(
            Traffic::Metadata,
            format!(
                "BEGIN2 SLOT:0x1 'proxy.gauge' 1 {t} #\nSET2 SLOT:0x1 'g1' 1 1 A\nSET2 SLOT:0x2 'g2' 2 # AR\nEND2\nM\n"
            )
        )]
    );
    // a parent with IEEE754: base64, the wall clock and the value explicit
    let (_h, r, mut p) = proxied("*", CHILD, PARENT | caps::IEEE754);
    feed_ok(&mut p, &block(1));
    r.take();
    assert_eq!(
        upstream_bytes(&mut p, &r)[0].1,
        "BEGIN2 SLOT:#B 'proxy.gauge' #B #B #B\nSET2 SLOT:#B 'g1' #B @D/wAAAAAAAA A\n\
         SET2 SLOT:#C 'g2' #C @EAAAAAAAAAA AR\nEND2\nM\n"
    );
}

/// A parent without INTERPOLATED gets v1 at END2 with the collected values, which SET2 never sets (D106.3): zeros,
/// every dimension still there (the dimensions are reset only after the finish).
#[test]
fn a_v1_parent_gets_zeros() {
    let (_h, r, mut p) = proxied("*", CHILD, PARENT & !caps::INTERPOLATED);
    feed_ok(&mut p, &block(NOW - 10));
    r.take();
    assert_eq!(
        upstream_bytes(&mut p, &r),
        vec![(Traffic::Metadata, "BEGIN \"proxy.gauge\" 0\nSET \"g1\" = 0\nSET \"g2\" = 0\nEND\nM\n".to_string())]
    );
}

/// A BEGIN2 without END2 closes the open block before the next BEGIN2, and another chart's block goes on under the
/// first chart's gate, one DATA commit for both.
#[test]
fn a_begin2_without_end2_closes_the_forwarded_block() {
    let (h, r, mut p) = proxied("*", CHILD, PARENT);
    let mut other: Vec<String> = PROXIED.iter().map(|l| l.replace("proxy.gauge", "proxy.other")).collect();
    other.extend(block(NOW - 20).into_iter().map(|l| l.replace("proxy.gauge", "proxy.other")));
    feed_ok(&mut p, &other);
    feed_ok(&mut p, &block(NOW - 20));
    r.take();
    upstream_bytes(&mut p, &r);
    let t = NOW - 10;
    let lines = [
        format!("BEGIN2 'proxy.gauge' 1 {t} #"),
        "SET2 'g1' 1 1 A".to_string(),
        format!("BEGIN2 'proxy.gauge' 1 {} #", t + 1),
        "SET2 'g1' 2 2 A".to_string(),
        format!("BEGIN2 'proxy.other' 1 {} #", t + 1),
        "SET2 'g1' 3 3 A".to_string(),
        "END2".to_string(),
    ];
    let (_, records) = netdata_agent_log::capture(|| feed_ok(&mut p, &lines));
    assert_eq!(records.len(), 2, "the stale lock twice: {records:?}");
    let other_slot = h.charts().find("proxy.other", true).unwrap().chart_slot();
    assert_eq!(
        upstream_bytes(&mut p, &r),
        vec![(
            Traffic::Metadata,
            format!(
                "BEGIN2 SLOT:0x1 'proxy.gauge' 1 {t} #\nSET2 SLOT:0x1 'g1' 1 1 A\nEND2\n\
                 BEGIN2 SLOT:0x1 'proxy.gauge' 1 {} #\nSET2 SLOT:0x1 'g1' 2 2 A\nEND2\n\
                 BEGIN2 SLOT:0x{other_slot:X} 'proxy.other' 1 {} #\nSET2 SLOT:0x1 'g1' 3 3 A\nEND2\nM\n",
                t + 1,
                t + 1
            )
        )]
    );
}

/// D116.2: a v1 collection inside an open forwarded block (C fatal()s) closes and commits the block first, so every
/// BEGIN2 upstream has its END2.
#[test]
fn a_v1_end_inside_a_forwarded_block_closes_it_first() {
    let (_h, r, mut p) = proxied("*", CHILD, PARENT);
    let t = NOW - 10;
    let lines = [
        format!("BEGIN2 'proxy.gauge' 1 {t} #"),
        "SET2 'g1' 1 1 A".to_string(),
        "BEGIN 'proxy.gauge'".to_string(),
        "SET 'g1' = 4".to_string(),
        format!("END {} 0", t + 1),
    ];
    let _ = netdata_agent_log::capture(|| feed_ok(&mut p, &lines));
    r.take();
    let sent = upstream_bytes(&mut p, &r);
    assert!(
        sent[0].1.starts_with(&format!("BEGIN2 SLOT:0x1 'proxy.gauge' 1 {t} #\nSET2 SLOT:0x1 'g1' 1 1 A\nEND2\n")),
        "{sent:?}"
    );
    assert_eq!(sent[0].1.matches("BEGIN2").count(), sent[0].1.matches("END2").count(), "{sent:?}");
}

/// The close of D116.2 writes END2 only: another chart's variables are not attached to the block (R47 n1).
#[test]
fn a_v1_end_of_another_chart_leaves_the_blocks_variables_alone() {
    let (_h, r, mut p) = proxied("*", CHILD, PARENT);
    let other: Vec<String> = PROXIED.iter().map(|l| l.replace("proxy.gauge", "proxy.other")).collect();
    feed_ok(&mut p, &other);
    let t = NOW - 20;
    let mut lines = block(t);
    lines.push("VARIABLE CHART av = 1".into());
    lines.extend(block(t).into_iter().map(|l| l.replace("proxy.gauge", "proxy.other")));
    lines.push("VARIABLE CHART bv = 2".into());
    feed_ok(&mut p, &lines);
    upstream_bytes(&mut p, &r);
    let lines = [
        format!("BEGIN2 'proxy.gauge' 1 {} #", t + 1),
        "SET2 'g1' 1 1 A".to_string(),
        "BEGIN 'proxy.other'".to_string(),
        "SET 'g1' = 4".to_string(),
        format!("END {} 0", t + 2),
    ];
    let _ = netdata_agent_log::capture(|| feed_ok(&mut p, &lines));
    let sent = upstream_bytes(&mut p, &r);
    assert!(
        sent[0].1.starts_with(&format!(
            "BEGIN2 SLOT:0x1 'proxy.gauge' 1 {} #\nSET2 SLOT:0x1 'g1' 1 1 A\nEND2\n",
            t + 1
        )),
        "{sent:?}"
    );
}

/// A v1 child's collections go through the batch too: 100 held, the 101st sends them.
#[test]
fn v1_collections_are_batched() {
    let (_h, r, mut p) = proxied("*", CHILD, PARENT);
    let collect = |p: &mut Parser, i: i64| {
        feed_ok(p, &["BEGIN 'proxy.gauge'".to_string(), "SET 'g1' = 1".to_string(), format!("END {} 0", NOW - 200 + i)])
    };
    for i in 0..100 {
        collect(&mut p, i);
    }
    let commits = r.take();
    assert_eq!(commits.len(), 1, "the definition only: {commits:?}");
    collect(&mut p, 100);
    let commits = r.take();
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert_eq!(commits[0].0, Traffic::Data);
    assert!(commits[0].1.starts_with("BEGIN2 SLOT:0x1 'proxy.gauge' "), "{commits:?}");
}

/// A chart the pattern excludes goes nowhere; a child not proxied forwards nothing.
#[test]
fn filtered_and_unproxied_charts_forward_nothing() {
    let (_h, r, mut p) = proxied("!proxy.gauge *", CHILD, PARENT);
    feed_ok(&mut p, &block(NOW - 10));
    assert_eq!(upstream_bytes(&mut p, &r), vec![(Traffic::Metadata, "M\n".to_string())]);
    let h = host();
    let mut p = parser_with(&h, CHILD);
    assert!(feed_all(&mut p, &PROXIED).iter().all(|&ok| ok));
    feed_ok(&mut p, &block(NOW - 10));
    assert!(p.forward.bytes().is_empty());
}
