use std::io::Write;

use netdata_agent_rrd::chart::flags;
use netdata_agent_rrd::host::{HostInfo, Hosts};

use super::*;

/// `is_plugin()`: one of the three suffixes after at least one byte.
#[test]
fn plugin_files_are_cs() {
    let cases: [(&[u8], Option<&[u8]>); 9] = [
        (b"go.d.plugin", Some(b"go.d")),
        (b"x_plugin", Some(b"x")),
        (b"x-plugin", Some(b"x")),
        (b".plugin", None),
        (b"plugin", None),
        (b"x.plugin.off", None),
        (b"x.plugins", None),
        (b"x.PLUGIN", None),
        (b"a.plugin", Some(b"a")),
    ];
    for (file, name) in cases {
        assert_eq!(plugin_name(file), name, "{}", text(file));
    }
}

/// `exec <path> <update every> <options>`: empty options leave the trailing space.
#[test]
fn commands_are_cs() {
    assert_eq!(command(b"/p/x.plugin", 1, b""), b"exec /p/x.plugin 1 ");
    assert_eq!(command(b"/p/x.plugin", 0, b"alpha 'b c'"), b"exec /p/x.plugin 0 alpha 'b c'");
    assert_eq!(command(&[b'a'; 9000], 1, b"").len(), CMD_MAX);
}

/// A thread's tag and a module are cut where C's buffers cut them.
#[test]
fn names_are_cut_as_c() {
    assert_eq!(cut("PD[difftest-longname]".into(), THREAD_TAG_MAX), "PD[difftest-lon");
    assert_eq!(cut("PD[go.d]".into(), THREAD_TAG_MAX), "PD[go.d]");
    assert_eq!(cut("PD[ab\u{e9}cdefghijkl]".into(), THREAD_TAG_MAX), "PD[ab\u{e9}cdefghij");
    assert_eq!(cut("PD[abcdefghijk\u{e9}]".into(), THREAD_TAG_MAX), "PD[abcdefghijk");
}

/// C's policy after a run, row by row (`plugins_d.c:61-118,172-185`).
#[test]
fn the_policy_is_cs() {
    let run = |rc, retry, collections, serial, enabled| {
        verdict(&Run {
            rc,
            retry,
            successful_collections: collections,
            serial_failures: serial,
            enabled,
            hostname: "h",
            fullfilename: "/p/x.plugin",
            pid: 7,
        })
    };
    let go = |record: Option<(Priority, &str)>, sleep_every| Verdict {
        record: record.map(|(p, m)| (p, m.to_string())),
        sleep_every,
        disable: false,
    };
    let stop = |priority, message: &str| Verdict {
        record: Some((priority, message.to_string())),
        sleep_every: 0,
        disable: true,
    };
    let who = "PLUGINSD: 'host:h', '/p/x.plugin' (pid 7)";
    let cases = [
        (run(3, true, 0, 0, true), go(None, 1)),
        (run(0, true, 0, 11, true), go(None, 1)),
        (run(0, false, 5, 0, true), go(None, 1)),
        (
            run(0, false, 0, 1, true),
            go(Some((Priority::Info, &format!("{who} does not generate useful output but it reports success (exits with 0). Waiting a bit before starting it again.."))), 10),
        ),
        (
            run(0, false, 0, 10, false),
            go(Some((Priority::Info, &format!("{who} does not generate useful output but it reports success (exits with 0). Will not start it again - it is now disabled.."))), 10),
        ),
        (
            run(0, false, 0, 11, true),
            stop(Priority::Err, "PLUGINSD: 'host:'h', '/p/x.plugin' (pid 7) does not generate useful output, although it reports success (exits with 0).We have tried to collect something 11 times - unsuccessfully. Disabling it."),
        ),
        (run(-1, true, 5, 0, true), stop(Priority::Info, &format!("{who} exited abnormally. Disabling it."))),
        (run(-1, false, 0, 0, true), stop(Priority::Info, &format!("{who} exited abnormally. Disabling it."))),
        (
            run(3, false, 0, 0, true),
            stop(Priority::Err, &format!("{who} exited with error code 3 and haven't collected any data. Disabling it.")),
        ),
        (
            run(3, false, 4, 1, true),
            go(Some((Priority::Err, &format!("{who} exited with error code 3, but has given useful output in the past (4 times). Waiting a bit before starting it again."))), 10),
        ),
        (
            run(3, false, 4, 10, false),
            go(Some((Priority::Err, &format!("{who} exited with error code 3, but has given useful output in the past (4 times). Will not start it again - it is disabled."))), 10),
        ),
        (
            run(3, false, 4, 11, true),
            stop(Priority::Err, &format!("{who} exited with error code 3, but has given useful output in the past (4 times).We tried to restart it 11 times, but it failed to generate data. Disabling it.")),
        ),
    ];
    for (i, (got, want)) in cases.into_iter().enumerate() {
        assert_eq!(got, want, "row {i}");
    }
}

