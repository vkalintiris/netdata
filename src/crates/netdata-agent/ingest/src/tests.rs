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
    stream_parser_on(host, capabilities, Arc::new(TestWire::default()))
}

/// A child's stream parser on `host`, whose calls go to the child through `wire`.
fn stream_parser_on(host: &Arc<Host>, capabilities: u32, wire: Arc<TestWire>) -> Parser {
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
        wire,
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

/// A SLOT over the cap is warned about, as C's `pluginsd_parse_rrd_slot()` (D126.5), and the chart is found by its
/// id; `SLOT:0` is a slot like any other, with no record.
#[test]
fn an_over_cap_slot_is_warned_about_as_c() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let t = NOW - 10;
    let (oks, records) = netdata_agent_log::capture(|| {
        let mut oks = feed_all(&mut p, &[&format!("BEGIN2 SLOT:2000000 'test.c1' 1 {t} #"), "SET2 'd1' 5 5 A", "END2"]);
        oks.extend(feed_all(&mut p, &[&format!("BEGIN2 SLOT:0 'test.c1' 1 {} #", t + 1), "SET2 'd1' 6 6 A", "END2"]));
        oks
    });
    assert!(oks.iter().all(|&ok| ok));
    assert_eq!(h.charts().find("test.c1", true).unwrap().dim("d1").unwrap().ring().unwrap().latest_time_s(), t + 1);
    let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(
        records,
        vec![(
            Source::Collector,
            Priority::Warning,
            "PLUGINSD: ignoring invalid SLOT value '2000000' above the supported maximum 1000000".to_string()
        )]
    );
}

/// A child's last entry after its wall clock is logged before the clamp, with the original value; a later check that
/// fails then logs the clamped one, " (fixed)" (R51 m1).
#[test]
fn the_clamp_is_logged_before_and_after_as_c() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let (first, last) = (NOW + 10, NOW + 50);
    let (ok, records) = netdata_agent_log::capture(|| {
        p.feed(format!("CHART_DEFINITION_END {first} {last} {NOW}\n").as_bytes())
    });
    assert!(ok);
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), "REPLAY_CHART \"test.c1\" \"true\" 0 0\n");
    let messages: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
    let head = |last: String, issue: &str| {
        format!(
            "STREAM SND REPLAY ERROR: 'host:{}/chart:test.c1' child sent: db from {first} to {last}, wall clock time \
             {NOW}, last request from 0 to 0, issue: {issue} - sending replication request from 0 to 0, start \
             streaming true",
            h.hostname()
        )
    };
    assert_eq!(
        messages,
        vec![
            head(last.to_string(), "child's db last entry > child's wall clock time"),
            head(
                format!("{NOW} (fixed)"),
                "sending empty replication request, child db first entry is after its wall clock time"
            ),
        ]
    );
}

/// A `CHART_DEFINITION_END` whose first entry is after its last sends the empty request with C's NOTICE
/// (`replicate_log_request()`, D126.4): the child's times, the issue, and the empty request C names.
#[test]
fn a_bad_replication_request_is_logged_as_c() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let (first, last) = (NOW - 50, NOW - 100);
    let (ok, records) = netdata_agent_log::capture(|| {
        p.feed(format!("CHART_DEFINITION_END {first} {last} {NOW}\n").as_bytes())
    });
    assert!(ok);
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), "REPLAY_CHART \"test.c1\" \"true\" 0 0\n");
    let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(
        records,
        vec![(
            Source::Daemon,
            Priority::Notice,
            format!(
                "STREAM SND REPLAY ERROR: 'host:{}/chart:test.c1' child sent: db from {first} to {last}, wall clock time \
                 {NOW}, last request from 0 to 0, issue: sending empty replication request, child timings are invalid \
                 (first entry > last entry) - sending replication request from 0 to 0, start streaming true",
                h.hostname()
            )
        )]
    );
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

/// A chart `t.<id>` with dimension 'd', with a slot or options when given.
fn slotted_chart(id: &str, slot: Option<u32>, options: &str) -> [String; 2] {
    let slot = slot.map_or(String::new(), |s| format!("SLOT:{s} "));
    [
        format!("CHART {slot}'t.{id}' '' 'title' 'units' 'family' 'ctx.{id}' line 1000 1 '{options}' p m"),
        "DIMENSION 'd' '' absolute 1 1 ''".to_string(),
    ]
}

/// One block for `t.<id>` with a slot, collecting `value` into 'd'.
fn slotted_block(id: &str, slot: u32, value: i64) -> [String; 3] {
    [
        format!("BEGIN2 SLOT:{slot} 't.{id}' 1 {} #", NOW - 10 + value),
        format!("SET2 'd' {value} {value} A"),
        "END2".to_string(),
    ]
}

/// The value 'd' of each chart last collected.
fn d_values(h: &Host, ids: &[&str]) -> Vec<i64> {
    ids.iter()
        .map(|id| {
            let chart = h.charts().find(&format!("t.{id}"), true).unwrap();
            chart.dim("d").unwrap().collection().last_collected_value
        })
        .collect()
}

/// The receive slot cache is the host's, as C's (D128.2): it grows to 1024 entries at its first slot, a slot inside
/// it that holds no chart caches the one found by id, and a later block with that slot lands on that chart whatever
/// id it names (a release build compares none); within one connection and across a reconnect.
#[test]
fn a_cached_slot_wins_over_the_id_it_names() {
    let h = host();
    let mut p = parser(&h);
    let charts = [slotted_chart("a", Some(1), "x"), slotted_chart("b", None, "x"), slotted_chart("c", None, "x")];
    feed_ok(&mut p, &charts.concat());
    feed_ok(&mut p, &[slotted_block("b", 5, 1), slotted_block("c", 5, 2)].concat());
    assert_eq!(d_values(&h, &["a", "b", "c"]), [0, 2, 0]);
    // the next connection: the accept marks every chart obsolete (each unslotted), the cache keeps its size
    drop(p);
    h.obsolete_all_charts();
    let mut p = parser(&h);
    feed_ok(&mut p, &[slotted_block("c", 9, 3), slotted_block("b", 9, 4)].concat());
    assert_eq!(d_values(&h, &["a", "b", "c"]), [0, 2, 4]);
}

/// An accept's obsolete-all clears every chart's slot: a slot from the last connection names nothing in the next.
#[test]
fn obsolete_charts_leave_their_slots() {
    let h = host();
    let mut p = parser(&h);
    feed_ok(&mut p, &[slotted_chart("a", Some(7), "x"), slotted_chart("b", None, "x")].concat());
    drop(p);
    h.obsolete_all_charts();
    let mut p = parser(&h);
    feed_ok(&mut p, &slotted_block("b", 7, 1));
    assert_eq!(d_values(&h, &["a", "b"]), [0, 1]);
}

/// An obsolete chart cached by its CHART line is unslotted when the scope moves on (`cleanup_slots`).
#[test]
fn an_obsolete_chart_is_unslotted_when_its_scope_ends() {
    let h = host();
    let mut p = parser(&h);
    let lines = [
        slotted_chart("a", Some(7), "obsolete"),
        slotted_chart("b", Some(8), "x"),
        slotted_chart("c", None, "x"),
    ];
    feed_ok(&mut p, &lines.concat());
    feed_ok(&mut p, &slotted_block("c", 7, 1));
    assert_eq!(d_values(&h, &["a", "b", "c"]), [0, 0, 1]);
}

/// A slotted DIMENSION marked obsolete sets the parser's `cleanup_slots` (`pluginsd_rrddim_put_to_slot()`): the next
/// scope change unslots the chart in scope, so a later block with its slot finds the chart it names.
#[test]
fn an_obsolete_slotted_dimension_unslots_its_chart_at_the_next_scope() {
    let h = host();
    let mut p = parser(&h);
    let mut lines = slotted_chart("a", Some(7), "x").to_vec();
    lines.push("DIMENSION SLOT:2 'o' '' absolute 1 1 'obsolete'".to_string());
    lines.extend(slotted_chart("b", Some(8), "x"));
    lines.extend(slotted_chart("c", None, "x"));
    feed_ok(&mut p, &lines);
    feed_ok(&mut p, &slotted_block("c", 7, 1));
    assert_eq!(d_values(&h, &["a", "b", "c"]), [0, 0, 1]);
}

/// A block whose empty slot finds an obsolete chart sets `cleanup_slots` (`pluginsd_rrdset_cache_get_from_slot()`),
/// which its own scope change applies to the chart in scope before it: that chart leaves its slot.
#[test]
fn a_lookup_finding_an_obsolete_chart_unslots_the_previous_scope() {
    let h = host();
    let mut p = parser(&h);
    let lines =
        [slotted_chart("p", Some(7), "x"), slotted_chart("x", None, "obsolete"), slotted_chart("q", None, "x")].concat();
    feed_ok(&mut p, &lines);
    feed_ok(&mut p, &[slotted_block("p", 7, 1), slotted_block("x", 5, 2), slotted_block("q", 7, 3)].concat());
    assert_eq!(d_values(&h, &["p", "x", "q"]), [1, 2, 3]);
}

/// The flush of an archived host frees its receive slot cache (`rrdhost_pluginsd_receive_chart_slots_free()`):
/// after it, a slot is outside the cache, so blocks find their charts by id and none is cached.
#[test]
fn a_flush_frees_the_receive_slot_cache() {
    let h = host();
    let mut p = parser(&h);
    feed_ok(&mut p, &slotted_chart("a", Some(1), "x"));
    drop(p);
    h.charts().flush();
    let mut p = parser(&h);
    feed_ok(&mut p, &[slotted_chart("b", None, "x"), slotted_chart("c", None, "x")].concat());
    feed_ok(&mut p, &[slotted_block("b", 5, 1), slotted_block("c", 5, 2)].concat());
    assert_eq!(d_values(&h, &["b", "c"]), [1, 2]);
}

/// A freed chart leaves its slot: the chart defined again under its id is found and cached.
#[test]
fn a_freed_chart_leaves_its_slot() {
    let h = host();
    let mut p = parser(&h);
    feed_ok(&mut p, &[slotted_chart("a", Some(7), "x"), slotted_chart("b", None, "x")].concat());
    let old = h.charts().find("t.a", true).unwrap();
    assert!(h.charts().free_if(&old, |_| true));
    feed_ok(&mut p, &[slotted_chart("a", None, "x").to_vec(), slotted_block("a", 7, 1).to_vec()].concat());
    let new = h.charts().find("t.a", true).unwrap();
    assert!(!Arc::ptr_eq(&old, &new));
    assert_eq!(new.dim("d").unwrap().collection().last_collected_value, 1);
}

/// `pluginsd_replay_begin()`'s invalid timestamps record names the wall clock that judged them: the child's when it
/// sent one above 0, the parent's otherwise (none, or 0), each with its tolerance.
#[test]
fn an_invalid_rbegin_names_the_wall_clock_that_judged_it() {
    let h = host();
    let hostname = h.hostname();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    for (line, clock, tolerance) in [
        (format!("RBEGIN 'test.c1' 100 50 {NOW}"), "child", 2),
        ("RBEGIN 'test.c1' 100 50 0".to_string(), "parent", 6),
        ("RBEGIN 'test.c1' 100 50".to_string(), "parent", 6),
    ] {
        let (_, logs) = feed_logged(&mut p, &[&line]);
        assert_eq!(
            logs,
            [format!(
                "PLUGINSD REPLAY ERROR: 'host:{hostname}/chart:test.c1' got a RBEGIN from 100 to 50, but timestamps \
                 are invalid (now is {NOW} [{clock} wall clock], tolerance {tolerance}). Ignoring RSET"
            )],
            "{line}"
        );
    }
}

