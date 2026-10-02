use std::sync::Arc;
use std::sync::atomic::Ordering;

use netdata_agent_log::{Captured, Priority};
use netdata_agent_nrpc::testing::inert;
use netdata_agent_nrpc::{MethodDesc, Source as NrpcSource};

use super::*;
use crate::chart::{Algorithm, ChartSpec};
use crate::collection::{set_value, set_value_float, timed_done};
use crate::host::StreamSend;
use crate::mode::DbMode;
use crate::testutil::{chart_spec, info};

const T: i64 = 1_700_000_000;

use crate::testing::Recorder;

/// Readable numbers: no SLOTS, no IEEE754.
const PLAIN: u32 = caps::INTERPOLATED | caps::CLABELS | caps::HLABELS | caps::CLAIM | caps::FUNCTIONS;

/// A localhost that streams with `pattern` as `send charts matching`, its sender negotiated `capabilities`.
fn streaming(pattern: &str, capabilities: u32) -> (Host, Arc<Recorder>) {
    let mut i = info("child");
    i.stream_send = StreamSend::new(true, "parent:19999", "key", pattern);
    let host = Host::new("guid-s", true, i);
    let recorder = Arc::new(Recorder::with_capabilities(capabilities));
    host.set_upstream(Arc::clone(&recorder) as Arc<dyn Upstream>);
    (host, recorder)
}

fn ready(host: &Host) {
    host.sender_flags_set(sender_flags::ADDED | sender_flags::CONNECTED | sender_flags::READY_4_METRICS);
}

/// A collection at `t` of every dimension given, as a plugin's BEGIN/SET/END (pulse's `rrdset_done()` shape).
fn collect(host: &Host, chart: &Chart, values: &[(&Arc<Dim>, i64)], t: i64) {
    for (dim, v) in values {
        set_value(dim, (t, 0), *v);
    }
    let pending = chart.collection().counter_done != 0;
    timed_done(host, chart, (t, 0), pending, 3, BufferSource::Thread);
}

fn texts(records: &[Captured]) -> Vec<(Priority, String)> {
    records.iter().map(|r| (r.priority, r.message.clone().unwrap_or_default())).collect()
}

fn definitions(commits: &[(Traffic, String)]) -> usize {
    commits.iter().filter(|(t, s)| *t == Traffic::Metadata && s.starts_with("CHART ")).count()
}

/// A definition sent again while the chart's replication runs (a new dimension) finds the claim taken: its count
/// is given back at once, the host still replicating.
#[test]
fn a_redefinition_while_replicating_keeps_one_claim() {
    let (host, recorder) = streaming("*", PLAIN | caps::REPLICATION);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (d1, _) = chart.dim_add("d1", None, 1, 1, Algorithm::Absolute);
    collect(&host, &chart, &[(&d1, 1)], T);
    assert_eq!(host.sender_replicating_charts(), 1);
    let (d2, _) = chart.dim_add("d2", None, 1, 1, Algorithm::Absolute);
    recorder.take();
    collect(&host, &chart, &[(&d1, 1), (&d2, 1)], T + 1);
    let commits = recorder.take();
    assert_eq!((commits.len(), definitions(&commits)), (1, 1), "the definition alone: {commits:?}");
    assert!(commits[0].1.contains("DIMENSION \"d2\" ") && commits[0].1.contains("\nCHART_DEFINITION_END "));
    assert_eq!(host.sender_replicating_charts(), 1);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_REPLICATING);
    assert_eq!(
        chart.flags() & (flags::SENDER_REPLICATION_FINISHED | flags::SENDER_REPLICATION_IN_PROGRESS),
        flags::SENDER_REPLICATION_IN_PROGRESS
    );
}

/// A claim found taken is not undone by the thread that lost it, even when the sender stopped meanwhile: the claim
/// and its count stay with the thread that made them.
#[test]
fn a_lost_claim_is_not_undone_when_the_sender_stopped() {
    let (host, recorder) = streaming("*", PLAIN | caps::REPLICATION);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    // another thread's claim, made before this sender stopped
    host.sender_replicating_charts_plus_one();
    chart.flags_set_and_clear(flags::SENDER_REPLICATION_IN_PROGRESS, flags::SENDER_REPLICATION_FINISHED);
    let pulse = host.pulse_state();
    let mut out = Vec::new();
    assert!(!send_definition(&host, recorder.as_ref(), &chart, &mut out));
    assert_eq!(host.sender_replicating_charts(), 1);
    assert_eq!(host.pulse_state(), pulse);
    assert_eq!(
        chart.flags() & (flags::SENDER_REPLICATION_FINISHED | flags::SENDER_REPLICATION_IN_PROGRESS),
        flags::SENDER_REPLICATION_IN_PROGRESS
    );
}