/// `pluginsd_sleep()`: whole 100 ms steps, ended by the first check that fails.
#[test]
fn sleeps_end_on_the_check() {
    let started = Instant::now();
    sleep(0, &|| true);
    sleep(-1, &|| true);
    assert!(started.elapsed() < CHECK_EVERY);
    let started = Instant::now();
    let checks = std::cell::Cell::new(0);
    sleep(1, &|| {
        checks.set(checks.get() + 1);
        checks.get() <= 3
    });
    assert_eq!(checks.get(), 4);
    assert!(started.elapsed() >= CHECK_EVERY * 3 && started.elapsed() < CHECK_EVERY * 5, "{:?}", started.elapsed());
}

fn pipe() -> (File, File) {
    let (r, w) = nix::unistd::pipe().unwrap();
    (File::from(r), File::from(w))
}

fn records(f: impl FnOnce()) -> Vec<(Source, Priority, i32, String)> {
    let ((), records) = netdata_agent_log::capture(f);
    records.into_iter().map(|r| (r.source, r.priority, r.errno, r.message.unwrap_or_default())).collect()
}

/// `buffered_reader_read_timeout()`: the bytes; a hang-up, after the bytes, with C's record and no errno; a
/// cancellation with ECANCELED.
#[test]
fn reads_end_as_cs() {
    let (mut input, mut plugin) = pipe();
    plugin.write_all(b"x\n").unwrap();
    drop(plugin);
    let mut buffer = [0; 16];
    assert_eq!(read(&mut input, &mut buffer, &|| false), Ok(2));
    let mut got = Ok(0);
    let logged = records(|| got = read(&mut input, &mut buffer, &|| false));
    assert_eq!(got, Err(READ_POLLHUP));
    assert_eq!(logged, [(Source::Daemon, Priority::Err, 0, "PARSER: read failed: POLLHUP.".to_string())]);
    let (mut input, _plugin) = pipe();
    let logged = records(|| got = read(&mut input, &mut buffer, &|| true));
    assert_eq!(got, Err(READ_POLL_CANCELLED));
    assert_eq!(
        logged,
        [(Source::Daemon, Priority::Err, Errno::ECANCELED as i32, "PARSER: thread cancelled while waiting for data.".to_string())]
    );
}

/// `send_to_plugin()` to a plugin that is gone: C's warning with the failed write's errno.
#[test]
fn a_failed_send_is_reported_with_its_errno() {
    let (plugin_in, mut output) = pipe();
    drop(plugin_in);
    let fd = output.as_raw_fd();
    let mut sent = 0;
    let logged = records(|| sent = send_to_plugin(&mut output, b"QUIT"));
    assert_eq!(sent, -3);
    assert_eq!(
        logged,
        [(
            Source::Daemon,
            Priority::Warning,
            Errno::EPIPE as i32,
            format!("PLUGINSD: cannot send command to plugin (fd = {fd}, sent bytes = -1 out of 4)")
        )]
    );
}

/// Localhost, the plugins' host.
fn hosts() -> Arc<Hosts> {
    Arc::new(Hosts::new(Host::new(
        "0f4b6e5c-1d2a-4b3c-9d8e-7f6a5b4c3d2e",
        true,
        HostInfo {
            hostname: "box".into(),
            registry_hostname: "box".into(),
            os: "linux".into(),
            timezone: "UTC".into(),
            abbrev_timezone: "UTC".into(),
            utc_offset: 0,
            program_name: "netdata".into(),
            program_version: "v0".into(),
            update_every: 1,
            db_mode: netdata_agent_rrd::mode::DbMode::Ram,
            history_entries: 4096,
            health_enabled: false,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: None,
            cache_dir: None,
        },
    )))
}