/// `stream_thread_received_metadata()` and `_replication()`: every scoped CHART_DEFINITION_END counts, and a REND
/// only when it starts streaming; one asking for more, the stuck loop's forced finish and a malformed one do not.
#[test]
fn the_waiting_list_counts_ended_definitions_and_finished_replications() {
    let counted = |p: &mut Parser, lines: &[String]| {
        let before = crate::throttle();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        feed_all(p, &refs);
        let after = crate::throttle();
        (after.0 - before.0, after.1 - before.1)
    };
    let end = || vec![format!("CHART_DEFINITION_END {} {NOW} {NOW}", NOW - 100)];
    let (s, e) = (NOW - 20, NOW - 19);
    let rend = |streaming: &str| {
        vec![
            "RBEGIN 'test.c1'".to_string(),
            format!("REND 1 {} {e} {streaming} {s} {e} 0x{:x}", NOW - 100, NOW),
        ]
    };
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    assert_eq!(counted(&mut p, &end()), (1, 0));
    let data = [
        "RBEGIN 'test.c1'".to_string(),
        format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
        "RSET 'd1' 7 A".to_string(),
        format!("RSSTATE {0} {0}", e * 1_000_000),
        rend("false")[1].clone(),
    ];
    assert_eq!(counted(&mut p, &data), (0, 0));
    assert_eq!(counted(&mut p, &rend("false")), (0, 0));
    assert_eq!(counted(&mut p, &rend("false")), (0, 0));
    assert_eq!(h.replicating_charts(), 0, "the stuck loop finished it");
    // REND left no chart in scope: a definition comes again first
    assert_eq!(counted(&mut p, &end()), (0, 0));
    assert_eq!(counted(&mut p, &[DEFINE[0].to_string(), end()[0].clone()]), (1, 0));
    assert_eq!(counted(&mut p, &rend("true")), (0, 1));
    // a CHART_DEFINITION_END out of scope and a malformed REND
    let h = host();
    let mut p = parser(&h);
    assert_eq!(counted(&mut p, &end()), (0, 0));
    feed_all(&mut p, &DEFINE);
    assert_eq!(counted(&mut p, &["RBEGIN 'test.c1'".to_string(), "REND 1".to_string()]), (0, 0));
}

/// `pluginsd_require_scope_chart()`, `pluginsd_find_chart()` and `pluginsd_acquire_dimension()`: each refusal is its
/// daemon error, then parser_action()'s with the line re-quoted, and the line ends the connection. REND counts the
/// reply before it requires the chart.
#[test]
fn scope_and_lookup_errors_are_logged_as_c() {
    let slotted = [
        "CHART 'test.s' '' t u f c line 1 1",
        "DIMENSION SLOT:1 'd1' '' absolute 1 1 ''",
        "DIMENSION SLOT:2 'd2' '' absolute 1 1 ''",
        "BEGIN2 'test.s' 1 10 #",
    ];
    let with_slots = |last: &'static str| [&slotted[..], &[last]].concat();
    let cases: Vec<(Vec<&str>, &str, &str)> = vec![
        (vec!["SET 'd1' 1"], "command SET requires a chart defined via command CHART, but is not set.", "'SET' 'd1' '1'"),
        (vec!["END"], "command END requires a chart defined via command BEGIN, but is not set.", "'END'"),
        (vec!["END2"], "command END2 requires a chart defined via command BEGIN2, but is not set.", "'END2'"),
        (
            vec!["DIMENSION 'd1' '' absolute 1 1 ''"],
            "command DIMENSION requires a chart defined via command CHART, but is not set.",
            "'DIMENSION' 'd1' '' 'absolute' '1' '1' ''",
        ),
        (
            vec!["REND 1 0 0 true 0 0"],
            "command REND requires a chart defined via command RBEGIN, but is not set.",
            "'REND' '1' '0' '0' 'true' '0' '0'",
        ),
        (
            vec!["BEGIN2 'test.nope' 1 10 #"],
            "'host:child/chart:test.nope' got a BEGIN2 but chart does not exist.",
            "'BEGIN2' 'test.nope' '1' '10' '#'",
        ),
        (
            with_slots("SET2 SLOT:3 'd1' 1 1 A"),
            "'host:child/chart:test.s' got a SET2 with slot 3, but slots in the range [1 - 2] are expected.",
            "'SET2' 'SLOT:3' 'd1' '1' '1' 'A'",
        ),
        (
            with_slots("SET2 'd1' 1 1 A"),
            "'host:child/chart:test.s' got a SET2 with slot -1, but slots in the range [1 - 2] are expected.",
            "'SET2' 'd1' '1' '1' 'A'",
        ),
    ];
    for (lines, error, shown) in cases {
        let h = host();
        let mut p = parser(&h);
        let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, &lines));
        let n = lines.len();
        assert_eq!(results, [vec![true; n - 1], vec![false]].concat(), "{lines:?}");
        let keyword = lines[n - 1].split(' ').next().unwrap();
        let records: Vec<_> =
            records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
        assert_eq!(
            records[records.len() - 2..],
            [
                (Source::Daemon, Priority::Err, format!("PLUGINSD: {error}")),
                (
                    Source::Daemon,
                    Priority::Err,
                    format!(
                        "PLUGINSD: parser_action('{keyword}') failed on line {n}: {{ {shown} }} (quotes added to show \
                         parsing)"
                    )
                ),
            ],
            "{lines:?}"
        );
        assert_eq!(h.replication_replies(), u32::from(keyword == "REND"), "{lines:?}");
    }
}

/// `pluginsd_rrdset_cache_get_from_slot()`: a chart slot above the host's cache finds the chart by its id and caches
/// nothing, so the same slot names another chart in the next block.
#[test]
fn a_chart_slot_above_the_cache_is_found_by_id_and_not_cached() {
    let h = host();
    let mut p = parser(&h);
    let charts = [slotted_chart("a", Some(1), "x"), slotted_chart("b", None, "x"), slotted_chart("c", None, "x")];
    feed_ok(&mut p, &charts.concat());
    feed_ok(&mut p, &[slotted_block("b", 1025, 1), slotted_block("c", 1025, 2)].concat());
    assert_eq!(d_values(&h, &["a", "b", "c"]), [0, 1, 2]);
}

/// `pluginsd_function()`: source STREAM; the timeout and priority are str2i() (decimal only), a value below 1 the
/// default (10 s, 100); the version str2u(); the access the old role names, else hex bits under HTTP_ACCESS_ALL.
#[test]
fn a_streamed_function_takes_cs_defaults_and_roles() {
    let h = host();
    let mut p = parser(&h);
    let lines = [
        "FUNCTION GLOBAL \"t0\" 0 \"h\" \"\" \"member\" 0",
        "FUNCTION GLOBAL \"tneg\" -5 \"h\" \"\" \"admins\" -1 7",
        "FUNCTION GLOBAL \"thex\" 0x20 \"h\" \"\" \"any\" 0x20 0x7",
        "FUNCTION \"bare\" 30 \"h\" \"\" \"fff\"",
    ];
    assert!(feed_all(&mut p, &lines).iter().all(|&ok| ok));
    let got: Vec<_> = ["t0", "tneg", "thex", "bare"]
        .iter()
        .map(|n| {
            let f = h.functions().get(n.as_bytes()).unwrap();
            (f.timeout_s, f.priority, f.version, f.access, f.source)
        })
        .collect();
    assert_eq!(
        got,
        [
            (10, 100, 0, 0x1b, nrpc::Source::Stream),
            (10, 100, 7, 0x7b, nrpc::Source::Stream),
            (10, 100, 0, 0x8, nrpc::Source::Stream),
            (30, 100, 0, 0x7ff, nrpc::Source::Stream),
        ]
    );
}

/// `pluginsd_function()`: a FUNCTION without GLOBAL inside a chart scope is registered host-wide with a NOTICE; with
/// GLOBAL, or outside a scope, silently.
#[test]
fn a_function_in_a_chart_scope_is_registered_host_wide_with_a_notice() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &["CHART 'test.c1' '' t u f c line 1 1"]);
    let (results, records) = netdata_agent_log::capture(|| {
        feed_all(&mut p, &["FUNCTION \"inside\" 10 \"h\"", "FUNCTION GLOBAL \"global\" 10 \"h\""])
    });
    assert_eq!(results, [true, true]);
    let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(
        records,
        [(
            Source::Daemon,
            Priority::Notice,
            "PLUGINSD: 'host:child' got a FUNCTION 'inside' within chart 'test.c1' scope - chart-scoped functions are \
             no longer supported, registering it host-wide"
                .to_string()
        )]
    );
    assert!(h.functions().get(b"inside").is_some() && h.functions().get(b"global").is_some());
}

/// `pluginsd_function_del()`: the bare form removes by name and an unknown name is a debug record, both counted;
/// no name, or an empty one, also after GLOBAL, is an error that ends the connection.
#[test]
fn function_del_without_global_and_without_a_name() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &["FUNCTION GLOBAL \"a\" 10 \"h\"", "FUNCTION GLOBAL \"b\" 10 \"h\""]);
    let (results, records) =
        netdata_agent_log::capture(|| feed_all(&mut p, &["FUNCTION_DEL \"a\"", "FUNCTION_DEL \"nope\""]));
    assert_eq!(results, [true, true]);
    let names: Vec<_> = h.functions().all().into_iter().map(|(k, _)| k).collect();
    assert_eq!(names, [b"b".to_vec()]);
    let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(
        records,
        [(
            Source::Daemon,
            Priority::Debug,
            "PLUGINSD: 'host:child' FUNCTION_DEL 'nope' - function not found or ownership mismatch".to_string()
        )]
    );
    assert_eq!(p.data_collections_count, 4);
    for (line, shown) in [
        ("FUNCTION_DEL", "'FUNCTION_DEL'"),
        ("FUNCTION_DEL GLOBAL", "'FUNCTION_DEL' 'GLOBAL'"),
        ("FUNCTION_DEL ''", "'FUNCTION_DEL' ''"),
    ] {
        let mut p = parser(&h);
        let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, &[line]));
        assert_eq!(results, [false], "{line}");
        let records: Vec<_> =
            records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
        assert_eq!(
            records,
            [
                (
                    Source::Daemon,
                    Priority::Err,
                    "PLUGINSD: 'host:child' got a FUNCTION_DEL without a name. Ignoring it.".to_string()
                ),
                (
                    Source::Daemon,
                    Priority::Err,
                    format!(
                        "PLUGINSD: parser_action('FUNCTION_DEL') failed on line 1: {{ {shown} }} (quotes added to \
                         show parsing)"
                    )
                ),
            ],
            "{line}"
        );
        assert_eq!(p.data_collections_count, 0, "{line}");
    }
}