/// The gate: until the sender is ready the host is queued once while its collection is online and says so once;
/// the first collection after it is ready says that, once.
#[test]
fn the_gate_waits_for_the_sender_and_records_each_transition_once() {
    let (host, recorder) = streaming("*", PLAIN);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let ((), records) = netdata_agent_log::capture(|| {
        collect(&host, &chart, &[(&dim, 1)], T);
        collect(&host, &chart, &[(&dim, 1)], T + 1);
    });
    let not_ready = (
        Priority::Info,
        "STREAM SND 'child': streaming is not ready, not sending data to a parent...".to_string(),
    );
    assert_eq!(texts(&records), vec![not_ready.clone()]);
    assert_eq!(recorder.starts.load(Ordering::Relaxed), 2, "queued at each collection until added");
    host.sender_flags_set(sender_flags::ADDED);
    collect(&host, &chart, &[(&dim, 1)], T + 2);
    assert_eq!(recorder.starts.load(Ordering::Relaxed), 2);
    assert!(recorder.take().is_empty());
    ready(&host);
    let ((), records) = netdata_agent_log::capture(|| {
        collect(&host, &chart, &[(&dim, 1)], T + 3);
        collect(&host, &chart, &[(&dim, 1)], T + 4);
    });
    assert_eq!(
        texts(&records),
        vec![(Priority::Info, "STREAM SND 'child': streaming is ready, sending metrics to parent...".to_string())]
    );
    let commits = recorder.take();
    assert_eq!(definitions(&commits), 1);
    assert_eq!(commits[0].0, Traffic::Metadata);
    assert!(commits[1..].iter().all(|(t, _)| *t == Traffic::Data));
}

/// The definition, then the stored points of each collection: one BEGIN2 block per point, `#` for the wall clock's
/// second and for a value equal to its baseline (the previous collection's value); the first collection stores
/// nothing.
#[test]
fn a_collection_sends_its_definition_then_its_stored_points() {
    let (host, recorder) = streaming("*", PLAIN);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (d1, _) = chart.dim_add("d1", Some("one"), 1, 1, Algorithm::Absolute);
    let (d2, _) = chart.dim_add("d2", None, 1, 1, Algorithm::Absolute);
    for (i, (a, b)) in [(7, 1), (7, 2), (9, 2)].into_iter().enumerate() {
        collect(&host, &chart, &[(&d1, a), (&d2, b)], T + i as i64);
    }
    let commits = recorder.take();
    let t1 = format!("0x{:X}", T + 1);
    let t2 = format!("0x{:X}", T + 2);
    assert_eq!(
        commits,
        vec![
            (
                Traffic::Metadata,
                "CHART \"t.c\" \"\" \"t\" \"u\" \"t\" \"t.c\" \"line\" 1 1 \"  \" \"p\" \"\"\n\
                 CLABEL \"_collect_plugin\" \"p\" 1\n\
                 CLABEL \"_collect_module\" \"[none]\" 1\n\
                 CLABEL_COMMIT\n\
                 DIMENSION \"d1\" \"one\" \"absolute\" 1 1 \"   type=int\"\n\
                 DIMENSION \"d2\" \"d2\" \"absolute\" 1 1 \"   type=int\"\n"
                    .to_string()
            ),
            (Traffic::Data, format!("BEGIN2 't.c' 0x1 {t1} #\nSET2 'd1' 0x7 # A\nSET2 'd2' 0x1 2 A\nEND2\n")),
            (Traffic::Data, format!("BEGIN2 't.c' 0x1 {t2} #\nSET2 'd1' 0x7 9 A\nSET2 'd2' 0x2 # A\nEND2\n")),
        ]
    );
    assert!(chart.is_exposed_upstream());
}

/// The definition goes again after any change a parent must see: a new dimension, a rename, a connection's reset.
#[test]
fn a_chart_is_defined_again_after_each_change() {
    let (host, recorder) = streaming("*", PLAIN);
    ready(&host);
    let named = ChartSpec { name: Some("n1"), ..chart_spec(DbMode::Ram) };
    let chart = host.charts().create(&named).0;
    let (d1, _) = chart.dim_add("d1", None, 1, 1, Algorithm::Absolute);
    let mut t = T;
    let mut run = |values: &[(&Arc<Dim>, i64)]| {
        collect(&host, &chart, values, t);
        t += 1;
        definitions(&recorder.take())
    };
    assert_eq!((run(&[(&d1, 1)]), run(&[(&d1, 1)])), (1, 0));
    let (d2, _) = chart.dim_add("d2", None, 1, 1, Algorithm::Absolute);
    assert_eq!((run(&[(&d1, 1), (&d2, 1)]), run(&[(&d1, 1), (&d2, 1)])), (1, 0), "a new dimension");
    host.charts().create(&ChartSpec { name: Some("n2"), ..chart_spec(DbMode::Ram) });
    assert_eq!(chart.flags() & (flags::UPSTREAM_SEND | flags::UPSTREAM_IGNORE), 0, "the verdict is taken again");
    assert_eq!((run(&[(&d1, 1), (&d2, 1)]), run(&[(&d1, 1), (&d2, 1)])), (1, 0), "a rename");
    reset_charts(&host);
    assert!(!d1.is_exposed_upstream(chart.version()));
    assert_eq!((run(&[(&d1, 1), (&d2, 1)]), run(&[(&d1, 1), (&d2, 1)])), (1, 0), "a reset");
}

