//! The transport's contracts (C's `pluginsd_functions-unittest.c` where they apply).

use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::mpsc;

use netdata_agent_nrpc::call::{CallSpec, Clock};
use netdata_agent_nrpc::reply::Payload;
use netdata_agent_nrpc::{Handler, MethodDesc, Registry, Source as NrpcSource, Transport};

use super::*;

/// A clock the tests move.
struct TestClock(Arc<AtomicU64>);

impl Clock for TestClock {
    fn monotonic_ut(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// The plugin's stdin: what was written, and what the next writes return (0: their length).
#[derive(Default)]
struct TestWire {
    written: Mutex<Vec<String>>,
    fail: AtomicIsize,
}

impl Wire for TestWire {
    fn send(&self, text: &[u8]) -> isize {
        lock(&self.written).push(String::from_utf8_lossy(text).into_owned());
        match self.fail.load(Ordering::Relaxed) {
            0 => text.len() as isize,
            code => code,
        }
    }
}

impl TestWire {
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.written))
    }
}

struct Fixture {
    now: Arc<AtomicU64>,
    calls: Arc<Calls>,
    wire: Arc<TestWire>,
    transport: Arc<PluginsdTransport>,
    registry: Registry,
}

const T0: u64 = 1_000_000_000;

fn fixture() -> Fixture {
    let now = Arc::new(AtomicU64::new(T0));
    let calls = Calls::new(Box::new(TestClock(Arc::clone(&now))));
    let wire = Arc::new(TestWire::default());
    let transport = PluginsdTransport::new(Arc::clone(&wire) as Arc<dyn Wire>, Arc::clone(&calls), true);
    let registry = Registry::default();
    for (name, timeout_s) in [(&b"slow"[..], 1), (b"top", 10)] {
        registry
            .register(
                "h",
                &MethodDesc {
                    name,
                    help: b"help",
                    tags: b"",
                    timeout_s,
                    priority: 0,
                    version: 0,
                    access: 0,
                    sync: false,
                    source: NrpcSource::Plugin,
                    handler: Handler::Transport(Arc::clone(&transport) as Arc<dyn Transport>),
                },
            )
            .unwrap();
    }
    Fixture { now, calls, wire, transport, registry }
}

type Answers = mpsc::Receiver<(Reply, u16)>;

impl Fixture {
    /// A no-wait call (a parent's, as a child runs it): its key and where its answer arrives.
    fn call(&self, cmd: &[u8], payload: Option<Payload>) -> (String, Answers) {
        self.call_with(cmd, payload, 0)
    }

    /// `call()` with the caller's timeout (0: the method's).
    fn call_with(&self, cmd: &[u8], payload: Option<Payload>, timeout_s: i32) -> (String, Answers) {
        let (tx, rx) = mpsc::channel();
        let before = self.wire.take();
        let called = self.calls.call(CallSpec {
            owner: Some((&self.registry, "h")),
            cmd,
            source: b"method=api,user=x",
            user_access: 0x13,
            timeout_s,
            wait: false,
            allow_restricted: true,
            call_id: None,
            payload,
            reply: Reply::new(ContentType::TextPlain),
            done: Some(Box::new(move |reply, code| {
                let _ = tx.send((reply, code));
            })),
            progress: None,
            is_cancelled: None,
        });
        let written = self.wire.take();
        lock(&self.wire.written).extend(before.into_iter().chain(written.clone()));
        let key = written.first().map(|l| l.split(' ').nth(1).unwrap().to_string()).unwrap_or_default();
        assert!(called.code == 200 || called.code == 503, "{}", called.code);
        (key, rx)
    }

    fn advance(&self, ut: u64) {
        self.now.fetch_add(ut, Ordering::Relaxed);
    }
}

fn now_s() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64
}

fn json(reply: &Reply) -> String {
    String::from_utf8(reply.body.clone()).unwrap()
}