/// RSET (C `pluginsd_replay_set()`): `nan` and the `E` flag (alone or after `A`) store an empty slot; `NAN`, which
/// str2ndd reads only in lower case, and a missing value (the literal "NAN") store 0 with their flags; a disabled RSET
/// (an RBEGIN without timestamps) is accepted without looking its dimension up, stores nothing, and records C's ERR.
#[test]
fn rset_stores_empty_slots_zeroes_and_nothing_when_disabled() {
    let h = host();
    let mut p = parser(&h);
    feed_all(&mut p, &DEFINE);
    let chart = h.charts().find("test.c1", true).unwrap();
    let (ok, records) = netdata_agent_log::capture(|| {
        feed_all(&mut p, &["RBEGIN 'test.c1'", "RSET 'd1' 7 A", "RSET 'nope' 7 A"])
    });
    let disabled = "PLUGINSD REPLAY ERROR: 'host:child/chart:test.c1' got a RSET but it is disabled by RBEGIN errors";
    assert_eq!(ok, [true, true, true]);
    assert_eq!(
        records.iter().map(|r| (r.source, r.priority, r.message.clone().unwrap())).collect::<Vec<_>>(),
        vec![(Source::Collector, Priority::Err, disabled.to_string()); 2]
    );
    let d1 = chart.dim("d1").unwrap();
    assert_eq!((d1.ring().unwrap().latest_time_s(), d1.collection().counter, d1.collection().last_collected_time), (0, 0, (0, 0)));
    let window = |n: i64| format!("RBEGIN 'test.c1' {} {} {NOW}", NOW - 21 + n, NOW - 20 + n);
    let lines = [
        window(1),
        "RSET 'd1' nan A".to_string(),
        "RSET 'd2' 5 E".to_string(),
        window(2),
        "RSET 'd1' 5 R".to_string(),
        "RSET 'd2' 5 AE".to_string(),
        window(3),
        "RSET 'd1' NAN A".to_string(),
        "RSET 'd2'".to_string(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    let points = |id: &str| {
        let dim = chart.dim(id).unwrap();
        let ring = dim.ring().unwrap();
        let mut q = ring.query(NOW - 19, NOW - 17);
        let points: Vec<_> = (0..3)
            .map(|_| {
                let p = q.next_metric();
                (p.end_time_s, (!p.is_gap()).then_some((p.sum, p.anomaly_count, p.flags)))
            })
            .collect();
        (ring.oldest_time_s(), ring.latest_time_s(), dim.collection().counter, dim.collection().last_collected_time, points)
    };
    use netdata_agent_storage::storage_number::{SN_FLAG_NOT_ANOMALOUS, SN_FLAG_RESET};
    assert_eq!(
        points("d1"),
        (NOW - 20, NOW - 17, 3, (NOW - 17, 0), vec![
            (NOW - 19, None),
            (NOW - 18, Some((5.0, 1, SN_FLAG_RESET))),
            (NOW - 17, Some((0.0, 0, SN_FLAG_NOT_ANOMALOUS))),
        ])
    );
    assert_eq!(
        points("d2"),
        (NOW - 20, NOW - 17, 3, (NOW - 17, 0), vec![(NOW - 19, None), (NOW - 18, None), (NOW - 17, Some((0.0, 1, 0)))])
    );
}

/// RDSTATE (C `pluginsd_replay_rrddim_collection_state()`): nothing without an enabled RSET, not even the dimension's
/// lookup; the last collected time only moves forward; an integer dimension parses its value as an integer, a float
/// one as a double only with FLOAT_BASELINE (else as an integer); the last calculated and stored values are restored,
/// 0 when missing.
#[test]
fn rdstate_restores_the_collection_state_as_c() {
    for (capabilities, f1) in [(0, 11.0), (caps::FLOAT_BASELINE, 11.5)] {
        let h = host();
        let mut p = parser_with(&h, capabilities);
        let define = [DEFINE[0], DEFINE[1], "DIMENSION 'f1' '' absolute 1 1 'type=float'"];
        assert!(feed_all(&mut p, &define).iter().all(|&ok| ok));
        let chart = h.charts().find("test.c1", true).unwrap();
        let state = |id: &str| {
            let c = chart.dim(id).unwrap().collection();
            (c.last_collected_time, c.last_collected_value, c.last_collected_value_float, c.last_calculated_value, c.last_stored_value)
        };
        let (s, e) = (NOW - 20, NOW - 19);
        let ut = e * 1_000_000 + 250_000;
        let lines = ["RBEGIN 'test.c1'".to_string(), format!("RDSTATE 'd1' {ut} 9 2.5 3.25"), format!("RDSTATE 'nope' {ut} 9 2.5 3.25")];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert_eq!(feed_all(&mut p, &refs), [true, true, true]);
        assert_eq!(state("d1"), ((0, 0), 0, 0.0, 0.0, 0.0));
        let lines = [
            format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
            format!("RDSTATE 'd1' {ut} 1e3 2.5 3.25"),
            format!("RDSTATE 'f1' {ut} 11.5 -1.5 1e3"),
            format!("RDSTATE 'd1' {} 1e3", (e - 5) * 1_000_000),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
        assert_eq!(
            (state("d1"), state("f1")),
            (((e, 250_000), 1, 0.0, 0.0, 0.0), ((e, 250_000), 0, f1, -1.5, 1000.0)),
            "capabilities {capabilities}"
        );
    }
}

/// The stuck loop's count (C `pluginsd_replay_end()`): an RSET-enabled REND zeroes it before counting itself, a REND
/// that requested nothing zeroes it; the third suspicious REND in a row records C's INFO (WARNING when the parent's
/// last entry is 300 s old or more), finishes the chart and sends one final empty request.
#[test]
fn a_stuck_replication_counts_three_in_a_row() {
    for (age, priority) in [(19, Priority::Info), (299, Priority::Info), (300, Priority::Warning)] {
        let h = host();
        let mut p = parser(&h);
        feed_all(&mut p, &DEFINE);
        feed_all(&mut p, &[&format!("CHART_DEFINITION_END {} {NOW} {NOW}", NOW - 1000)]);
        let (s, e) = (NOW - age - 1, NOW - age);
        let rend = format!("REND 1 {} {e} false {s} {e} 0x{:x}", NOW - 1000, NOW);
        let data = vec![
            "RBEGIN 'test.c1'".to_string(),
            format!("RBEGIN 'test.c1' {s} {e} {NOW}"),
            "RSET 'd1' 7 A".to_string(),
            rend.clone(),
        ];
        let empty = vec!["RBEGIN 'test.c1'".to_string(), rend];
        let nothing = vec!["RBEGIN 'test.c1'".to_string(), format!("REND 1 {} {e} false 0 0 0x{:x}", NOW - 1000, NOW)];
        let chart = h.charts().find("test.c1", true).unwrap();
        let mut counts = Vec::new();
        for step in [&data, &empty, &data, &empty, &nothing, &empty, &empty] {
            feed_ok(&mut p, step);
            counts.push(chart.receiver().replication_empty_response_count);
        }
        assert_eq!((counts, h.replicating_charts()), (vec![1, 2, 1, 2, 0, 1, 2], 1));
        p.take_output();
        let requests = h.replication_requests();
        let (ok, records) = netdata_agent_log::capture(|| feed_all(&mut p, &["RBEGIN 'test.c1'", empty[1].as_str()]));
        assert_eq!(ok, [true, true]);
        assert_eq!(
            records.iter().map(|r| (r.priority, r.message.clone().unwrap())).collect::<Vec<_>>(),
            [(priority, format!("PLUGINSD REPLAY: 'host:child/chart:test.c1' detected stuck replication loop. Parent last entry: {e}, Child last entry: {e}, Gap: 0 seconds, Empty responses: 3. Forcing replication to finish."))]
        );
        assert_eq!(String::from_utf8(p.take_output()).unwrap(), "REPLAY_CHART \"test.c1\" \"true\" 0 0\n");
        let f = chart.meta().flags & (flags::RECEIVER_REPLICATION_FINISHED | flags::RECEIVER_REPLICATION_IN_PROGRESS);
        assert_eq!(
            (chart.receiver().replication_empty_response_count, h.replicating_charts(), f, h.replication_requests() - requests),
            (0, 0, flags::RECEIVER_REPLICATION_FINISHED, 1)
        );
    }
}

/// `rrdhost_stream_path_self()`'s flags: health while the host runs health, ephemeral while its `_is_ephemeral` label
/// says so (OVERWRITE), in `STREAM_PATH_FLAGS`' order; neither once both are off.
#[test]
fn the_parents_entry_flags_health_and_ephemeral() {
    let (h, mut p) = stream_path_parser(CAPTURED_CAPS);
    h.update_info(|i| i.health_enabled = true);
    feed_all(&mut p, &["LABEL '_is_ephemeral' 1 'yes'", "OVERWRITE"]);
    p.take_output();
    let body = format!(r#"{{"version":1,"streaming_path":[{CAPTURED_CHILD_ENTRY}]}}"#);
    feed_strings(&mut p, &stream_path_block(&body));
    let reply = |flags: &str| {
        let parent = captured_parent_entry(0).replace(r#""flags":[]"#, &format!(r#""flags":[{flags}]"#));
        format!("JSON STREAM_PATH\n{{\"version\":1,\"streaming_path\":[{CAPTURED_CHILD_ENTRY},{parent}]}}\nJSON_PAYLOAD_END\n")
    };
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(r#""health","ephemeral""#));
    h.update_info(|i| i.health_enabled = false);
    feed_all(&mut p, &["LABEL '_is_ephemeral' 1 'no'", "OVERWRITE"]);
    p.retention_updated(0);
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(""));
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
        Arc::new(TestWire::default()),
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
use netdata_agent_rrd::contexts::Taker;
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

/// `should_send_rrdset_matching()`: a proxied child's chart is not forwarded while its replication runs (from its
/// CHART_DEFINITION_END to the REND that starts streaming); blocks and the REND send nothing up, and the next block
/// sends the definition, then itself.
#[test]
fn a_proxied_chart_is_defined_upstream_after_its_replication() {
    let t = NOW - 10;
    let (_h, r, mut p) = proxied("*", CHILD, PARENT);
    let marker = vec![(Traffic::Metadata, "M\n".to_string())];
    feed_ok(&mut p, &[format!("CHART_DEFINITION_END {} {t} {NOW}", t - 100)]);
    feed_ok(&mut p, &block(t));
    assert_eq!(upstream_bytes(&mut p, &r), marker, "a block while replicating");
    feed_ok(&mut p, &["RBEGIN 'proxy.gauge'".to_string(), format!("REND 1 {} {t} true 0 0 {NOW}", t - 100)]);
    assert_eq!(upstream_bytes(&mut p, &r), marker, "the REND");
    feed_ok(&mut p, &block(t + 1));
    let commits = r.take();
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert!(commits[0].1.starts_with("CHART SLOT:0x1 \"proxy.gauge\" "), "{commits:?}");
    assert_eq!(commits[0].1.matches("\nDIMENSION ").count(), 2, "{commits:?}");
    let sent = upstream_bytes(&mut p, &r);
    assert!(sent[0].1.starts_with(&format!("BEGIN2 SLOT:0x1 'proxy.gauge' 1 {} #\n", t + 1)), "{sent:?}");
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

/// A proxied host's metadata goes up in messages of its own (they overtake the batch, D119): the child's claim id,
/// and its changed path (up, then back to the child); a retention change sends the path up, then down; nothing goes
/// up before the sender can take metadata, nor a path to a parent without PATHS.
#[test]
fn a_proxied_hosts_metadata_goes_up() {
    let (h, mut p) = stream_path_parser(CAPTURED_CAPS);
    let r = Arc::new(Recorder::with_capabilities(caps::CLAIM | caps::PATHS));
    h.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
    let claim = "5a1e0000-0000-4000-8000-0000000000e1";
    let claimed = [format!("CLAIMED_ID '{}' '{claim}'", h.machine_guid())];
    feed_ok(&mut p, &claimed);
    assert!(r.take().is_empty(), "not ready");
    h.sender_flags_set(sender_flags::READY_4_METRICS);
    feed_ok(&mut p, &claimed);
    assert_eq!(r.take(), vec![(Traffic::Metadata, format!("CLAIMED_ID '{}' '{claim}'\n", h.machine_guid()))]);
    let reply = |first: i64| {
        format!(
            "JSON STREAM_PATH\n{{\"version\":1,\"streaming_path\":[{CAPTURED_CHILD_ENTRY},{}]}}\nJSON_PAYLOAD_END\n",
            captured_parent_entry(first)
        )
    };
    let body = format!(r#"{{"version":1,"streaming_path":[{CAPTURED_CHILD_ENTRY}]}}"#);
    feed_ok(&mut p, &stream_path_block(&body));
    assert_eq!(r.take(), vec![(Traffic::Metadata, reply(0))]);
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(0));
    p.retention_updated(1_790_360_431);
    assert_eq!(r.take(), vec![(Traffic::Metadata, reply(1_790_360_431))]);
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(1_790_360_431));
    r.capabilities.store(caps::CLAIM, std::sync::atomic::Ordering::Relaxed);
    p.retention_updated(1_790_360_432);
    assert!(r.take().is_empty(), "a parent without PATHS");
    assert_eq!(String::from_utf8(p.take_output()).unwrap(), reply(1_790_360_432));
}

/// A vnode's first-time changes go up from its own sender (D120.1), in a path of the vnode with this agent's entry;
/// once its plugin lets it go (its collector offline), they are drained and nothing goes up.
#[test]
fn a_vnodes_retention_changes_go_up_while_collected() {
    let localhost = named_host("parity-child", "5a1e0000-0000-4000-8000-0000000000c6", true);
    let v = named_host("vnode", "5a1e0000-0000-4000-8000-0000000000c9", false);
    v.set_virtual();
    v.set_collector_online();
    let r = Arc::new(Recorder::with_capabilities(caps::PATHS));
    v.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
    v.sender_flags_set(sender_flags::READY_4_METRICS);
    v.contexts().record_first_time_changes(Taker::Sender, true);
    let mut p = parser(&v);
    assert!(feed_all(&mut p, &DEFINE).iter().all(|&ok| ok));
    let collect = |p: &mut Parser, t: i64| {
        let lines = [format!("BEGIN2 'test.c1' 1 {t} #"), "SET2 'd1' 5 5 A".to_string(), "END2".to_string()];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(p, &refs).iter().all(|&ok| ok));
        v.contexts().process_queued();
    };
    collect(&mut p, NOW - 10);
    r.take();
    let first = v.contexts().retention().0;
    stream_path::send_retention_changes_to_parent(&v, &localhost);
    let sent = r.take();
    assert_eq!(sent, vec![(Traffic::Metadata, String::from_utf8(stream_path::message(&v, &localhost, Some(first))).unwrap())]);
    assert!(sent[0].1.contains(r#""hops":0,"#) && sent[0].1.contains(r#""flags":["virtual"]"#), "{}", sent[0].1);
    v.virtual_offline();
    let define = [
        "CHART 'test.c2' '' 'title' 'units' 'family' 'ctx.c2' line 1000 1 '' fixture-pusher corpus",
        "DIMENSION 'd1' '' absolute 1 1 ''",
    ];
    assert!(feed_all(&mut p, &define).iter().all(|&ok| ok));
    let lines = [format!("BEGIN2 'test.c2' 1 {} #", NOW - 20), "SET2 'd1' 5 5 A".to_string(), "END2".to_string()];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    assert!(feed_all(&mut p, &refs).iter().all(|&ok| ok));
    v.contexts().process_queued();
    assert!(v.contexts().retention().0 < first);
    r.take();
    stream_path::send_retention_changes_to_parent(&v, &localhost);
    assert!(r.take().is_empty(), "its collector is offline");
    assert!(v.contexts().take_first_time_changes(Taker::Sender).is_empty(), "drained");
}

/// Localhost's first-time changes go up from its sender's stream thread (D120): each in a path of its own with the
/// value of its change, this agent's entry at hops 0; nothing before the sender is ready, nor to a parent without
/// PATHS.
#[test]
fn localhosts_retention_changes_go_up() {
    let h = named_host("parity-child", "5a1e0000-0000-4000-8000-0000000000c6", true);
    let r = Arc::new(Recorder::with_capabilities(caps::PATHS));
    h.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
    h.contexts().record_first_time_changes(Taker::Sender, true);
    let mut p = parser(&h);
    assert!(feed_all(&mut p, &DEFINE).iter().all(|&ok| ok));
    let collect = |p: &mut Parser, chart: &str, t: i64| {
        let lines = [
            format!("BEGIN2 '{chart}' 1 {t} #"),
            "SET2 'd1' 5 5 A".to_string(),
            "END2".to_string(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(p, &refs).iter().all(|&ok| ok));
        h.contexts().process_queued();
    };
    let t = NOW - 10;
    collect(&mut p, "test.c1", t);
    let first = h.contexts().retention().0;
    assert!(first > 0);
    stream_path::send_retention_changes_to_parent(&h, &h);
    assert!(r.take().is_empty(), "not ready");
    h.sender_flags_set(sender_flags::READY_4_METRICS);
    // a chart with an older first time widens localhost's
    let define = |p: &mut Parser, chart: &str| {
        let lines = [
            format!("CHART '{chart}' '' 'title' 'units' 'family' 'ctx.{chart}' line 1000 1 '' fixture-pusher corpus"),
            "DIMENSION 'd1' '' absolute 1 1 ''".to_string(),
        ];
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        assert!(feed_all(p, &refs).iter().all(|&ok| ok));
    };
    define(&mut p, "test.c2");
    collect(&mut p, "test.c2", t - 5);
    let widened = h.contexts().retention().0;
    assert_eq!(widened, first - 5);
    stream_path::send_retention_changes_to_parent(&h, &h);
    let sent = r.take();
    assert_eq!(sent, vec![(Traffic::Metadata, String::from_utf8(stream_path::message(&h, &h, Some(widened))).unwrap())]);
    assert!(sent[0].1.contains(r#""hops":0,"since":"#), "{}", sent[0].1);
    assert!(sent[0].1.contains(&format!(r#""first_time_t":{widened},"#)), "{}", sent[0].1);
    stream_path::send_retention_changes_to_parent(&h, &h);
    assert!(r.take().is_empty(), "each change once");
    r.capabilities.store(0, std::sync::atomic::Ordering::Relaxed);
    define(&mut p, "test.c3");
    collect(&mut p, "test.c3", t - 8);
    assert_eq!(h.contexts().retention().0, first - 8);
    stream_path::send_retention_changes_to_parent(&h, &h);
    assert!(r.take().is_empty(), "a parent without PATHS");
}

/// `pluginsd_process_cleanup()` writes under its caller's fields: none of the parser's at a stream thread's removal
/// outside a read, the parser's inside a read or a plugin's loop, whose frame the caller holds.
#[test]
fn the_cleanup_record_takes_its_callers_fields() {
    let h = host();
    let open = || {
        let mut p = parser(&h);
        assert!(feed_all(&mut p, &DEFINE).iter().all(|&ok| ok));
        assert!(feed_all(&mut p, &[&format!("BEGIN2 'test.c1' 1 {} #", NOW - 10), "SET2 'd1' 1 1 A"]).iter().all(|&ok| ok));
        p
    };
    let p = open();
    let ((), outside) = netdata_agent_log::capture(|| drop(p));
    let p = open();
    let ((), inside) = netdata_agent_log::capture(|| {
        let frame = p.log_frame();
        drop(p);
        drop(frame);
    });
    let fields = |records: Vec<netdata_agent_log::Captured>| records.into_iter().map(|r| r.fields).collect::<Vec<_>>();
    assert_eq!(fields(outside), [vec![]]);
    let parsers = [
        (Field::NidlNode, "child".to_string()),
        (Field::NidlInstance, "test.c1".to_string()),
        (Field::NidlContext, "ctx.c1".to_string()),
    ];
    assert_eq!(fields(inside), [parsers.to_vec()]);
}

/// `pluginsd_process_cleanup()`'s `pluginsd_cleanup_v2()`: a parser destroyed inside a BEGIN2 lets the collection lock go
/// with C's record; one destroyed after its END2 says nothing.
#[test]
fn a_parser_destroyed_inside_a_block_unlocks_it_as_c() {
    let h = host();
    let mut p = parser(&h);
    assert!(feed_all(&mut p, &DEFINE).iter().all(|&ok| ok));
    assert!(feed_all(&mut p, &[&format!("BEGIN2 'test.c1' 1 {} #", NOW - 10), "SET2 'd1' 1 1 A"]).iter().all(|&ok| ok));
    let ((), records) = netdata_agent_log::capture(|| drop(p));
    let texts: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        texts,
        ["PLUGINSD: 'host:child/chart:test.c1/' stale data collection lock found during THREAD CLEANUP; it has been \
          unlocked"]
    );
    let mut p = parser(&h);
    let block = [format!("BEGIN2 'test.c1' 1 {} #", NOW - 9), "SET2 'd1' 2 2 A".into(), "END2".into()];
    feed_ok(&mut p, &block);
    let ((), records) = netdata_agent_log::capture(|| drop(p));
    assert!(records.is_empty(), "{:?}", records.iter().map(|r| r.message.clone()).collect::<Vec<_>>());
}

// ---- a plugin's parser (milestone 8 commit 3, D142) ----

const PLUGIN_UPDATE_EVERY: i32 = 2;

thread_local! {
    /// The wall clock of a plugin's parser, in seconds, which a test may move.
    static PLUGIN_NOW: std::cell::Cell<i64> = const { std::cell::Cell::new(NOW) };
}

fn localhost() -> Arc<Host> {
    named_host("parent", "5a1e0000-0000-4000-8000-0000000000aa", true)
}

/// A parent's hosts for a plugin's parser: its localhost as `localhost()` makes it.
fn plugin_hosts() -> Arc<Hosts> {
    let lh = localhost();
    Arc::new(Hosts::new(Host::new(lh.machine_guid(), true, lh.info())))
}

fn plugin_parser(hosts: &Arc<Hosts>) -> Parser {
    plugin_parser_attaching(hosts, Arc::new(|_| {}))
}

/// A plugin's stdin: what its parser's transport wrote.
#[derive(Default)]
struct TestWire(std::sync::Mutex<Vec<u8>>);

impl crate::functions::Wire for TestWire {
    fn send(&self, text: &[u8]) -> isize {
        self.0.lock().unwrap().extend_from_slice(text);
        text.len() as isize
    }
}

/// A plugin's parser writing to `wire`.
fn plugin_parser_with_wire(hosts: &Arc<Hosts>, wire: &Arc<TestWire>) -> Parser {
    plugin_parser_on(hosts, Arc::new(|_| {}), Arc::clone(wire) as Arc<dyn crate::functions::Wire>)
}

/// A plugin's parser whose vnodes get their senders from `attach_sender`.
fn plugin_parser_attaching(hosts: &Arc<Hosts>, attach_sender: AttachSender) -> Parser {
    plugin_parser_on(hosts, attach_sender, Arc::new(TestWire::default()))
}

fn plugin_parser_on(hosts: &Arc<Hosts>, attach_sender: AttachSender, wire: Arc<dyn crate::functions::Wire>) -> Parser {
    Parser::plugin(
        PluginHosts {
            hosts: Arc::clone(hosts),
            update_every: 1,
            history: 4096,
            replication: true,
            replication_period: 86400,
            replication_step: 3600,
            attach_sender,
        },
        Config {
            capabilities: 0,
            update_every: PLUGIN_UPDATE_EVERY,
            page_size: 4096,
            now: || (PLUGIN_NOW.with(std::cell::Cell::get), 0),
            gap_when_lost_iterations_above: 3,
        },
        "difftest.plugin".into(),
        wire,
    )
}

/// The records of `lines` fed to a fresh plugin parser, the daemon's only (the collectors' reasons share one limiter
/// with every test): what feeding the last line returned, whether the plugin stays enabled, and the texts.
fn plugin_run(lines: &[&str]) -> (bool, bool, Vec<(Priority, String)>) {
    let hosts = plugin_hosts();
    let mut p = plugin_parser(&hosts);
    let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, lines));
    let records = records
        .into_iter()
        .filter(|r| r.source == Source::Daemon)
        .map(|r| (r.priority, r.message.unwrap_or_default()))
        .collect();
    (*results.last().unwrap(), p.enabled, records)
}

/// `PARSER_INIT_PLUGINSD` (`gperf-hashtable.h`): what a plugin may send and a child may not, and the reverse.
#[test]
fn a_plugins_repertoire_is_cs() {
    for line in [
        "FLUSH",
        "PLUGIN_KEEPALIVE",
        "TRUST_DURATIONS 1",
        "HOST",
        "HOST ''",
        "HOST localhost",
        "HOST 5A1E0000-0000-4000-8000-0000000000AA",
        "DYNCFG_ENABLE x",
    ] {
        assert_eq!(plugin_run(&[line]), (true, true, vec![]), "{line}");
        let h = host();
        let (results, _) = netdata_agent_log::capture(|| feed_all(&mut parser(&h), &[line]));
        assert_eq!(results, [line.starts_with("DYNCFG")], "a child's {line}");
    }
    for line in ["BEGIN2 'test.c1' 1 10 #", "SET2 'd1' 1 1 A", "END2", "CLAIMED_ID a b", "JSON STREAM_PATH", "RBEGIN x"] {
        let (ok, enabled, records) = plugin_run(&[line]);
        assert!(!ok && enabled, "{line}");
        assert!(matches!(&records[..], [(Priority::Err, r)] if r.starts_with("PLUGINSD: parser_action(")), "{line}: {records:?}");
    }
}

/// C's parser return classes: an error ends the run, a disable also disables the plugin, a stop ends the run with no
/// parser record.
#[test]
fn refusals_end_a_plugins_run_by_c_s_classes() {
    let action = |keyword: &str, n: usize, shown: &str| {
        (
            Priority::Err,
            format!("PLUGINSD: parser_action('{keyword}') failed on line {n}: {{ {shown} }} (quotes added to show parsing)"),
        )
    };
    let chart = "CHART 'test.c1' '' t u f c line 1 1";
    type Case<'a> = (&'a [&'a str], bool, Vec<(Priority, String)>);
    let cases: [Case; 14] = [
        (&["NOT_A_KEYWORD x"], true, vec![action("NOT_A_KEYWORD", 1, "'NOT_A_KEYWORD' 'x'")]),
        (&["FUNCTION"], true, vec![
            (Priority::Err, "PLUGINSD: 'host:parent' got a FUNCTION, without providing the required data (global = 'no', name = '(unset)', timeout = '(unset)', priority = '(unset)', version = '(unset)', help = '(unset)'). Ignoring it.".into()),
            action("FUNCTION", 1, "'FUNCTION'"),
        ]),
        (&["FUNCTION_DEL"], true, vec![
            (Priority::Err, "PLUGINSD: 'host:parent' got a FUNCTION_DEL without a name. Ignoring it.".into()),
            action("FUNCTION_DEL", 1, "'FUNCTION_DEL'"),
        ]),
        (&["CONFIG x"], true, vec![action("CONFIG", 1, "'CONFIG' 'x'")]),
        (&["HOST 5a1e0000-0000-4000-8000-0000000000bb"], false, vec![action("HOST", 1, "'HOST' '5a1e0000-0000-4000-8000-0000000000bb'")]),
        (&["HOST not-a-guid"], false, vec![action("HOST", 1, "'HOST' 'not-a-guid'")]),
        (&["HOST_DEFINE a b"], false, vec![action("HOST_DEFINE", 1, "'HOST_DEFINE' 'a' 'b'")]),
        (&["CHART 'nodot' '' t u f c line 1 1"], false, vec![action("CHART", 1, "'CHART' 'nodot' '' 't' 'u' 'f' 'c' 'line' '1' '1'")]),
        (&["BEGIN 'test.c1'"], false, vec![
            (Priority::Err, "PLUGINSD: 'host:parent/chart:test.c1' got a BEGIN but chart does not exist.".into()),
            action("BEGIN", 1, "'BEGIN' 'test.c1'"),
        ]),
        (&["SET 'd1' = 1"], false, vec![
            (Priority::Err, "PLUGINSD: command SET requires a chart defined via command CHART, but is not set.".into()),
            action("SET", 1, "'SET' 'd1' '1'"),
        ]),
        (&[chart, "TRUST_DURATIONS"], false, vec![action("TRUST_DURATIONS", 2, "'TRUST_DURATIONS'")]),
        (&["TRUST_DURATIONS 2"], false, vec![action("TRUST_DURATIONS", 1, "'TRUST_DURATIONS' '2'")]),
        (&[chart, "DISABLE"], false, vec![(Priority::Info, "PLUGINSD: plugin called DISABLE. Disabling it.".into())]),
        (&[chart, "EXIT"], true, vec![(Priority::Info, "PLUGINSD: plugin called EXIT.".into())]),
    ];
    for (lines, enabled, records) in cases {
        assert_eq!(plugin_run(lines), (false, enabled, records), "{lines:?}");
    }
}

/// `pluginsd_chart()` for a plugin: its file name is the default plugin, its update every the default one, and the
/// chart does not lower a receiver's minimum.
#[test]
fn a_plugins_chart_takes_its_defaults() {
    let hosts = plugin_hosts();
    let lh = Arc::clone(hosts.localhost());
    let before = lh.receiver_min_update_every();
    let mut p = plugin_parser(&hosts);
    feed_ok(&mut p, &[
        "CHART 'p.a' '' t u f c line 1000 '' '' '' corpus".into(),
        "CHART 'p.b' '' t u f c line 1000 1 '' go.d corpus".into(),
    ]);
    let meta = |id: &str| {
        let chart = lh.charts().find(id, true).unwrap();
        (chart.meta().plugin, chart.update_every())
    };
    assert_eq!(meta("p.a"), ("difftest.plugin".to_string(), PLUGIN_UPDATE_EVERY));
    assert_eq!(meta("p.b"), ("go.d".to_string(), 1));
    assert_eq!(lh.receiver_min_update_every(), before);
}

/// `pluginsd_begin()`: a plugin's microseconds go through the chart's clock unless it trusts its durations: a gap
/// over five intervals replaces them then.
#[test]
fn a_plugins_durations_are_filtered_until_trusted() {
    let since_last = |trust: &str| {
        let hosts = plugin_hosts();
        let lh = Arc::clone(hosts.localhost());
        let mut p = plugin_parser(&hosts);
        let at = |t: i64, p: &mut Parser, lines: &[String]| {
            PLUGIN_NOW.with(|now| now.set(t));
            feed_ok(p, lines);
        };
        let collect = ["SET 'd' = 1".to_string(), "END".into()];
        at(NOW - 20, &mut p, &[trust.into(), "CHART 'p.a' '' t u f c line 1000 1".into(), "DIMENSION 'd' '' absolute 1 1".into()]);
        at(NOW - 20, &mut p, &["BEGIN 'p.a'".into()]);
        at(NOW - 20, &mut p, &collect);
        at(NOW - 19, &mut p, &["BEGIN 'p.a'".into()]);
        at(NOW - 19, &mut p, &collect);
        at(NOW, &mut p, &["BEGIN 'p.a' 1000000".into()]);
        let chart = lh.charts().find("p.a", true).unwrap();
        let c = chart.collection();
        PLUGIN_NOW.with(|now| now.set(NOW));
        (c.usec_since_last_update, NOW * 1_000_000 - (c.last_collected.0 * 1_000_000 + c.last_collected.1))
    };
    let (untrusted, gap) = since_last("TRUST_DURATIONS 0");
    assert!(gap > 5_000_000, "{gap}");
    assert_eq!(untrusted, gap as u64);
    assert_eq!(since_last("PLUGIN_KEEPALIVE").0, gap as u64);
    assert_eq!(since_last("TRUST_DURATIONS 1").0, 1_000_000);
}

/// `rrdset_timed_done()` outside a receiver: a plugin's collections go upstream one by one, not batched (R55 N8).
#[test]
fn a_plugins_collections_go_upstream_at_once() {
    let mut info = localhost().info();
    info.stream_send = StreamSend::new(true, "parent:19999", "key", "*");
    let hosts = Arc::new(Hosts::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, info)));
    let lh = Arc::clone(hosts.localhost());
    let r = Arc::new(Recorder::with_capabilities(PARENT));
    lh.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
    lh.sender_flags_set(sender_flags::ADDED | sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
    let mut p = plugin_parser(&hosts);
    feed_ok(&mut p, &["CHART 'p.a' '' t u f c line 1000 1".into(), "DIMENSION 'd' '' absolute 1 1".into()]);
    assert_eq!(r.take(), []);
    for i in 0..3 {
        PLUGIN_NOW.with(|now| now.set(NOW + i));
        feed_ok(&mut p, &["BEGIN 'p.a'".into(), "SET 'd' = 1".into(), "END".into()]);
        let commits = r.take();
        match i {
            // the first collection stores nothing, and sends the definition
            0 => assert!(
                matches!(&commits[..], [(Traffic::Metadata, definition)] if definition.starts_with("CHART SLOT:0x1 \"p.a\" ")),
                "{commits:?}"
            ),
            _ => assert!(
                matches!(&commits[..], [(Traffic::Data, data)] if data.starts_with("BEGIN2 SLOT:0x1 'p.a' ")),
                "{i}: {commits:?}"
            ),
        }
    }
    PLUGIN_NOW.with(|now| now.set(NOW));
}

/// `pluginsd_config()` before DynCfg: an action is counted as a collection, an unknown one reported.
#[test]
fn config_is_counted_and_unknown_actions_reported() {
    let hosts = plugin_hosts();
    let mut p = plugin_parser(&hosts);
    let (results, records) = netdata_agent_log::capture(|| {
        feed_all(&mut p, &["CONFIG x create accepted job /x internal internal update 0 0", "CONFIG x status running", "CONFIG x bogus"])
    });
    assert_eq!(results, [true; 3]);
    assert_eq!(p.data_collections_count, 3);
    let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(records, [(Source::Collector, Priority::Warning, "DYNCFG: unknown action 'bogus' received from plugin".to_string())]);
}

/// `pluginsd_function()` from a plugin registers it as the plugin's: `config` is refused; FUNCTION_DEL removes it.
#[test]
fn a_plugins_functions_are_its_own() {
    let hosts = plugin_hosts();
    let lh = Arc::clone(hosts.localhost());
    let mut p = plugin_parser(&hosts);
    let (results, records) = netdata_agent_log::capture(|| {
        feed_all(&mut p, &["FUNCTION GLOBAL 'f1' 10 'help' 'top' 'member' 100 3", "FUNCTION GLOBAL 'config' 10 'help'"])
    });
    assert_eq!(results, [true, true]);
    assert_eq!(lh.functions().get(b"f1").map(|f| f.source), Some(nrpc::Source::Plugin));
    assert!(lh.functions().get(b"config").is_none());
    let records: Vec<_> = records.into_iter().map(|r| r.message.unwrap_or_default()).collect();
    assert_eq!(records, ["NRPC: 'host:parent' attempted to register reserved dynamic-configuration method 'config' from a plugin. Ignoring it."]);
    feed_ok(&mut p, &["FUNCTION_DEL GLOBAL 'f1'".into()]);
    assert!(lh.functions().get(b"f1").is_none());
    assert_eq!(p.data_collections_count, 3);
}

/// `pluginsd_set_scope_chart()` and `pluginsd_clear_scope_chart()`: a scope chart another thread took over is
/// collected twice (the scope kept, the plugin disabled), and this thread does not give it up for the other.
#[test]
fn a_chart_another_thread_collects_is_collected_twice() {
    let hosts = plugin_hosts();
    let lh = Arc::clone(hosts.localhost());
    let mut p = plugin_parser(&hosts);
    feed_ok(&mut p, &["CHART 'p.a' '' t u f c line 1000 1".into(), "DIMENSION 'd' '' absolute 1 1".into()]);
    let chart = lh.charts().find("p.a", true).unwrap();
    let me = netdata_agent_log::tid();
    assert_eq!(chart.scope_tid(), me);
    feed_ok(&mut p, &["BEGIN 'p.a'".into(), "SET 'd' = 1".into(), "END".into()]);
    assert_eq!(chart.scope_tid(), 0, "END gives the chart up");
    feed_ok(&mut p, &["BEGIN 'p.a'".into()]);
    let other = me + 1;
    chart.set_scope_tid(other);
    let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, &["BEGIN 'p.a'"]));
    assert_eq!((results, p.enabled), (vec![false], false));
    let records: Vec<_> = records.into_iter().map(|r| (r.source, r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(records, [
        (Source::Collector, Priority::Warning, format!("PLUGINSD: keyword BEGIN: 'host:parent/chart:p.a' is collected twice (my tid {me}, other collector tid {other})")),
        (Source::Daemon, Priority::Err, "PLUGINSD: parser_action('BEGIN') failed on line 7: { 'BEGIN' 'p.a' } (quotes added to show parsing)".to_string()),
    ]);
    let ((), records) = netdata_agent_log::capture(|| drop(p));
    let records: Vec<_> = records.into_iter().map(|r| r.message.unwrap_or_default()).collect();
    assert_eq!(records, [format!("PLUGINSD: attempted to clear collector_tid {other} for 'host:parent/chart:p.a/' from non-owner thread {me} during THREAD CLEANUP")]);
    assert_eq!(chart.scope_tid(), other);
}

/// The exit sweep (`pluginsd_process()`): the charts this thread created or redefined last become obsolete, another
/// thread's stay.
#[test]
fn a_plugins_charts_are_obsolete_after_its_run() {
    let hosts = plugin_hosts();
    let lh = Arc::clone(hosts.localhost());
    let mut p = plugin_parser(&hosts);
    feed_ok(&mut p, &["CHART 'p.a' '' t u f c line 1000 1".into(), "CHART 'p.b' '' t u f c line 1000 1".into()]);
    std::thread::scope(|s| {
        s.spawn(|| {
            let mut other = plugin_parser(&hosts);
            feed_ok(&mut other, &["CHART 'p.b' '' t u f c line 1000 1".into(), "CHART 'p.c' '' t u f c line 1000 1".into()]);
        });
    });
    lh.charts().obsolete_created_by(&lh, netdata_agent_log::tid());
    drop(p);
    let obsolete = |id: &str| lh.charts().find(id, true).unwrap().flags() & flags::OBSOLETE != 0;
    assert_eq!((obsolete("p.a"), obsolete("p.b"), obsolete("p.c")), (true, false, false));
}

/// The run frame: every record of the run carries the host and the scope chart's name and context as they are when it
/// is written; between lines no request.
#[test]
fn a_plugins_run_frame_follows_its_scope() {
    let hosts = plugin_hosts();
    let mut p = plugin_parser(&hosts);
    let frame = p.run_frame();
    let record = || {
        let ((), records) = netdata_agent_log::capture(|| nd_log!(Source::Daemon, Priority::Err, "x"));
        records.into_iter().next().unwrap().fields
    };
    let node = (Field::NidlNode, "parent".to_string());
    assert_eq!(record(), std::slice::from_ref(&node));
    feed_ok(&mut p, &["CHART 'p.a' 'named' t u f ctx.a line 1000 1".into()]);
    assert_eq!(record(), [node.clone(), (Field::NidlInstance, "p.named".into()), (Field::NidlContext, "ctx.a".into())]);
    feed_ok(&mut p, &["FLUSH".into()]);
    assert_eq!(record(), std::slice::from_ref(&node));
    feed_ok(&mut p, &["BEGIN 'p.a'".into()]);
    // the parser's end takes its node and scope with it
    drop(p);
    assert_eq!(record(), []);
    drop(frame);
    assert_eq!(record(), []);
}

/// The run frame on records rrd writes: mid-line, the line and the host, the scope having ended at END as in C; between
/// lines, the host and the scope chart (the non-owner record of a parser dropped holding another thread's chart).
#[test]
fn records_under_a_plugins_run_frame_carry_cs_fields() {
    let hosts = plugin_hosts();
    let lh = Arc::clone(hosts.localhost());
    let mut p = plugin_parser(&hosts);
    let frame = p.run_frame();
    feed_ok(&mut p, &[
        "CHART 'p.a' 'named' t u f ctx.a line 1000 1 obsolete".into(),
        "DIMENSION 'd' '' absolute 1 1".into(),
        "BEGIN 'p.a'".into(),
        "SET 'd' = 1".into(),
    ]);
    let fields = |records: Vec<netdata_agent_log::Captured>, text: &str| {
        records.into_iter().find(|r| r.message.as_deref().is_some_and(|m| m.contains(text))).map(|r| r.fields)
    };
    let (_, records) = netdata_agent_log::capture(|| feed_all(&mut p, &["END"]));
    let node = (Field::NidlNode, "parent".to_string());
    assert_eq!(
        fields(records, "has the OBSOLETE flag set, but it is collected."),
        Some(vec![node.clone(), (Field::Request, "'END'".to_string())])
    );
    feed_ok(&mut p, &["BEGIN 'p.a'".into()]);
    lh.charts().find("p.a", true).unwrap().set_scope_tid(netdata_agent_log::tid() + 1);
    let ((), records) = netdata_agent_log::capture(|| drop(p));
    assert_eq!(
        fields(records, "attempted to clear collector_tid"),
        Some(vec![node, (Field::NidlInstance, "p.named".into()), (Field::NidlContext, "ctx.a".into())])
    );
    drop(frame);
}


// ---- a plugin's vnodes (milestone 8 commit 4, D145) ----

const VNODE: &str = "5a1e0000-0000-4000-8000-0000000000d1";

fn vnode_parser(hosts: &Arc<Hosts>, attached: &Arc<std::sync::atomic::AtomicUsize>) -> Parser {
    let attached = Arc::clone(attached);
    plugin_parser_attaching(
        hosts,
        Arc::new(move |_| {
            attached.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }),
    )
}

/// `pluginsd_host_define()` … `pluginsd_host_define_end()`: the vnode created with localhost's settings, its labels'
/// system info, the virtual OS and its labels (with `_collector_machine_guid` and `_is_ephemeral` normalized), flagged
/// and collected, a sender attached, and the scope host: its charts land in it, until `HOST localhost`.
#[test]
fn a_plugin_defines_and_collects_a_vnode() {
    let hosts = plugin_hosts();
    let attached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut p = vnode_parser(&hosts, &attached);
    let (results, records) = netdata_agent_log::capture(|| {
        feed_all(&mut p, &[
            &format!("HOST_DEFINE {} 'v1'", VNODE.to_uppercase()),
            "HOST_LABEL _os_name 'Linux'",
            "HOST_LABEL role web",
            "HOST_LABEL _node_stale_after_seconds 60",
            "HOST_DEFINE_END",
            "CHART 'v.a' '' t u f c line 1000 1",
        ])
    });
    assert!(results.iter().all(|&ok| ok), "{results:?}");
    let v = hosts.find_by_guid(VNODE).expect("defined");
    let info = v.info();
    assert_eq!(
        (info.hostname.as_str(), info.os.as_str(), info.system_info.hops, info.system_info.host_os_name.as_deref()),
        ("v1", "Netdata Virtual Host 1.0", 1, Some("Linux"))
    );
    assert_eq!((v.is_virtual(), v.collector_online(), v.is_online(), v.is_orphan()), (true, true, true, false));
    let labels = v.labels();
    assert_eq!(
        (labels.get(b"role"), labels.get(b"_collector_machine_guid"), labels.get(b"_is_ephemeral")),
        (Some(&b"web"[..]), Some(hosts.localhost().machine_guid().as_bytes()), Some(&b"false"[..]))
    );
    assert_eq!(attached.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(v.charts().find("v.a", true).is_some() && hosts.localhost().charts().find("v.a", true).is_none());
    let records: Vec<_> = records.into_iter().filter(|r| r.message.as_deref().is_some_and(|m| m.starts_with("VNODE"))).map(|r| (r.priority, r.message.unwrap())).collect();
    assert_eq!(records, [(Priority::Info, "VNODE: Configuring node stale after 0 seconds for host \"v1\"".to_string())]);
    feed_ok(&mut p, &["HOST localhost".into(), "CHART 'l.a' '' t u f c line 1000 1".into()]);
    assert!(hosts.localhost().charts().find("l.a", true).is_some() && v.charts().find("l.a", true).is_none());
    feed_ok(&mut p, &[format!("HOST {VNODE}"), "CHART 'v.b' '' t u f c line 1000 1".into()]);
    assert!(v.charts().find("v.b", true).is_some());
    // the run's end lets it go
    let ((), records) = netdata_agent_log::capture(|| p.vnodes_offline());
    let records: Vec<_> = records.into_iter().map(|r| (r.priority, r.message.unwrap_or_default())).collect();
    assert_eq!(
        records,
        [
            (Priority::Info, "PLUGINSD: Checking virtual status for v1".to_string()),
            (Priority::Info, "PLUGINSD: Reseting virtual host status for v1".to_string()),
        ]
    );
    assert_eq!((v.is_virtual(), v.collector_online(), v.is_online()), (false, false, false));
}

/// C's quirk: HOST keeps the chart in scope, so a SET after it collects into the previous host's chart.
#[test]
fn host_keeps_the_chart_in_scope_as_c() {
    let hosts = plugin_hosts();
    let attached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut p = vnode_parser(&hosts, &attached);
    feed_ok(&mut p, &[
        format!("HOST_DEFINE {VNODE} v1"),
        "HOST_DEFINE_END".into(),
        "HOST localhost".into(),
        "CHART 'l.a' '' t u f c line 1000 1".into(),
        "DIMENSION 'd' '' absolute 1 1".into(),
        "BEGIN 'l.a'".into(),
        format!("HOST {VNODE}"),
        "SET 'd' = 7".into(),
    ]);
    let chart = hosts.localhost().charts().find("l.a", true).unwrap();
    assert_eq!(chart.dim("d").unwrap().collection().collected_value, 7);
    feed_ok(&mut p, &["END".into()]);
}

/// A vnode its run let go and another plugin's `HOST` re-enables: claimed again (C's record), collected again.
#[test]
fn a_vnode_let_go_is_reenabled_by_host() {
    let hosts = plugin_hosts();
    let attached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut p = vnode_parser(&hosts, &attached);
    feed_ok(&mut p, &[format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END".into()]);
    let v = hosts.find_by_guid(VNODE).unwrap();
    // another plugin's run end, which defined it too
    v.virtual_offline();
    let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, &[&format!("HOST {VNODE}")]));
    assert_eq!(results, [true]);
    let records: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(records, ["VNODE: Re-enabling virtual host \"v1\""]);
    assert_eq!((v.is_virtual(), v.collector_online()), (true, true));
}

/// `pluginsd_host_claim_as_local_vnode()`: a child streaming the vnode's GUID is evicted, with C's warning; the
/// vnode is collected.
#[test]
fn a_definition_evicts_a_child_streaming_its_guid() {
    use netdata_agent_rrd::host::{Attach, ReceiverLink, ReceiverSlot};
    let hosts = plugin_hosts();
    let attached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut p = vnode_parser(&hosts, &attached);
    let child = hosts.find_or_create(VNODE, DbMode::Ram, || named_host("v1", VNODE, false).info(), |_| {}).unwrap();
    let slot = Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
    assert_eq!(child.set_receiver(Arc::clone(&slot)), Attach::Attached);
    let (results, records) = std::thread::scope(|s| {
        s.spawn(|| {
            while !slot.stop_requested.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::yield_now();
            }
            child.clear_receiver(&slot, 0);
        });
        netdata_agent_log::capture(|| feed_all(&mut p, &[&format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END"]))
    });
    assert_eq!(results, [true, true]);
    let warnings: Vec<_> = records.into_iter().filter(|r| r.priority == Priority::Warning).filter_map(|r| r.message).collect();
    assert_eq!(
        warnings,
        [format!(
            "PLUGINSD: HOST_DEFINE_END: host 'v1' (machine guid {VNODE}) was receiving a stream while it is collected locally \
             as a vnode - the stream has been disconnected. If this is a real child node, its machine guid conflicts \
             with the guid of a locally collected vnode."
        )]
    );
    assert_eq!((child.is_virtual(), child.collector_online(), child.receiver().is_none()), (true, true, true));
    assert_eq!((child.is_orphan(), child.is_online()), (false, true));
}

/// The vnode keywords' refusals, each C's disable: the plugin is not started again.
#[test]
fn vnode_keywords_refuse_as_c() {
    let action = |keyword: &str, n: usize, shown: &str| {
        (
            Priority::Err,
            format!("PLUGINSD: parser_action('{keyword}') failed on line {n}: {{ {shown} }} (quotes added to show parsing)"),
        )
    };
    let define = format!("HOST_DEFINE {VNODE} v1");
    type Case<'a> = (Vec<&'a str>, (Priority, String));
    let cases: [Case; 7] = [
        (vec!["HOST_DEFINE"], action("HOST_DEFINE", 1, "'HOST_DEFINE'")),
        (vec!["HOST_DEFINE g ''"], action("HOST_DEFINE", 1, "'HOST_DEFINE' 'g' ''")),
        (vec![&define, &define], action("HOST_DEFINE", 2, &format!("'HOST_DEFINE' '{VNODE}' 'v1'"))),
        (vec!["HOST_DEFINE zz v1"], action("HOST_DEFINE", 1, "'HOST_DEFINE' 'zz' 'v1'")),
        (vec!["HOST_LABEL k v"], action("HOST_LABEL", 1, "'HOST_LABEL' 'k' 'v'")),
        (vec![&define, "HOST_LABEL k"], action("HOST_LABEL", 2, "'HOST_LABEL' 'k'")),
        (vec!["HOST_DEFINE_END"], action("HOST_DEFINE_END", 1, "'HOST_DEFINE_END'")),
    ];
    for (lines, record) in cases {
        assert_eq!(plugin_run(&lines), (false, false, vec![record]), "{lines:?}");
    }
}

/// D146.1: a vnode is created with localhost's replication settings (C passes both `stream_receive.replication`), and
/// one revived from its archived state takes the configured ones, capped for its own ring (`rrdhost_update()`).
#[test]
fn a_vnodes_replication_is_cs() {
    const ARCHIVED: &str = "5a1e0000-0000-4000-8000-0000000000d2";
    let hosts = plugin_hosts();
    let archived = HostInfo {
        history_entries: 100_000,
        replication_enabled: false,
        replication_period: 0,
        replication_step: 0,
        ..named_host("a1", ARCHIVED, false).info()
    };
    hosts.add_archived(ARCHIVED, archived, |_| {}).clear_pending_context_load();
    let attached = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut p = vnode_parser(&hosts, &attached);
    feed_ok(&mut p, &[
        format!("HOST_DEFINE {VNODE} v1"),
        "HOST_DEFINE_END".into(),
        format!("HOST_DEFINE {ARCHIVED} a1"),
        "HOST_DEFINE_END".into(),
    ]);
    let replication = |host: &Host| {
        let info = host.info();
        (info.replication_enabled, info.replication_period, info.replication_step)
    };
    // localhost's: the configured period capped for its ring of 4096 entries a second
    assert_eq!(replication(hosts.localhost()), (true, 4096, 3600));
    assert_eq!(replication(&hosts.find_by_guid(VNODE).unwrap()), (true, 4096, 3600));
    assert_eq!(replication(&hosts.find_by_guid(ARCHIVED).unwrap()), (true, 86400, 3600));
}

/// R60-2: END completes the chart in scope on its own host (`st->rrdhost`), after a HOST moved the parser's: its
/// definition goes up through localhost's sender, not the vnode's.
#[test]
fn end_completes_the_chart_on_its_own_host() {
    let mut info = localhost().info();
    info.stream_send = StreamSend::new(true, "parent:19999", "key", "*");
    let hosts = Arc::new(Hosts::new(Host::new("5a1e0000-0000-4000-8000-0000000000aa", true, info)));
    let up = |host: &Host| {
        let r = Arc::new(Recorder::with_capabilities(PARENT));
        host.set_upstream(Arc::clone(&r) as Arc<dyn Upstream>);
        host.sender_flags_set(sender_flags::ADDED | sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
        r
    };
    let lh = up(hosts.localhost());
    let mut p = plugin_parser(&hosts);
    feed_ok(&mut p, &[format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END".into()]);
    let v = up(&hosts.find_by_guid(VNODE).unwrap());
    feed_ok(&mut p, &[
        "HOST localhost".into(),
        "CHART 'l.a' '' t u f c line 1000 1".into(),
        "DIMENSION 'd' '' absolute 1 1".into(),
    ]);
    lh.take();
    feed_ok(&mut p, &["BEGIN 'l.a'".into(), format!("HOST {VNODE}"), "SET 'd' = 7".into(), "END".into()]);
    let commits = lh.take();
    assert!(
        matches!(&commits[..], [(Traffic::Metadata, definition)] if definition.starts_with("CHART SLOT:0x1 \"l.a\" ")),
        "{commits:?}"
    );
    assert_eq!(v.take(), []);
}

/// `pluginsd_host_claim_as_local_vnode()` failing (a receiver that does not leave): at HOST_DEFINE_END the vnode is
/// let go, at a re-enabling HOST it stays flagged and the parser has no host (the run frame shows no node); both
/// retry the plugin, with C's record.
#[test]
fn a_claim_that_fails_retries_the_plugin() {
    use netdata_agent_rrd::host::{Attach, ReceiverLink, ReceiverSlot};
    let stuck = || Arc::new(ReceiverSlot::new(1, Default::default(), ReceiverLink::default(), Box::new(|| {})));
    let refusal = |keyword: &str| {
        (
            Priority::Err,
            format!(
                "PLUGINSD: {keyword}: host 'v1' (machine guid {VNODE}) is collected locally as a vnode, but its \
                 streaming receiver could not be stopped - not collecting into it"
            ),
        )
    };
    // the parser's records (the receiver's own "takes too long to stop" comes first, as C's)
    let errors = |records: Vec<netdata_agent_log::Captured>| -> Vec<(Priority, String)> {
        records
            .into_iter()
            .filter(|r| r.priority == Priority::Err && r.source == Source::Daemon)
            .map(|r| (r.priority, r.message.unwrap_or_default()))
            .filter(|(_, m)| m.starts_with("PLUGINSD: ") && !m.starts_with("PLUGINSD: parser_action"))
            .collect()
    };

    let hosts = plugin_hosts();
    let child = hosts.find_or_create(VNODE, DbMode::Ram, || named_host("v1", VNODE, false).info(), |_| {}).unwrap();
    let slot = stuck();
    assert_eq!(child.set_receiver(Arc::clone(&slot)), Attach::Attached);
    let mut p = plugin_parser(&hosts);
    let (results, records) =
        netdata_agent_log::capture(|| feed_all(&mut p, &[&format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END"]));
    assert_eq!(results, [true, false]);
    assert_eq!(errors(records), [refusal("HOST_DEFINE_END")]);
    assert_eq!((p.retry, p.enabled), (true, true));
    assert_eq!((child.is_virtual(), child.receiver().is_some()), (false, true));
    child.clear_receiver(&slot, 0);

    let hosts = plugin_hosts();
    let mut p = plugin_parser(&hosts);
    let frame = p.run_frame();
    feed_ok(&mut p, &[format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END".into()]);
    let v = hosts.find_by_guid(VNODE).unwrap();
    // another plugin's run end, then a child streams its GUID
    v.virtual_offline();
    let slot = stuck();
    assert_eq!(v.set_receiver(Arc::clone(&slot)), Attach::Attached);
    let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, &[&format!("HOST {VNODE}")]));
    assert_eq!(results, [false]);
    assert_eq!(errors(records), [refusal("HOST")]);
    assert_eq!((p.retry, p.enabled), (true, true));
    // flagged, as C leaves it; online is the receiver's
    assert_eq!((v.is_virtual(), v.collector_online(), v.receiver().is_some()), (true, true, true));
    let ((), records) = netdata_agent_log::capture(|| nd_log!(Source::Daemon, Priority::Err, "x"));
    assert_eq!(records.into_iter().next().unwrap().fields, []);
    drop(p);
    drop(frame);
    v.clear_receiver(&slot, 0);
}

/// `pluginsd_host_define_end()` when `rrdhost_find_or_create()` returns NULL (`pluginsd_parser.c:291-295`): an archived
/// host of another memory mode that the metadata writer holds is not discarded (`rrdhost.c:873-876`, rrd's
/// `MetadataBusy`, D95.7), so the line fails with `parser_action()`'s record only (`pluginsd_parser.h:260-264`) and the
/// plugin is retried, still enabled.
#[test]
fn a_definition_over_a_host_the_metadata_writer_holds_retries_the_plugin() {
    const ARCHIVED: &str = "5a1e0000-0000-4000-8000-0000000000d2";
    let hosts = plugin_hosts();
    let archived = HostInfo { db_mode: DbMode::Alloc, ..named_host("a1", ARCHIVED, false).info() };
    assert_ne!(archived.db_mode, hosts.localhost().info().db_mode);
    let archived = hosts.add_archived(ARCHIVED, archived, |_| {});
    archived.clear_pending_context_load();
    let held = archived.metadata_try_read().expect("not freed");
    let mut p = plugin_parser(&hosts);
    let (results, records) =
        netdata_agent_log::capture(|| feed_all(&mut p, &[&format!("HOST_DEFINE {ARCHIVED} a1"), "HOST_DEFINE_END"]));
    let records: Vec<_> = records
        .into_iter()
        .filter(|r| r.source == Source::Daemon)
        .map(|r| (r.priority, r.message.unwrap_or_default()))
        .collect();
    assert_eq!(results, [true, false]);
    assert_eq!(
        records,
        [(
            Priority::Err,
            "PLUGINSD: parser_action('HOST_DEFINE_END') failed on line 2: { 'HOST_DEFINE_END' } (quotes added to show \
             parsing)"
                .to_string()
        )]
    );
    assert_eq!((p.retry, p.enabled), (true, true));
    assert!(Arc::ptr_eq(&hosts.find_by_guid(ARCHIVED).unwrap(), &archived));
    assert_eq!((archived.is_virtual(), archived.collector_online()), (false, false));
    drop(held);
}

/// The run frame names the parser's host as HOST_DEFINE_END and HOST move it; a scope clear names the chart's own
/// host in its text while the frame names the parser's (C's `st->rrdhost` and `parser->user.host`).
#[test]
fn the_run_frame_follows_the_parsers_host() {
    let hosts = plugin_hosts();
    let mut p = plugin_parser(&hosts);
    let frame = p.run_frame();
    let node = || {
        let ((), records) = netdata_agent_log::capture(|| nd_log!(Source::Daemon, Priority::Err, "x"));
        records.into_iter().next().unwrap().fields.into_iter().find(|(f, _)| *f == Field::NidlNode).map(|(_, v)| v)
    };
    assert_eq!(node().as_deref(), Some("parent"));
    feed_ok(&mut p, &[format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END".into()]);
    assert_eq!(node().as_deref(), Some("v1"));
    feed_ok(&mut p, &[
        "HOST localhost".into(),
        "CHART 'l.a' '' t u f c line 1000 1".into(),
        "DIMENSION 'd' '' absolute 1 1".into(),
        "BEGIN 'l.a'".into(),
    ]);
    assert_eq!(node().as_deref(), Some("parent"));
    feed_ok(&mut p, &[format!("HOST {VNODE}")]);
    assert_eq!(node().as_deref(), Some("v1"));
    hosts.localhost().charts().find("l.a", true).unwrap().set_scope_tid(netdata_agent_log::tid() + 1);
    let ((), records) = netdata_agent_log::capture(|| drop(p));
    let record = records
        .into_iter()
        .find(|r| r.message.as_deref().is_some_and(|m| m.contains("attempted to clear collector_tid")))
        .expect("the non-owner record");
    assert!(record.message.unwrap().contains("'host:parent/chart:l.a/'"));
    assert_eq!(record.fields.iter().find(|(f, _)| *f == Field::NidlNode).map(|(_, v)| v.as_str()), Some("v1"));
    drop(frame);
}

// ---- a plugin's functions (milestone 8 commit 5, D147) ----

/// A parent's no-wait call on localhost (as a child runs it), and where its answer arrives.
fn call_plugin(hosts: &Arc<Hosts>, cmd: &[u8], tx: &[u8]) -> std::sync::mpsc::Receiver<(nrpc::reply::Reply, u16)> {
    let (send, answers) = std::sync::mpsc::channel();
    let lh = hosts.localhost();
    let called = nrpc::call::Calls::process().call(nrpc::call::CallSpec {
        owner: Some((lh.functions(), "parent")),
        cmd,
        source: b"from-parent",
        user_access: 0,
        timeout_s: 0,
        wait: false,
        allow_restricted: true,
        call_id: Some(tx),
        payload: None,
        reply: nrpc::reply::Reply::new(nrpc::reply::ContentType::TextPlain),
        done: Some(Box::new(move |reply, code| {
            let _ = send.send((reply, code));
        })),
        progress: None,
        is_cancelled: None,
    });
    assert_eq!(called.code, 200);
    answers
}

/// A call of a child's method, from the parent's web (wait off, the answer to `answers`, the progress to `progress`).
fn call_child(
    h: &Arc<Host>,
    cmd: &[u8],
    tx: &[u8],
    progress: Arc<std::sync::Mutex<Vec<(usize, usize)>>>,
) -> std::sync::mpsc::Receiver<(nrpc::reply::Reply, u16)> {
    let (send, answers) = std::sync::mpsc::channel();
    let called = nrpc::call::Calls::process().call(nrpc::call::CallSpec {
        owner: Some((h.functions(), "child")),
        cmd,
        source: b"from-web",
        user_access: 0,
        timeout_s: 0,
        wait: false,
        allow_restricted: true,
        call_id: Some(tx),
        payload: None,
        reply: nrpc::reply::Reply::new(nrpc::reply::ContentType::TextPlain),
        done: Some(Box::new(move |reply, code| {
            let _ = send.send((reply, code));
        })),
        progress: Some(Arc::new(move |_: &[u8; 16], done, all| progress.lock().unwrap().push((done, all)))),
        is_cancelled: None,
    });
    assert_eq!(called.code, 200);
    answers
}

/// A child's FUNCTION is its connection's (`send_to_child`, D164.B1): a call goes down the wire, a progress request
/// only with PROGRESS negotiated (pluginsd_functions.c:469-475), the child's FUNCTION_PROGRESS reaches the caller
/// (missing numbers as 0, :715-736) and its result span answers the call.
#[test]
fn a_childs_function_is_called_through_its_connection() {
    for (caps, tx, progress_goes_down) in
        [(caps::PROGRESS, "5a1e00000000400080000000000000f7", true), (0, "5a1e00000000400080000000000000f8", false)]
    {
        let h = host();
        let wire = Arc::new(TestWire::default());
        let mut p = stream_parser_on(&h, caps, Arc::clone(&wire));
        feed_ok(&mut p, &["FUNCTION GLOBAL \"answer\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
        let progress = Arc::new(std::sync::Mutex::new(Vec::new()));
        let answers = call_child(&h, b"answer now", tx.as_bytes(), Arc::clone(&progress));
        nrpc::call::Calls::process().progress(tx);
        let mut down = format!("FUNCTION {tx} 10 \"answer now\" \"0x0\" \"from-web\"\n");
        if progress_goes_down {
            down += &format!("FUNCTION_PROGRESS {tx}\n");
        }
        assert_eq!(String::from_utf8(std::mem::take(&mut *wire.0.lock().unwrap())).unwrap(), down);
        feed_ok(&mut p, &[format!("FUNCTION_PROGRESS '{tx}'"), format!("FUNCTION_PROGRESS '{tx}' 5 10")]);
        assert_eq!(*progress.lock().unwrap(), [(0, 0), (5, 10)]);
        let expires = netdata_agent_rrd::clock::now_realtime_s() + 60;
        let span = format!("FUNCTION_RESULT_BEGIN '{tx}' 200 application/json {expires}");
        feed_ok(&mut p, &[span, "{}".into(), "FUNCTION_RESULT_END".into()]);
        let (reply, code) = answers.recv().unwrap();
        assert_eq!((code, reply.body.as_slice()), (200, &b"{}\n"[..]));
    }
}

/// A child's FUNCTION sent again with changes (every reconnect, R:137-172): its record carries the line being read, as
/// C's receiver frame gives every record written during a read (stream-receiver.c:991-998).
#[test]
fn a_childs_reregistration_is_recorded_with_its_line() {
    let h = host();
    let mut p = parser(&h);
    feed_ok(&mut p, &["FUNCTION GLOBAL \"x\" 10 \"h\" \"top\" \"0x0\" 100 0".into()]);
    let (_, records) =
        netdata_agent_log::capture(|| feed_all(&mut p, &["FUNCTION GLOBAL \"x\" 20 \"h\" \"top\" \"0x0\" 100 0"]));
    let changed = records
        .into_iter()
        .find(|r| r.message.as_deref().is_some_and(|m| m.contains("re-registered with changes")))
        .unwrap();
    let line = "'FUNCTION' 'GLOBAL' 'x' '20' 'h' 'top' '0x0' '100' '0'".to_string();
    assert!(changed.fields.contains(&(Field::Request, line)), "{:?}", changed.fields);
}

/// The connection's end (`parser_destroy()`): a span cut short answers 503 with what came, another pending call C's
/// "exited before responding" 503, and the methods answer as a transport that is gone.
#[test]
fn a_childs_end_answers_its_pending_calls() {
    let h = host();
    let wire = Arc::new(TestWire::default());
    let mut p = stream_parser_on(&h, caps::PROGRESS, Arc::clone(&wire));
    feed_ok(&mut p, &["FUNCTION GLOBAL \"answer\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    let (cut, waiting) = ("5a1e00000000400080000000000000f9", "5a1e00000000400080000000000000fa");
    let cut_rx = call_child(&h, b"answer", cut.as_bytes(), Arc::default());
    let waiting_rx = call_child(&h, b"answer", waiting.as_bytes(), Arc::default());
    feed_ok(&mut p, &[format!("FUNCTION_RESULT_BEGIN {cut} 200 text/plain 0"), "half".into()]);
    drop(p);
    let (reply, code) = cut_rx.recv().unwrap();
    assert_eq!((code, reply.body.as_slice()), (503, &b"half\n"[..]));
    let (reply, code) = waiting_rx.recv().unwrap();
    let exited = concat!(
        r#"{"status":503,"errorMessage":"#,
        r#""The plugin that was servicing this request, exited before responding."}"#
    );
    assert_eq!((code, String::from_utf8(reply.body).unwrap()), (503, exited.to_string()));
}

/// A plugin's FUNCTION is its transport's: a call goes to its stdin, its result span answers it, the span's END
/// counts as a collection; its FUNCTION_PROGRESS for an unknown call and a result for an unknown one are C's records.
#[test]
fn a_plugin_answers_a_call_through_its_parser() {
    let hosts = plugin_hosts();
    let wire = Arc::new(TestWire::default());
    let mut p = plugin_parser_with_wire(&hosts, &wire);
    feed_ok(&mut p, &["FUNCTION GLOBAL \"answer\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    let tx = "5a1e00000000400080000000000000f1";
    let answers = call_plugin(&hosts, b"answer now", tx.as_bytes());
    assert_eq!(
        String::from_utf8(std::mem::take(&mut *wire.0.lock().unwrap())).unwrap(),
        format!("FUNCTION {tx} 10 \"answer now\" \"0x0\" \"from-parent\"\n")
    );
    let before = p.data_collections_count;
    let lines = [
        format!("FUNCTION_RESULT_BEGIN {tx} 200 application/json {}", netdata_agent_rrd::clock::now_realtime_s() + 60),
        "{\"rows\":[1]}".into(),
        "FUNCTION_RESULT_END".into(),
    ];
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    let (results, records) = netdata_agent_log::capture(|| feed_all(&mut p, &refs));
    assert!(results.iter().all(|&ok| ok) && records.is_empty(), "{records:?}");
    let (reply, code) = answers.recv().unwrap();
    assert_eq!((code, reply.body.as_slice()), (200, &b"{\"rows\":[1]}\n"[..]));
    assert_eq!(p.data_collections_count, before + 1);
    let (_, records) = netdata_agent_log::capture(|| {
        feed_all(&mut p, &[&format!("FUNCTION_PROGRESS {tx} 1 2"), "FUNCTION_RESULT_BEGIN other 200 x 0", "late", "FUNCTION_RESULT_END"])
    });
    let texts: Vec<_> = records.into_iter().filter_map(|r| r.message).collect();
    assert_eq!(
        texts,
        [
            format!("got a FUNCTION_PROGRESS for transaction '{tx}', but the transaction is not found."),
            "got a FUNCTION_RESULT_BEGIN for transaction 'other', but the transaction is not found.".to_string(),
        ]
    );
}

/// The run's end: a span cut short answers 503 with what came; another pending call C's "exited" 503; the
/// parser's methods answer as a transport that is gone.
#[test]
fn a_plugins_end_answers_its_pending_calls() {
    let hosts = plugin_hosts();
    let wire = Arc::new(TestWire::default());
    let mut p = plugin_parser_with_wire(&hosts, &wire);
    feed_ok(&mut p, &["FUNCTION GLOBAL \"answer\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    let (cut, waiting) = ("5a1e00000000400080000000000000f2", "5a1e00000000400080000000000000f3");
    let cut_rx = call_plugin(&hosts, b"answer", cut.as_bytes());
    let waiting_rx = call_plugin(&hosts, b"answer", waiting.as_bytes());
    feed_ok(&mut p, &[format!("FUNCTION_RESULT_BEGIN {cut} 200 text/plain 0"), "half".into()]);
    drop(p);
    let (reply, code) = cut_rx.recv().unwrap();
    assert_eq!((code, reply.body.as_slice()), (503, &b"half\n"[..]));
    let (reply, code) = waiting_rx.recv().unwrap();
    assert_eq!(
        (code, String::from_utf8(reply.body).unwrap()),
        (503, r#"{"status":503,"errorMessage":"The plugin that was servicing this request, exited before responding."}"#.into())
    );
    let (send, after) = std::sync::mpsc::channel();
    let lh = hosts.localhost();
    nrpc::call::Calls::process().call(nrpc::call::CallSpec {
        owner: Some((lh.functions(), "parent")),
        cmd: b"answer",
        source: b"",
        user_access: 0,
        timeout_s: 0,
        wait: false,
        allow_restricted: true,
        call_id: None,
        payload: None,
        reply: nrpc::reply::Reply::new(nrpc::reply::ContentType::TextPlain),
        done: Some(Box::new(move |reply, code| {
            let _ = send.send((reply, code));
        })),
        progress: None,
        is_cancelled: None,
    });
    let (reply, code) = after.recv().unwrap();
    assert_eq!(
        (code, String::from_utf8(reply.body).unwrap()),
        (503, r#"{"status":503,"errorMessage":"The plugin that offered this function is not available."}"#.into())
    );
}

/// Feeds `tx`'s answer in 1 MiB lines until the parser ends the run: the bytes fed and the records.
fn answer_past_the_cap(p: &mut Parser, tx: &str) -> (usize, Vec<String>) {
    feed_ok(p, &[format!("FUNCTION_RESULT_BEGIN {tx} 200 text/plain 0")]);
    let mut line = vec![b'x'; 1 << 20];
    line.push(b'\n');
    let (mut ok, mut fed) = (true, 0);
    let cut = MAX_DEFERRED_SIZE / line.len() + 1;
    let (_, records) = netdata_agent_log::capture(|| {
        // bounded: without the cap the parser buffers every line, and an unbounded loop exhausts memory
        while ok && fed <= cut {
            ok = p.feed(&line);
            fed += 1;
        }
    });
    assert_eq!(fed, cut);
    (fed * line.len(), records.into_iter().filter_map(|r| r.message).collect())
}

/// C's record of an answer past the cap, from `plugin`.
fn too_big(size: usize, plugin: &str, tx: &str) -> String {
    format!(
        "PLUGINSD: deferred response is too big ({size} bytes, limit {MAX_DEFERRED_SIZE} bytes) while waiting for keyword 'FUNCTION_RESULT_END' from plugin '{plugin}' (transaction '{tx}'). Stopping this plugin."
    )
}

/// C's limit on an answer: past it the run ends with C's record naming the end keyword, the plugin and the call.
#[test]
fn an_answer_too_big_ends_the_run() {
    let hosts = plugin_hosts();
    let wire = Arc::new(TestWire::default());
    let mut p = plugin_parser_with_wire(&hosts, &wire);
    feed_ok(&mut p, &["FUNCTION GLOBAL \"answer\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    let tx = "5a1e00000000400080000000000000f4";
    let rx = call_plugin(&hosts, b"answer", tx.as_bytes());
    let (size, texts) = answer_past_the_cap(&mut p, tx);
    assert_eq!(texts, [too_big(size, "difftest.plugin", tx)]);
    drop(p);
    let (reply, code) = rx.recv().unwrap();
    assert_eq!((code, reply.body.len()), (503, size));
}

/// The same cap on a child's connection: the record names no plugin (`cd.filename = NULL`), and the connection's end
/// answers the call 503 with what it had.
#[test]
fn a_childs_answer_too_big_ends_the_connection() {
    let h = host();
    let mut p = stream_parser_on(&h, 0, Arc::new(TestWire::default()));
    feed_ok(&mut p, &["FUNCTION GLOBAL \"answer\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    let tx = "5a1e00000000400080000000000069b5";
    let rx = call_child(&h, b"answer", tx.as_bytes(), Arc::default());
    let (size, texts) = answer_past_the_cap(&mut p, tx);
    assert_eq!(texts, [too_big(size, "", tx)]);
    drop(p);
    let (reply, code) = rx.recv().unwrap();
    assert_eq!((code, reply.body.len()), (503, size));
}

/// `pluginsd_host_define_end()`'s new epoch (`pluginsd_parser.c:313`): a vnode's methods from its earlier definition
/// are unavailable once it is defined again, until registered again.
#[test]
fn a_vnodes_definition_starts_a_new_epoch() {
    let hosts = plugin_hosts();
    let mut p = plugin_parser(&hosts);
    let define = [format!("HOST_DEFINE {VNODE} v1"), "HOST_DEFINE_END".into()];
    feed_ok(&mut p, &define);
    feed_ok(&mut p, &["FUNCTION GLOBAL \"vfn\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    let v = hosts.find_by_guid(VNODE).unwrap();
    assert!(v.functions().available(b"vfn"));
    feed_ok(&mut p, &define);
    assert!(!v.functions().available(b"vfn"), "the earlier definition's");
    feed_ok(&mut p, &["FUNCTION GLOBAL \"vfn\" 10 \"help\" \"top\" \"0x0\" 100 0".into()]);
    assert!(v.functions().available(b"vfn"));
}