/// `send charts matching` on the context, then the name, then the id: a negative match anywhere ignores the chart,
/// a positive one is needed; the verdict stays on the chart.
#[test]
fn the_charts_pattern_matches_context_then_name_then_id() {
    let cases = [
        ("*", true),
        ("", false),
        ("t.c", true),
        ("!t.c *", false),
        ("t.named", true),
        ("!t.named *", false),
        ("!ctx *", false),
        ("other", false),
    ];
    for (pattern, sent) in cases {
        let (host, _) = streaming(pattern, PLAIN);
        let spec = ChartSpec { name: Some("named"), context: Some("ctx"), ..chart_spec(DbMode::Ram) };
        let chart = host.charts().create(&spec).0;
        assert_eq!(should_send(&host, &chart, chart.flags()), sent, "{pattern}");
        let verdict = if sent { flags::UPSTREAM_SEND } else { flags::UPSTREAM_IGNORE };
        assert_eq!(chart.flags() & (flags::UPSTREAM_SEND | flags::UPSTREAM_IGNORE), verdict, "{pattern}");
    }
    // a chart whose replication from its child is running is never sent
    let (host, _) = streaming("*", PLAIN);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    assert!(!should_send(&host, &chart, chart.flags() & !flags::RECEIVER_REPLICATION_FINISHED));
}

/// With REPLICATION, the definition ends with CHART_DEFINITION_END and claims the chart's replication, and no data
/// follow; the first claim marks the host replicating, an obsolete push gives the claim back and the last one
/// marks it running.
#[test]
fn a_definition_claims_the_replication_and_the_obsolete_push_gives_it_back() {
    let (host, recorder) = streaming("*", PLAIN | caps::REPLICATION);
    ready(&host);
    let c1 = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let c2 = host.charts().create(&ChartSpec { id: "c2", ..chart_spec(DbMode::Ram) }).0;
    let (d1, _) = c1.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let (d2, _) = c2.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&host, &c1, &[(&d1, 1)], T);
    assert_eq!(host.sender_replicating_charts(), 1);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_REPLICATING);
    collect(&host, &c2, &[(&d2, 1)], T);
    collect(&host, &c1, &[(&d1, 1)], T + 1);
    assert_eq!(host.sender_replicating_charts(), 2);
    let commits = recorder.take();
    assert_eq!(commits.len(), 2, "definitions only: {commits:?}");
    assert!(commits[0].1.contains("\nCHART_DEFINITION_END 0 0 "), "no retention yet: {commits:?}");
    assert_eq!(
        c1.flags() & (flags::SENDER_REPLICATION_FINISHED | flags::SENDER_REPLICATION_IN_PROGRESS),
        flags::SENDER_REPLICATION_IN_PROGRESS
    );
    c1.is_obsolete(&host);
    assert_eq!(host.sender_replicating_charts(), 1);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_REPLICATING);
    let commits = recorder.take();
    assert_eq!(commits.len(), 1);
    assert!(commits[0].1.starts_with("CHART \"t.c\" \"\" \"t\" \"u\" \"t\" \"t.c\" \"line\" 1 1 \"obsolete  \""));
    assert!(commits[0].1.contains("\nCHART_DEFINITION_END "));
    c2.is_obsolete(&host);
    assert_eq!(host.sender_replicating_charts(), 0);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_RUNNING);
    assert_eq!(c2.flags() & flags::SENDER_REPLICATION_IN_PROGRESS, 0);
}

/// A claim made while the sender stopped (READY cleared by a disconnect) is undone at once.
#[test]
fn a_claim_is_undone_when_the_sender_stopped_meanwhile() {
    let (host, recorder) = streaming("*", PLAIN | caps::REPLICATION);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let mut out = Vec::new();
    assert!(!send_definition(&host, recorder.as_ref(), &chart, &mut out));
    assert_eq!(host.sender_replicating_charts(), 0);
    assert_eq!(host.pulse_state() & host_status::SENDER, host_status::SND_RUNNING);
    assert_eq!(
        chart.flags() & (flags::SENDER_REPLICATION_FINISHED | flags::SENDER_REPLICATION_IN_PROGRESS),
        flags::SENDER_REPLICATION_FINISHED
    );
}