/// FT 1: the call goes out as C's `FUNCTION` line (the timeout rounded from the deadline, the access as hex, the
/// caller's sanitized source); the plugin's answer comes back with its code, content type and expiry, and the call
/// leaves the table.
#[test]
fn a_call_is_written_and_answered() {
    let f = fixture();
    let (key, answers) = f.call(b"top  full", None);
    assert_eq!(f.wire.take(), [format!("FUNCTION {key} 10 \"top full\" \"0x13\" \"method=api,user=x\"\n")]);
    let expires = now_s() + 30;
    assert!(f.transport.result_begin(key.as_bytes(), 200, Some(b"application/json"), expires, now_s()));
    assert_eq!(f.transport.result_line(key.as_bytes(), b"{\"a\":1}\n"), 8);
    assert!(answers.try_recv().is_err(), "answered at the span's end");
    f.transport.result_end(key.as_bytes());
    let (reply, code) = answers.recv().unwrap();
    assert_eq!(
        (code, json(&reply).as_str(), reply.content_type, reply.expires, reply.cacheable),
        (200, "{\"a\":1}\n", ContentType::ApplicationJson, expires, true)
    );
    assert_eq!(f.calls.deadline(&key), None);
    // an unknown call
    assert!(!f.transport.result_begin(b"nope", 200, None, 0, now_s()));
}

/// C's correction 12: an expiry that passed is none, never cacheable; a format the plugin leaves empty keeps the
/// caller's content type.
#[test]
fn a_past_expiry_is_none() {
    let f = fixture();
    let (key, answers) = f.call(b"top", None);
    assert!(f.transport.result_begin(key.as_bytes(), 202, Some(b""), now_s() - 1, now_s()));
    f.transport.result_end(key.as_bytes());
    let (reply, code) = answers.recv().unwrap();
    assert_eq!((code, reply.expires, reply.cacheable, reply.content_type), (202, 0, false, ContentType::TextPlain));
}

/// FT 4: a payload goes out as `FUNCTION_PAYLOAD` with its content type, its bytes and the end line in one piece; an
/// empty payload is a plain `FUNCTION`.
#[test]
fn a_payload_is_written_in_one_piece() {
    let f = fixture();
    let payload = Payload { body: b"{\"q\":1}".to_vec(), content_type: ContentType::ApplicationJson };
    let (key, _answers) = f.call(b"top", Some(payload));
    assert_eq!(
        f.wire.take(),
        [format!(
            "FUNCTION_PAYLOAD {key} 10 \"top\" \"0x13\" \"method=api,user=x\" \"application/json\"\n{{\"q\":1}}\nFUNCTION_PAYLOAD_END\n"
        )]
    );
    let (key, _answers) = f.call(b"top", Some(Payload { body: Vec::new(), content_type: ContentType::TextPlain }));
    assert_eq!(f.wire.take(), [format!("FUNCTION {key} 10 \"top\" \"0x13\" \"method=api,user=x\"\n")]);
}