fn worker(hosts: &Arc<Hosts>) -> Worker {
    Worker {
        hosts: PluginHosts {
            hosts: Arc::clone(hosts),
            update_every: 1,
            history: 4096,
            replication: true,
            replication_period: 86400,
            replication_step: 3600,
            attach_sender: Arc::new(|_| {}),
        },
        filename: "x.plugin".into(),
        fullfilename: "/p/x.plugin".into(),
        module: "plugins.d[x.plugin]".into(),
        cmd: command(b"/p/x.plugin", 1, b""),
        update_every: 1,
        parser: ParserConfig {
            capabilities: 0,
            update_every: 1,
            page_size: 4096,
            now: netdata_agent_rrd::collection::now_realtime_timeval,
            gap_when_lost_iterations_above: 3,
        },
        successful_collections: 0,
        serial_failures: 0,
        pid: 0,
        state: Arc::new(State { enabled: AtomicBool::new(true), ..State::default() }),
        collectors_cancelled: Arc::new(AtomicBool::new(false)),
    }
}

/// What the plugin wrote, a run over it, and what the run sent back.
fn run(w: &mut Worker, plugin_says: &[u8], hang_up: bool) -> ((u64, bool), Vec<u8>) {
    let (mut input, mut plugin_out) = pipe();
    let (mut plugin_in, output) = pipe();
    let output = PluginWire::new(Some(output));
    plugin_out.write_all(plugin_says).unwrap();
    let plugin_out = if hang_up {
        drop(plugin_out);
        None
    } else {
        // the run reads the lines, then waits until cancelled, the plugin still there
        let state = Arc::clone(&w.state);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            state.cancelled.store(true, Ordering::Release);
        });
        Some(plugin_out)
    };
    let result = w.process(&mut input, &output);
    drop(WireClose(output));
    drop(plugin_out);
    let mut sent = Vec::new();
    std::io::Read::read_to_end(&mut plugin_in, &mut sent).unwrap();
    (result, sent)
}

const COLLECT: &[u8] = b"CHART 'x.a' '' t u f c line 1000 1\nDIMENSION 'd' '' absolute 1 1\nBEGIN 'x.a'\nSET 'd' = 1\nEND\n";

/// `pluginsd_process()`: a plugin that hangs up gets no QUIT; its collections are counted and its charts made
/// obsolete.
#[test]
fn a_run_ending_in_a_hang_up_sends_no_quit() {
    let hosts = hosts();
    let mut w = worker(&hosts);
    let ((count, retry), sent) = run(&mut w, COLLECT, true);
    assert_eq!((count, retry, sent.as_slice()), (1, false, &b""[..]));
    assert_eq!((w.successful_collections, w.serial_failures), (1, 0));
    assert_ne!(hosts.localhost().charts().find("x.a", true).unwrap().flags() & flags::OBSOLETE, 0);
    // a run without data counts a failure
    let ((count, _), _) = run(&mut w, b"", true);
    assert_eq!((count, w.successful_collections, w.serial_failures), (0, 1, 1));
}

/// The run's end lets the vnodes it defined go (C's `pluginsd_process()` after its counters): both flags cleared.
#[test]
fn a_runs_end_takes_its_vnodes_offline() {
    const VNODE: &str = "5a1e0000-0000-4000-8000-0000000000d1";
    let hosts = hosts();
    let mut w = worker(&hosts);
    let says = format!("HOST_DEFINE {VNODE} v1\nHOST_DEFINE_END\n");
    let ((count, _), _) = run(&mut w, says.as_bytes(), true);
    assert_eq!(count, 0);
    let v = hosts.find_by_guid(VNODE).expect("defined");
    assert_eq!((v.is_virtual(), v.collector_online(), v.is_online()), (false, false, false));
}

/// A refused line ends the run with QUIT (4 bytes, no newline); so does the thread's cancellation, after C's record.
#[test]
fn a_refused_line_or_a_cancellation_sends_quit() {
    let hosts = hosts();
    let mut w = worker(&hosts);
    let ((count, _), sent) = run(&mut w, b"DISABLE\n", true);
    assert_eq!((count, sent.as_slice(), w.state.enabled.load(Ordering::Acquire)), (0, &b"QUIT"[..], false));
    let mut w = worker(&hosts);
    let mut outcome = ((0, false), Vec::new());
    let logged = records(|| outcome = run(&mut w, COLLECT, false));
    assert_eq!(outcome, ((1, false), b"QUIT".to_vec()));
    let logged: Vec<_> = logged.into_iter().filter(|r| r.1 != Priority::Debug).collect();
    assert_eq!(
        logged,
        [
            (Source::Daemon, Priority::Err, Errno::ECANCELED as i32, "PARSER: thread cancelled while waiting for data.".to_string()),
            (Source::Collector, Priority::Info, 0, "PLUGINSD: buffered reader not OK (-8)".to_string()),
        ]
    );
}