/// The reset takes back only the claims it finds; a residual count is reported and left.
#[test]
fn the_reset_takes_back_the_claims_and_reports_a_residual() {
    let (host, _) = streaming("*", PLAIN | caps::REPLICATION);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&host, &chart, &[(&dim, 1)], T);
    assert_eq!(host.sender_replicating_charts(), 1);
    let ((), records) = netdata_agent_log::capture(|| reset_charts(&host));
    assert_eq!((host.sender_replicating_charts(), texts(&records)), (0, vec![]));
    assert_eq!(chart.resync_time_s(), 0);
    host.sender_replicating_charts_plus_one();
    let ((), records) = netdata_agent_log::capture(|| reset_charts(&host));
    assert_eq!(
        (host.sender_replicating_charts(), texts(&records)),
        (
            1,
            vec![(
                Priority::Warning,
                "STREAM REPLAY: sender replicating-charts counter is 1 after reset (expected 0); leaving it \
                 untouched to preserve any concurrent claim-before-publish in flight"
                    .to_string()
            )]
        )
    );
}

/// Without INTERPOLATED, the collected values go out at every collection, with no time since the last update until
/// the resync horizon: the previous collection's second at the definition plus 3 iterations of 1 s. A chart's first
/// definition reads a zero previous second, so only a definition after a reset waits.
#[test]
fn v1_sends_the_collected_values_with_the_resync_horizon() {
    let (host, recorder) = streaming("*", caps::CLABELS);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let data = |recorder: &Recorder| -> Vec<String> {
        recorder.take().into_iter().filter(|(t, _)| *t == Traffic::Data).map(|(_, s)| s).collect()
    };
    for i in 0..3 {
        collect(&host, &chart, &[(&dim, 10 + i)], T + i);
    }
    // the first collection has no time since the last one, and its time is at .5 s
    assert_eq!(
        data(&recorder),
        vec![
            "BEGIN \"t.c\" 0\nSET \"d\" = 10\nEND\n",
            "BEGIN \"t.c\" 500000\nSET \"d\" = 11\nEND\n",
            "BEGIN \"t.c\" 1000000\nSET \"d\" = 12\nEND\n",
        ]
    );
    reset_charts(&host);
    for i in 3..8 {
        collect(&host, &chart, &[(&dim, 10 + i)], T + i);
    }
    assert_eq!(chart.resync_time_s(), T + 2 + 3);
    let begins: Vec<String> = data(&recorder).iter().map(|s| s.lines().next().unwrap().to_string()).collect();
    assert_eq!(
        begins,
        ["BEGIN \"t.c\" 0", "BEGIN \"t.c\" 0", "BEGIN \"t.c\" 0", "BEGIN \"t.c\" 1000000", "BEGIN \"t.c\" 1000000"]
    );
}

/// A float dimension's baseline is a double with FLOAT_BASELINE, else C's cast to an integer; v1 sends the value
/// the same way.
#[test]
fn a_float_dimension_sends_a_double_only_with_float_baseline() {
    for (extra, set2, v1) in [(0, "SET2 'f' 0x2 2.5 A\n", "SET \"f\" = 3\n"), (caps::FLOAT_BASELINE, "SET2 'f' 2.5 # A\n", "SET \"f\" = 3.5\n")]
    {
        let (host, recorder) = streaming("*", PLAIN | extra);
        ready(&host);
        let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
        let (dim, _) = chart.dim_add("f", None, 1, 1, Algorithm::Absolute);
        dim.update_meta(|m| m.flags |= dim_flags::FLOAT);
        for (i, v) in [2.5, 2.5].into_iter().enumerate() {
            set_value_float(&dim, (T + i as i64, 0), v);
            timed_done(&host, &chart, (T + i as i64, 0), i != 0, 3, BufferSource::Thread);
        }
        let data = recorder.take().into_iter().filter(|(t, _)| *t == Traffic::Data).map(|(_, s)| s).collect::<Vec<_>>();
        assert_eq!(data.len(), 1, "{data:?}");
        assert!(data[0].contains(set2), "{extra:#x}: {data:?}");
        // v1: the same chart without INTERPOLATED
        let (host, recorder) = streaming("*", extra);
        ready(&host);
        let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
        let (dim, _) = chart.dim_add("f", None, 1, 1, Algorithm::Absolute);
        dim.update_meta(|m| m.flags |= dim_flags::FLOAT);
        set_value_float(&dim, (T, 0), 3.5);
        timed_done(&host, &chart, (T, 0), false, 3, BufferSource::Thread);
        let data = recorder.take().into_iter().filter(|(t, _)| *t == Traffic::Data).map(|(_, s)| s).collect::<Vec<_>>();
        assert!(data[0].contains(v1), "{extra:#x}: {data:?}");
    }
}