/// FT 6 and C's correction 18: a later call collects the expired ones after its own line; the cancels go out in the
/// calls' order and the 504s in the reverse one; a call answered with a body before its expiry keeps it.
#[test]
fn a_later_call_collects_the_expired_ones() {
    let f = fixture();
    let (a1, first) = f.call(b"slow", None);
    let (a2, second) = f.call(b"slow", None);
    f.wire.take();
    // past the deadline (1 s) and its grace (1 s)
    f.advance(2_000_001);
    let (b, _later) = f.call(b"top", None);
    assert_eq!(
        f.wire.take(),
        [
            format!("FUNCTION {b} 10 \"top\" \"0x13\" \"method=api,user=x\"\n"),
            format!("FUNCTION_CANCEL {a1}\n"),
            format!("FUNCTION_CANCEL {a2}\n"),
        ]
    );
    for rx in [&first, &second] {
        let (reply, code) = rx.try_recv().unwrap();
        assert_eq!(
            (code, json(&reply)),
            (504, r#"{"status":504,"errorMessage":"Timeout waiting for a response."}"#.into())
        );
    }
    assert_eq!(lock(&f.transport.table).calls.len(), 1);
}

/// The GC answers in the reverse of the calls' order (C prepends its victims and delivers from the head).
#[test]
fn the_gc_answers_in_reverse() {
    let f = fixture();
    let delivered = Arc::new(Mutex::new(Vec::new()));
    let mut keys = Vec::new();
    for _ in 0..3 {
        let delivered = Arc::clone(&delivered);
        let called = f.calls.call(CallSpec {
            owner: Some((&f.registry, "h")),
            cmd: b"slow",
            source: b"",
            user_access: 0,
            timeout_s: 0,
            wait: false,
            allow_restricted: false,
            call_id: None,
            payload: None,
            reply: Reply::new(ContentType::TextPlain),
            done: Some(Box::new(move |reply, _| lock(&delivered).push(reply.body))),
            progress: None,
            is_cancelled: None,
        });
        assert_eq!(called.code, 200);
        let line = f.wire.take().pop().unwrap();
        keys.push(line.split(' ').nth(1).unwrap().to_string());
    }
    // each answer names its call: the GC's 504 replaced nothing it could tell apart, so mark them first
    for (i, key) in keys.iter().enumerate() {
        assert!(f.transport.result_begin(key.as_bytes(), 400, None, 0, now_s()));
        f.transport.result_line(key.as_bytes(), format!("{i}\n").as_bytes());
        // let the span go without answering: the record stays, its body kept (a 4xx is not replaced)
        let mut t = lock(&f.transport.table);
        t.calls.iter_mut().find(|p| &*p.key == key.as_str()).unwrap().held = false;
    }
    f.advance(2_000_001);
    f.call(b"top", None);
    let cancels: Vec<_> = f.wire.take().into_iter().filter(|l| l.starts_with("FUNCTION_CANCEL")).collect();
    assert_eq!(cancels, keys.iter().map(|k| format!("FUNCTION_CANCEL {k}\n")).collect::<Vec<_>>());
    assert_eq!(*lock(&delivered), [b"2\n".to_vec(), b"1\n".to_vec(), b"0\n".to_vec()]);
}

/// FT 8: a send that fails answers 503 with C's text at once, with C's record, and collects.
#[test]
fn a_failed_send_answers_503() {
    let f = fixture();
    f.wire.fail.store(-3, Ordering::Relaxed);
    let ((key, answers), records) = netdata_agent_log::capture(|| f.call(b"top", None));
    assert!(!key.is_empty());
    let (reply, code) = answers.recv().unwrap();
    assert_eq!(
        (code, json(&reply)),
        (503, r#"{"status":503,"errorMessage":"Failed to send this request to the plugin that offered it."}"#.into())
    );
    let errors: Vec<_> = records.into_iter().filter(|r| r.priority == Priority::Err).filter_map(|r| r.message).collect();
    assert_eq!(errors, ["PLUGINSD: FUNCTION 'top': failed to send it to the plugin, error -3"]);
    assert!(lock(&f.transport.table).calls.is_empty());
}

/// FT 9: the call table's cancel and progress reach the plugin while the call is pending; C's records otherwise.
#[test]
fn cancel_and_progress_reach_the_plugin() {
    let f = fixture();
    let (key, _answers) = f.call(b"top", None);
    f.wire.take();
    f.calls.progress(&key);
    f.calls.cancel(&key);
    assert_eq!(f.wire.take(), [format!("FUNCTION_PROGRESS {key}\n"), format!("FUNCTION_CANCEL {key}\n")]);
    let debug = |run: &dyn Fn()| -> Vec<String> {
        let ((), records) = netdata_agent_log::capture(run);
        records.into_iter().filter(|r| r.message.as_deref().is_some_and(|m| m.starts_with("PLUGINSD"))).filter_map(|r| r.message).collect()
    };
    assert_eq!(
        debug(&|| f.transport.cancel("other")),
        ["PLUGINSD: FUNCTION_CANCEL request didn't match any pending function requests in pluginsd.d."]
    );
    assert_eq!(
        debug(&|| f.transport.progress("other")),
        ["PLUGINSD: FUNCTION_PROGRESS request for transaction 'other' that is not in progress!"]
    );
    assert_eq!(debug(&|| f.transport.progress("")), ["PLUGINSD: FUNCTION_PROGRESS request without transaction!"]);
    f.transport.shutdown();
    assert_eq!(
        debug(&|| f.transport.cancel(&key)),
        [format!("PLUGINSD: FUNCTION_CANCEL for transaction '{key}', but the plugin is not running.")]
    );
    assert_eq!(
        debug(&|| f.transport.progress(&key)),
        [format!("PLUGINSD: FUNCTION_PROGRESS for transaction '{key}', but the plugin is not running.")]
    );
}

/// FT 11 and C's correction 13: the run's end answers what is pending (C's "exited" text without a body), a span cut
/// short with a 2xx as 503 with its partial body; a call after it gets C's dead-transport answer.
#[test]
fn the_runs_end_answers_what_is_pending() {
    let f = fixture();
    let (waiting, waiting_rx) = f.call(b"top", None);
    let (cut, cut_rx) = f.call(b"top", None);
    assert!(f.transport.result_begin(cut.as_bytes(), 200, Some(b"text/plain"), 0, now_s()));
    f.transport.result_line(cut.as_bytes(), b"half\n");
    f.transport.release_span(cut.as_bytes());
    f.transport.shutdown();
    let (reply, code) = waiting_rx.recv().unwrap();
    assert_eq!(
        (code, json(&reply)),
        (503, r#"{"status":503,"errorMessage":"The plugin that was servicing this request, exited before responding."}"#.into())
    );
    let (reply, code) = cut_rx.recv().unwrap();
    assert_eq!((code, json(&reply).as_str()), (503, "half\n"));
    let _ = waiting;
    let (_, after) = f.call(b"top", None);
    let (reply, code) = after.recv().unwrap();
    assert_eq!(
        (code, json(&reply)),
        (503, r#"{"status":503,"errorMessage":"The plugin that offered this function is not available."}"#.into())
    );
}

/// D147.6: a call whose deadline passes inside its span is answered 504 in its place at the span's end (C's GC
/// replaces a 2xx), the span's later lines going nowhere; one whose span already carries an error keeps it.
#[test]
fn a_call_expiring_in_its_span_is_answered_at_its_end() {
    let f = fixture();
    let (ok, ok_rx) = f.call(b"slow", None);
    let (failed, failed_rx) = f.call(b"slow", None);
    assert!(f.transport.result_begin(ok.as_bytes(), 200, Some(b"application/json"), 0, now_s()));
    f.transport.result_line(ok.as_bytes(), b"[1,\n");
    f.advance(2_000_001);
    assert!(f.transport.result_begin(failed.as_bytes(), 400, Some(b"text/plain"), 0, now_s()));
    f.transport.result_line(failed.as_bytes(), b"bad request\n");
    f.wire.take();
    f.call(b"top", None);
    let cancels: Vec<_> = f.wire.take().into_iter().filter(|l| l.starts_with("FUNCTION_CANCEL")).collect();
    assert_eq!(cancels, [format!("FUNCTION_CANCEL {ok}\n"), format!("FUNCTION_CANCEL {failed}\n")]);
    assert!(ok_rx.try_recv().is_err() && failed_rx.try_recv().is_err(), "both spans hold their calls");
    let timeout = r#"{"status":504,"errorMessage":"Timeout waiting for a response."}"#;
    assert_eq!(f.transport.result_line(ok.as_bytes(), b"2]\n"), timeout.len() + 3, "dropped, counted as C's buffer");
    f.transport.result_end(ok.as_bytes());
    f.transport.result_line(failed.as_bytes(), b"more\n");
    f.transport.result_end(failed.as_bytes());
    let (reply, code) = ok_rx.recv().unwrap();
    assert_eq!((code, json(&reply)), (504, r#"{"status":504,"errorMessage":"Timeout waiting for a response."}"#.into()));
    let (reply, code) = failed_rx.recv().unwrap();
    assert_eq!((code, json(&reply).as_str()), (400, "bad request\nmore\n"));
}

/// The plugin's FUNCTION_PROGRESS reaches the caller's progress callback with the call's id.
#[test]
fn the_plugins_progress_reaches_the_caller() {
    let f = fixture();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let (tx, _rx) = mpsc::channel::<(Reply, u16)>();
    let progress: ProgressCb = {
        let seen = Arc::clone(&seen);
        Arc::new(move |id: &[u8; 16], done, all| lock(&seen).push((*id, done, all)))
    };
    let called = f.calls.call(CallSpec {
        owner: Some((&f.registry, "h")),
        cmd: b"top",
        source: b"",
        user_access: 0,
        timeout_s: 0,
        wait: false,
        allow_restricted: false,
        call_id: Some(b"0a0b0c0d0e0f40118213141516171819"),
        payload: None,
        reply: Reply::new(ContentType::TextPlain),
        done: Some(Box::new(move |reply, code| {
            let _ = tx.send((reply, code));
        })),
        progress: Some(progress),
        is_cancelled: None,
    });
    assert_eq!(called.code, 200);
    assert!(f.transport.progress_from_plugin(b"0a0b0c0d0e0f40118213141516171819", 3, 10));
    assert!(!f.transport.progress_from_plugin(b"0A0B0C0D0E0F40118213141516171819", 3, 10), "exact keys");
    let id = [0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x40, 0x11, 0x82, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19];
    assert_eq!(*lock(&seen), [(id, 3, 10)]);
    f.transport.shutdown();
}

/// R63-1: a call the GC collects inside its answer's span is unlinked in C (`dictionary_del` under the span's hold),
/// so the caller's cancel and progress pings and the plugin's own progress miss it: nothing reaches stdin.
#[test]
fn a_call_the_gc_took_from_its_span_is_beyond_the_hooks() {
    let f = fixture();
    let (a, _a_rx) = f.call(b"slow", None);
    assert!(f.transport.result_begin(a.as_bytes(), 200, None, 0, now_s()));
    f.advance(2_000_001);
    f.call(b"top", None);
    let lines = f.wire.take();
    assert!(lines.contains(&format!("FUNCTION_CANCEL {a}\n")), "{lines:?}");
    f.calls.cancel(&a);
    f.calls.progress(&a);
    assert_eq!(f.wire.take(), Vec::<String>::new());
    assert!(!f.transport.progress_from_plugin(a.as_bytes(), 1, 2));
    f.transport.shutdown();
}

/// R63 M7: a span-held call the GC collected is not collected again by a later GC: one CANCEL.
#[test]
fn a_call_the_gc_took_from_its_span_is_cancelled_once() {
    let f = fixture();
    let (a, _a_rx) = f.call(b"slow", None);
    assert!(f.transport.result_begin(a.as_bytes(), 200, None, 0, now_s()));
    f.advance(2_000_001);
    let (d, _d_rx) = f.call(b"slow", None);
    f.wire.take();
    f.advance(2_000_001);
    f.call(b"top", None);
    let cancels: Vec<_> = f.wire.take().into_iter().filter(|l| l.starts_with("FUNCTION_CANCEL")).collect();
    assert_eq!(cancels, [format!("FUNCTION_CANCEL {d}\n")]);
    f.transport.shutdown();
}

/// R63-2: once the GC's 504 replaced a held answer, the span's later lines are dropped but counted as C's buffer
/// holds them (the 504 body, then the lines), so the parser's 100 MiB cap stops the plugin as C's does.
#[test]
fn a_replaced_answer_still_counts_its_lines() {
    let f = fixture();
    let (a, a_rx) = f.call(b"slow", None);
    assert!(f.transport.result_begin(a.as_bytes(), 200, None, 0, now_s()));
    assert_eq!(f.transport.result_line(a.as_bytes(), b"abc\n"), 4);
    f.advance(2_000_001);
    f.call(b"top", None);
    let body = r#"{"status":504,"errorMessage":"Timeout waiting for a response."}"#;
    assert_eq!(f.transport.result_line(a.as_bytes(), b"defgh\n"), body.len() + 6);
    assert_eq!(f.transport.result_line(a.as_bytes(), b"ij\n"), body.len() + 9);
    f.transport.result_end(a.as_bytes());
    let (reply, code) = a_rx.try_recv().unwrap();
    assert_eq!((code, json(&reply)), (504, body.into()));
    f.transport.shutdown();
}

/// R63 M5, FT 13's boundary: a call whose deadline plus grace equals now is not expired; a microsecond later it is.
#[test]
fn the_gc_boundary_is_strict() {
    let f = fixture();
    let (_b, _b_rx) = f.call(b"slow", None);
    let (a, a_rx) = f.call_with(b"slow", None, 2);
    f.advance(3_000_000);
    f.call(b"top", None);
    assert!(a_rx.try_recv().is_err(), "deadline + grace == now: not expired");
    f.wire.take();
    f.advance(1);
    f.call(b"top", None);
    assert!(f.wire.take().contains(&format!("FUNCTION_CANCEL {a}\n")));
    assert_eq!(a_rx.try_recv().map(|(_, code)| code).ok(), Some(504));
    f.transport.shutdown();
}

/// R63 M27: the GC keeps the earliest deadline of the calls it leaves, so a survivor that expires before the next
/// call's own deadline is collected at that call (the GC's smaller-timeout update in C).
#[test]
fn the_gc_remembers_the_survivors_deadlines() {
    let f = fixture();
    let (_a, _a_rx) = f.call_with(b"slow", None, 1);
    let (b, b_rx) = f.call_with(b"slow", None, 5);
    f.advance(2_500_000);
    f.call_with(b"top", None, 10);
    f.wire.take();
    f.advance(4_000_000);
    f.call_with(b"top", None, 10);
    assert!(f.wire.take().contains(&format!("FUNCTION_CANCEL {b}\n")));
    assert_eq!(b_rx.try_recv().map(|(_, code)| code).ok(), Some(504));
    f.transport.shutdown();
}

/// The plugin's stdin for the next test: its first write waits for the test's release, then fails.
struct BlockingWire {
    entered: Mutex<Option<mpsc::Sender<()>>>,
    release: Mutex<Option<mpsc::Receiver<()>>>,
}

impl Wire for BlockingWire {
    fn send(&self, text: &[u8]) -> isize {
        let entered = lock(&self.entered).take();
        if let Some(entered) = entered {
            entered.send(()).unwrap();
            lock(&self.release).take().unwrap().recv().unwrap();
            return -3;
        }
        text.len() as isize
    }
}

/// R63 M23: the run's end waits for a dispatch in flight before it sweeps, so a send that fails meanwhile answers its
/// own call ("Failed to send", under C's dispatcher ref), not the sweep's "exited".
#[test]
fn the_runs_end_waits_for_a_dispatch_in_flight() {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let wire = Arc::new(BlockingWire { entered: Mutex::new(Some(entered_tx)), release: Mutex::new(Some(release_rx)) });
    let calls = Calls::new(Box::new(TestClock(Arc::new(AtomicU64::new(T0)))));
    let transport = PluginsdTransport::new(wire as Arc<dyn Wire>, Arc::clone(&calls), true);
    let registry = Arc::new(Registry::default());
    registry
        .register(
            "h",
            &MethodDesc {
                name: b"top",
                help: b"help",
                tags: b"",
                timeout_s: 10,
                priority: 0,
                version: 0,
                access: 0,
                sync: false,
                source: NrpcSource::Plugin,
                handler: Handler::Transport(Arc::clone(&transport) as Arc<dyn Transport>),
            },
        )
        .unwrap();
    let (answer_tx, answer_rx) = mpsc::channel();
    let caller = {
        let (calls, registry) = (Arc::clone(&calls), Arc::clone(&registry));
        std::thread::spawn(move || {
            calls
                .call(CallSpec {
                    owner: Some((&registry, "h")),
                    cmd: b"top",
                    source: b"",
                    user_access: 0,
                    timeout_s: 0,
                    wait: false,
                    allow_restricted: true,
                    call_id: None,
                    payload: None,
                    reply: Reply::new(ContentType::TextPlain),
                    done: Some(Box::new(move |reply, code| {
                        let _ = answer_tx.send((reply, code));
                    })),
                    progress: None,
                    is_cancelled: None,
                })
                .code
        })
    };
    entered_rx.recv().unwrap();
    let ending = {
        let transport = Arc::clone(&transport);
        std::thread::spawn(move || transport.shutdown())
    };
    while transport.gate.is_alive() {
        std::thread::yield_now();
    }
    // the run's end now waits in the gate's retire for the dispatch's pass
    std::thread::sleep(std::time::Duration::from_millis(50));
    release_tx.send(()).unwrap();
    assert_eq!(caller.join().unwrap(), 503);
    ending.join().unwrap();
    let (reply, code) = answer_rx.recv().unwrap();
    assert_eq!(
        (code, json(&reply)),
        (503, r#"{"status":503,"errorMessage":"Failed to send this request to the plugin that offered it."}"#.into())
    );
}