/// The host's own metadata: labels then OVERWRITE, the claimed id, variables (each VARIABLE line at once, as C's
/// add-then-set always counts as a change; names sanitized), and the functions again at the next collection after
/// they changed, dynamic-configuration ones left out.
#[test]
fn the_host_metadata_goes_when_the_sender_can_take_it() {
    let (host, recorder) = streaming("*", PLAIN);
    send_host_labels(&host);
    host.set_variable("v", 1.0);
    assert!(recorder.take().is_empty(), "not ready");
    ready(&host);
    host.update_labels(|l| l.add(b"k", b"v", crate::labels::SRC_CONFIG));
    send_host_labels(&host);
    send_claimed_id(&host);
    host.set_variable("v", 2.0);
    host.set_variable("v", 2.0);
    host.set_variable("n", f64::NAN);
    host.set_variable("a b", 3.0);
    send_host_variables(&host);
    let commits: Vec<String> = recorder.take().into_iter().map(|(_, s)| s).collect();
    let source = host.labels().iter().next().unwrap().flags;
    assert_eq!(
        commits,
        vec![
            format!("LABEL \"k\" = {source} \"v\"\nOVERWRITE labels\n"),
            "CLAIMED_ID 'guid-s' 'NULL'\n".to_string(),
            "VARIABLE HOST v = 2.0000000\n".to_string(),
            "VARIABLE HOST v = 2.0000000\n".to_string(),
            "VARIABLE HOST n = nan\n".to_string(),
            "VARIABLE HOST a_b = 3.0000000\n".to_string(),
            "VARIABLE HOST v = 2.0000000\nVARIABLE HOST n = nan\nVARIABLE HOST a_b = 3.0000000\n".to_string(),
        ]
    );
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&host, &chart, &[(&dim, 1)], T);
    recorder.take();
    let desc = MethodDesc {
        name: b"f",
        help: b"h",
        tags: b"",
        timeout_s: 10,
        priority: 0,
        version: 3,
        access: 0,
        sync: false,
        source: NrpcSource::Stream,
        handler: netdata_agent_nrpc::Handler::Builtin(inert),
    };
    host.register_function(&desc).unwrap();
    host.register_function(&MethodDesc { name: b"config", ..desc }).unwrap();
    collect(&host, &chart, &[(&dim, 1)], T + 1);
    let commits = recorder.take();
    assert_eq!(commits[0], (Traffic::Metadata, "FUNCTION GLOBAL \"f\" 10 \"h\" \"top\" 0x0 100 3\n".to_string()));
    assert_eq!(host.sender_flags() & sender_flags::GLOBAL_FUNCTIONS_UPDATED, 0);
    collect(&host, &chart, &[(&dim, 1)], T + 2);
    assert!(recorder.take().iter().all(|(t, _)| *t == Traffic::Data));
}

/// The functions' re-list (`stream_send_global_functions()`, `nrpc_catalog_render_global_functions()`,
/// `nrpc-catalog.c:159-171`): the queued removals go as FUNCTION_DEL only to a parent that takes it, and the queue is
/// emptied either way, so a later parent that takes it gets no stale removal; only the available methods go, not one
/// whose serving thread finished.
#[test]
fn the_functions_go_again_with_the_removals_the_parent_takes() {
    let (host, recorder) = streaming("*", PLAIN);
    ready(&host);
    let desc = |name: &'static [u8]| MethodDesc {
        name,
        help: b"h",
        tags: b"",
        timeout_s: 10,
        priority: 0,
        version: 3,
        access: 0,
        sync: false,
        source: NrpcSource::Stream,
        handler: netdata_agent_nrpc::Handler::Builtin(inert),
    };
    host.register_function(&desc(b"f")).unwrap();
    host.register_function(&desc(b"gone")).unwrap();
    std::thread::scope(|s| s.spawn(|| host.register_function(&desc(b"ended")).unwrap()).join().unwrap());
    assert!(matches!(host.unregister_function(b"gone", NrpcSource::Stream), netdata_agent_nrpc::Unregistered::Removed { .. }));
    let f = (Traffic::Metadata, "FUNCTION GLOBAL \"f\" 10 \"h\" \"top\" 0x0 100 3\n".to_string());
    send_global_functions(&host);
    assert_eq!(recorder.take(), std::slice::from_ref(&f), "the available methods, no FUNCTION_DEL without the capability");
    recorder.capabilities.store(PLAIN | caps::FUNCTION_DEL, Ordering::Relaxed);
    send_global_functions(&host);
    assert_eq!(recorder.take(), [f], "the removal was dropped with the first re-list");
    host.unregister_function(b"f", NrpcSource::Stream);
    send_global_functions(&host);
    assert_eq!(recorder.take(), [(Traffic::Metadata, "FUNCTION_DEL GLOBAL \"f\"\n".to_string())]);
}

/// DynCfg's methods go as one `config` line after the others (`stream_send_global_functions()`,
/// `command-function.c:37-42`): only when the host has one and the parent takes DYNCFG.
#[test]
fn the_dyncfg_methods_go_as_one_config_line_to_a_parent_that_takes_dyncfg() {
    let (host, recorder) = streaming("*", PLAIN);
    ready(&host);
    let desc = |name: &'static [u8]| MethodDesc {
        name,
        help: b"h",
        tags: b"",
        timeout_s: 10,
        priority: 0,
        version: 3,
        access: 0,
        sync: false,
        source: NrpcSource::Stream,
        handler: netdata_agent_nrpc::Handler::Builtin(inert),
    };
    host.register_function(&desc(b"f")).unwrap();
    let f = "FUNCTION GLOBAL \"f\" 10 \"h\" \"top\" 0x0 100 3\n";
    recorder.capabilities.store(PLAIN | caps::DYNCFG, Ordering::Relaxed);
    send_global_functions(&host);
    assert_eq!(recorder.take(), [(Traffic::Metadata, f.to_string())], "no DynCfg method, no config line");
    host.register_function(&desc(b"config go.d:x")).unwrap();
    host.register_function(&desc(b"config go.d:y")).unwrap();
    send_global_functions(&host);
    let config = "FUNCTION GLOBAL config 120 \"Dynamic configuration\" \"config\" 0x8 1000\n";
    assert_eq!(recorder.take(), [(Traffic::Metadata, format!("{f}{config}"))]);
    recorder.capabilities.store(PLAIN, Ordering::Relaxed);
    send_global_functions(&host);
    assert_eq!(recorder.take(), [(Traffic::Metadata, f.to_string())], "a parent without DYNCFG");
}

/// A chart variable that changed goes with the chart's next data, after its points.
#[test]
fn a_changed_chart_variable_goes_with_the_next_data() {
    let (host, recorder) = streaming("*", PLAIN);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&host, &chart, &[(&dim, 1)], T);
    collect(&host, &chart, &[(&dim, 1)], T + 1);
    recorder.take();
    chart.set_variable("cv", 5.0);
    collect(&host, &chart, &[(&dim, 1)], T + 2);
    let data = recorder.take();
    assert!(data[0].1.ends_with("VARIABLE CHART cv = 5.0000000\nEND2\n"), "{data:?}");
    assert_eq!(chart.flags() & flags::UPSTREAM_SEND_VARIABLES, 0);
    collect(&host, &chart, &[(&dim, 1)], T + 3);
    assert!(!recorder.take()[0].1.contains("VARIABLE"));
}

/// Slots: every chart takes one, a freed chart gives it back for the next, and none comes back after the host's
/// charts all went; a chart's dimensions count from 1.
#[test]
fn chart_slots_are_reused_until_the_charts_all_go() {
    let (host, _) = streaming("*", PLAIN);
    let charts = host.charts();
    let a = charts.create(&ChartSpec { id: "a", ..chart_spec(DbMode::Ram) }).0;
    let b = charts.create(&ChartSpec { id: "b", ..chart_spec(DbMode::Ram) }).0;
    assert_eq!((a.chart_slot(), b.chart_slot()), (1, 2));
    let (d1, _) = b.dim_add("d1", None, 1, 1, Algorithm::Absolute);
    let (d2, _) = b.dim_add("d2", None, 1, 1, Algorithm::Absolute);
    assert_eq!((d1.slot(), d2.slot()), (1, 2));
    assert!(charts.free_if(&a, |_| true));
    let c = charts.create(&ChartSpec { id: "c", ..chart_spec(DbMode::Ram) }).0;
    assert_eq!(c.chart_slot(), 1);
    charts.flush();
    let d = charts.create(&ChartSpec { id: "d", ..chart_spec(DbMode::Ram) }).0;
    assert_eq!(d.chart_slot(), 3);
}

mod replay;

/// DATA commits of `blocks` through a receiver's forward buffer.
fn hold(up: &dyn Upstream, fwd: &mut ForwardBuffer, blocks: &[&str]) {
    for b in blocks {
        fwd.start(up).extend_from_slice(b.as_bytes());
        fwd.commit(up, Traffic::Data);
    }
}

/// A commit through the forward buffer that is not DATA: `line`, after what the buffer held.
fn metadata(up: &dyn Upstream, fwd: &mut ForwardBuffer, line: &str) {
    fwd.start(up).extend_from_slice(line.as_bytes());
    fwd.commit(up, Traffic::Metadata);
}

/// A receiver's batch: 100 DATA commits are held, empty ones too, and the 101st sends them all in order.
#[test]
fn a_forward_batch_is_101_data_commits() {
    let r = Recorder::default();
    let mut fwd = ForwardBuffer::default();
    let blocks: Vec<String> =
        (0..=BATCH_HELD).map(|i| if i % 10 == 3 { String::new() } else { format!("B{i}\n") }).collect();
    let blocks: Vec<&str> = blocks.iter().map(String::as_str).collect();
    hold(&r, &mut fwd, &blocks[..100]);
    assert!(r.take().is_empty());
    hold(&r, &mut fwd, &blocks[100..]);
    assert_eq!(r.take(), vec![(Traffic::Data, blocks.concat())]);
    hold(&r, &mut fwd, &["C\n"]);
    assert!(r.take().is_empty(), "the next batch starts over");
}

/// Or the DATA commit that brings the pending bytes to 10,836 (`batch`'s 19 blocks hold 10,070 bytes, its 20th
/// sends them).
#[test]
fn a_forward_batch_goes_at_10836_bytes() {
    let r = Recorder::default();
    let mut fwd = ForwardBuffer::default();
    let a = "x".repeat(BATCH_BYTES - 2) + "\n";
    hold(&r, &mut fwd, &[&a]);
    assert!(r.take().is_empty(), "10,835 bytes are held");
    hold(&r, &mut fwd, &["\n"]);
    assert_eq!(r.take(), vec![(Traffic::Data, format!("{a}\n"))]);
}

/// Any other commit through the buffer sends the held DATA ahead of itself, as its own traffic; one committed
/// elsewhere (a host variable's) overtakes them.
#[test]
fn a_metadata_commit_sends_the_held_data_ahead_of_itself() {
    let (host, r) = streaming("*", PLAIN);
    ready(&host);
    let mut fwd = ForwardBuffer::default();
    hold(&*r, &mut fwd, &["B1\n", "B2\n"]);
    host.set_variable("v", 1.0);
    assert_eq!(r.take(), vec![(Traffic::Metadata, "VARIABLE HOST v = 1.0000000\n".to_string())]);
    metadata(&*r, &mut fwd, "M\n");
    assert_eq!(r.take(), vec![(Traffic::Metadata, "B1\nB2\nM\n".to_string())]);
}

/// A flush of the sender's buffer (each connection and disconnection) drops what was held, as C frees the host
/// buffer; a block started before one is refused at its commit (D106.11).
#[test]
fn a_flush_of_the_senders_buffer_drops_the_held_data() {
    let r = Recorder::default();
    let mut fwd = ForwardBuffer::default();
    hold(&r, &mut fwd, &["B1\n"]);
    r.flush_ut.store(5, Ordering::Relaxed);
    hold(&r, &mut fwd, &["B2\n"]);
    metadata(&r, &mut fwd, "M\n");
    assert_eq!(r.take(), vec![(Traffic::Metadata, "B2\nM\n".to_string())]);
    fwd.start(&r).extend_from_slice(b"B3\n");
    r.flush_ut.store(9, Ordering::Relaxed);
    fwd.commit(&r, Traffic::Metadata);
    assert!(r.take().is_empty());
    hold(&r, &mut fwd, &["B4\n"]);
    metadata(&r, &mut fwd, "M\n");
    assert_eq!(r.take(), vec![(Traffic::Metadata, "B4\nM\n".to_string())]);
}

/// The forward gate: the chart's definition goes through the batch, after the held blocks, and so does the
/// functions' resend at the next gate; a forwarded block is closed at its finish and held.
#[test]
fn the_forward_gate_commits_its_metadata_through_the_batch() {
    let (host, r) = streaming("*", PLAIN);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let mut fwd = ForwardBuffer::default();
    hold(&*r, &mut fwd, &["B1\n"]);
    let b = forward_gate(&host, &chart, &mut fwd).unwrap();
    assert!(b.v2 && !b.begin_added);
    let commits = r.take();
    assert_eq!(commits.len(), 1, "{commits:?}");
    assert_eq!(commits[0].0, Traffic::Metadata);
    assert!(commits[0].1.starts_with("B1\nCHART \"t.c\" "), "{commits:?}");
    fwd.bytes().extend_from_slice(b"BEGIN2 't.c' 1 2 #\n");
    forward_finish(&host, &chart, &ProxyBlock { begin_added: true, ..b }, &mut fwd);
    assert!(r.take().is_empty(), "the block is held");
    let desc = MethodDesc {
        name: b"f",
        help: b"h",
        tags: b"",
        timeout_s: 10,
        priority: 0,
        version: 3,
        access: 0,
        sync: false,
        source: NrpcSource::Stream,
        handler: netdata_agent_nrpc::Handler::Builtin(inert),
    };
    host.register_function(&desc).unwrap();
    let b = forward_gate(&host, &chart, &mut fwd).unwrap();
    assert_eq!(
        r.take(),
        vec![(
            Traffic::Metadata,
            "BEGIN2 't.c' 1 2 #\nEND2\nFUNCTION GLOBAL \"f\" 10 \"h\" \"top\" 0x0 100 3\n".to_string()
        )]
    );
    // a block with no BEGIN2 forwarded commits nothing but counts
    forward_finish(&host, &chart, &b, &mut fwd);
    metadata(&*r, &mut fwd, "M\n");
    assert_eq!(r.take(), vec![(Traffic::Metadata, "M\n".to_string())]);
}

/// A v1 child's collections on its receiver (`timed_done` from the forward buffer) are held in the batch, empty
/// commits too.
#[test]
fn a_collection_through_the_forward_buffer_is_batched() {
    let (host, r) = streaming("*", PLAIN);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let mut fwd = ForwardBuffer::default();
    for i in 0..3 {
        set_value(&dim, (T + i, 0), 7);
        timed_done(&host, &chart, (T + i, 0), i != 0, 3, BufferSource::Forward(&mut fwd));
    }
    let commits = r.take();
    assert_eq!((commits.len(), definitions(&commits)), (1, 1), "{commits:?}");
    metadata(&*r, &mut fwd, "M\n");
    let (t1, t2) = (format!("0x{:X}", T + 1), format!("0x{:X}", T + 2));
    assert_eq!(
        r.take(),
        vec![(
            Traffic::Metadata,
            format!(
                "BEGIN2 't.c' 0x1 {t1} #\nSET2 'd' 0x7 # A\nEND2\nBEGIN2 't.c' 0x1 {t2} #\nSET2 'd' 0x7 # A\nEND2\nM\n"
            )
        )]
    );
}

/// A forwarded block to a parent without INTERPOLATED: END2 writes the chart's collected values as v1, into the
/// batch; with INTERPOLATED nothing.
#[test]
fn a_forwarded_block_goes_as_v1_without_interpolated() {
    let (host, r) = streaming("*", PLAIN & !caps::INTERPOLATED);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    let mut fwd = ForwardBuffer::default();
    let b = forward_gate(&host, &chart, &mut fwd).unwrap();
    assert!(!b.v2);
    r.take();
    dim.update_meta(|m| m.flags |= dim_flags::UPDATED);
    forward_v1(&chart, &b, &mut fwd);
    forward_finish(&host, &chart, &b, &mut fwd);
    forward_v1(&chart, &ProxyBlock { v2: true, ..b }, &mut fwd);
    metadata(&*r, &mut fwd, "M\n");
    assert_eq!(r.take(), vec![(Traffic::Metadata, "BEGIN \"t.c\" 0\nSET \"d\" = 0\nEND\nM\n".to_string())]);
}

/// C's free list is a stack (`rrdset-slots.c:10-14`, `:38-39`): the slot freed last is taken first, then the counter
/// goes on; a flush (`rrdhost_pluginsd_send_chart_slots_free()`) empties the list and gives no slot back, then or
/// later.
#[test]
fn chart_slots_are_reused_last_freed_first_and_never_after_a_flush() {
    let (host, _) = streaming("*", PLAIN);
    let charts = host.charts();
    let create = |id: &str| charts.create(&ChartSpec { id, ..chart_spec(DbMode::Ram) }).0;
    let (a, b, c) = (create("a"), create("b"), create("c"));
    assert_eq!((a.chart_slot(), b.chart_slot(), c.chart_slot()), (1, 2, 3));
    assert!(charts.free_if(&a, |_| true));
    assert!(charts.free_if(&c, |_| true));
    let (d, e, f) = (create("d"), create("e"), create("f"));
    assert_eq!((d.chart_slot(), e.chart_slot(), f.chart_slot()), (3, 1, 4));
    assert!(charts.free_if(&d, |_| true));
    charts.flush();
    let g = create("g");
    assert_eq!(g.chart_slot(), 5, "the free list went with the flush");
    assert!(charts.free_if(&g, |_| true));
    assert_eq!(create("h").chart_slot(), 6, "no slot given back after a flush");
}

/// A collection that stores several points sends one BEGIN2 block per point, in one commit: 2.5 s after the last one,
/// the step at T + 2 interpolated (10 + 25 / 2.5) and T + 3 the value, each SET2 against the previous collection's
/// value; a step stored as a gap (3 iterations, not under C's default 3) opens no block.
#[test]
fn a_collection_storing_several_points_sends_a_block_per_point_in_one_commit() {
    let (host, recorder) = streaming("*", PLAIN);
    ready(&host);
    let chart = host.charts().create(&chart_spec(DbMode::Ram)).0;
    let (dim, _) = chart.dim_add("d", None, 1, 1, Algorithm::Absolute);
    collect(&host, &chart, &[(&dim, 7)], T);
    collect(&host, &chart, &[(&dim, 10)], T + 1);
    recorder.take();
    set_value(&dim, (T + 3, 500_000), 35);
    timed_done(&host, &chart, (T + 3, 500_000), true, 3, BufferSource::Thread);
    let (t2, t3, t5) = (format!("0x{:X}", T + 2), format!("0x{:X}", T + 3), format!("0x{:X}", T + 5));
    assert_eq!(
        recorder.take(),
        vec![(
            Traffic::Data,
            format!("BEGIN2 't.c' 0x1 {t2} {t3}\nSET2 'd' 0xA 20 A\nEND2\nBEGIN2 't.c' 0x1 {t3} #\nSET2 'd' 0xA 35 A\nEND2\n")
        )]
    );
    collect(&host, &chart, &[(&dim, 50)], T + 5);
    assert_eq!(recorder.take(), vec![(Traffic::Data, format!("BEGIN2 't.c' 0x1 {t5} #\nSET2 'd' 0x23 50 A\nEND2\n"))]);
}
