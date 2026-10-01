//! The calls' contracts (C's `nrpc-unittest.c` access suite and `pluginsd_functions-unittest.c`'s call-table
//! vectors that need no transport).

use std::sync::atomic::AtomicU64;
use std::sync::mpsc;

use super::*;
use crate::reply::ContentType;
use crate::{MethodDesc, Source};

/// A clock the tests move.
#[derive(Default)]
struct TestClock(Arc<AtomicU64>);

impl Clock for TestClock {
    fn monotonic_ut(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// A transport that keeps its requests for the test to answer, and the hooks it was called through.
#[derive(Default)]
struct Held {
    requests: Mutex<Vec<Request>>,
    hooked: Arc<HookLog>,
}

#[derive(Default)]
struct HookLog(Mutex<Vec<String>>);

impl Hooks for HookLog {
    fn cancel(&self, key: &str) {
        lock(&self.0).push(format!("cancel {key}"));
    }
    fn progress(&self, key: &str) {
        lock(&self.0).push(format!("progress {key}"));
    }
}

impl Transport for Held {
    fn dispatch(&self, req: Request) -> u16 {
        req.call.set_cancel_hook(Arc::clone(&self.hooked) as Arc<dyn Hooks>);
        req.call.set_progress_hook(Arc::clone(&self.hooked) as Arc<dyn Hooks>);
        lock(&self.requests).push(req);
        200
    }
}

use crate::Transport;

fn calls() -> (Arc<Calls>, Arc<AtomicU64>) {
    let now = Arc::new(AtomicU64::new(1_000_000_000));
    (Calls::new(Box::new(TestClock(Arc::clone(&now)))), now)
}

fn register(r: &Registry, name: &'static [u8], handler: Handler, f: impl FnOnce(&mut MethodDesc)) {
    let mut d = MethodDesc {
        name,
        help: b"help",
        tags: b"",
        timeout_s: 10,
        priority: 0,
        version: 0,
        access: 0,
        sync: matches!(handler, Handler::Builtin(_)),
        source: Source::Daemon,
        handler,
    };
    f(&mut d);
    r.register("h", &d).unwrap();
}

fn spec<'a>(r: &'a Registry, cmd: &'a [u8]) -> CallSpec<'a> {
    CallSpec {
        owner: Some((r, "h")),
        cmd,
        source: b"test",
        user_access: 0,
        timeout_s: 0,
        wait: false,
        allow_restricted: false,
        call_id: None,
        payload: None,
        reply: Reply::new(ContentType::TextPlain),
        done: None,
        progress: None,
        is_cancelled: None,
    }
}

/// The caller's callback as a channel.
fn done_channel() -> (Done, mpsc::Receiver<(Reply, u16)>) {
    let (tx, rx) = mpsc::channel();
    (Box::new(move |reply, code| tx.send((reply, code)).unwrap()), rx)
}

fn error_text(reply: &Reply) -> String {
    String::from_utf8(reply.body.clone()).unwrap()
}

fn ok_builtin(reply: &mut Reply, function: &[u8], _: Option<&Payload>, source: &[u8]) -> u16 {
    reply.body = [function, b" from ", source].concat();
    200
}

/// `nrpc_method_authorize()`: C's order and texts, 403 for a signed-in caller and 412 otherwise.
#[test]
fn authorization_answers_as_c() {
    let r = Registry::default();
    register(&r, b"__hidden", Handler::Builtin(ok_builtin), |_| {});
    register(&r, b"sso", Handler::Builtin(ok_builtin), |d| d.access = access::SIGNED_ID | access::VIEW_AGENT_CONFIG);
    register(&r, b"space", Handler::Builtin(ok_builtin), |d| d.access = access::SAME_SPACE);
    register(&r, b"paid", Handler::Builtin(ok_builtin), |d| d.access = access::COMMERCIAL_SPACE);
    register(&r, b"config-only", Handler::Builtin(ok_builtin), |d| {
        d.access = access::VIEW_AGENT_CONFIG | access::EDIT_AGENT_CONFIG | access::ANONYMOUS_DATA
    });
    let refused = |owner, cmd: &[u8], user, restricted| {
        authorize(owner, cmd, user, restricted).map(|m| m.timeout_s)
    };
    let owner = Some((&r, "h"));
    assert_eq!(refused(None, b"sso", 0, false), Err((500, "No host given for routing this request to.".into())));
    assert_eq!(
        refused(owner, b"nothing", 0, false),
        Err((404, "This feature is not available on this host at this time.".into()))
    );
    assert_eq!(refused(owner, b"__hidden", 0, false), Err((412, "This feature is not available via this API.".into())));
    assert_eq!(refused(owner, b"__hidden", access::SIGNED_ID, false).map_err(|e| e.0), Err(403));
    assert_eq!(refused(owner, b"__hidden", 0, true), Ok(10));
    assert_eq!(
        refused(owner, b"sso", access::VIEW_AGENT_CONFIG, false),
        Err((
            412,
            "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. Sign-in on \
             this dashboard, or access your Netdata via https://app.netdata.cloud."
                .into()
        ))
    );
    assert_eq!(
        refused(owner, b"space", access::SIGNED_ID, false),
        Err((403, "You need to login to the Netdata Cloud space this agent is claimed to, to access this feature.".into()))
    );
    assert!(refused(owner, b"paid", 0, false).unwrap_err().1.starts_with("This feature is only available for commercial"));
    assert_eq!(
        refused(owner, b"config-only", access::ANONYMOUS_DATA, false),
        Err((412, "This feature requires additional permissions: view-config, edit-config.".into()))
    );
    assert_eq!(refused(owner, b"sso", access::SIGNED_ID | access::VIEW_AGENT_CONFIG, false), Ok(10));
}

/// A sync built-in answers into the caller's buffer and leaves the table; a cancelled caller gets 499 and nothing.
#[test]
fn a_sync_builtin_answers_and_leaves() {
    let (calls, _) = calls();
    let r = Registry::default();
    register(&r, b"echo", Handler::Builtin(ok_builtin), |_| {});
    let called = calls.call(spec(&r, b"echo  now"));
    assert_eq!((called.code, called.reply.map(|r| r.body)), (200, Some(b"echo now from test".to_vec())));
    assert!(lock(&calls.table).is_empty());
    let (done, rx) = done_channel();
    let called = calls.call(CallSpec { done: Some(done), is_cancelled: Some(Arc::new(|| true)), ..spec(&r, b"echo") });
    assert_eq!((called.code, called.reply.is_none()), (499, true));
    let (reply, code) = rx.recv().unwrap();
    assert_eq!((code, reply.body), (499, Vec::new()));
}

/// No-wait: the caller's callback gets the answer whenever the transport gives it, then the record leaves; ids are
/// parsed as C's flexible UUIDs and keyed compact and lowercase; a second call with an id in flight is a duplicate.
#[test]
fn a_nowait_call_answers_through_its_callback() {
    let (calls, now) = calls();
    let r = Registry::default();
    let held = Arc::new(Held::default());
    register(&r, b"slow", Handler::Transport(Arc::clone(&held) as Arc<dyn Transport>), |d| d.timeout_s = 7);
    let id = b"0A0B0C0D-0E0F-4011-8213-141516171819";
    let key = "0a0b0c0d0e0f40118213141516171819";
    let (done, rx) = done_channel();
    let called = calls.call(CallSpec { call_id: Some(id), done: Some(done), ..spec(&r, b"slow") });
    assert_eq!(called.code, 200);
    assert_eq!(calls.deadline(key), Some(now.load(Ordering::Relaxed) + 7_000_000));
    let req = lock(&held.requests).pop().unwrap();
    assert_eq!((req.call.key(), req.function.as_slice(), req.source.as_slice()), (key, &b"slow"[..], &b"test"[..]));
    // the same id in flight
    let (dup_done, dup_rx) = done_channel();
    let (dup, records) = netdata_agent_log::capture(|| {
        calls.call(CallSpec { call_id: Some(key.as_bytes()), done: Some(dup_done), ..spec(&r, b"slow") })
    });
    assert_eq!(dup.code, 400);
    assert_eq!(error_text(&dup_rx.recv().unwrap().0), r#"{"status":400,"errorMessage":"Duplicate transaction."}"#);
    let notices: Vec<_> = records.into_iter().filter(|r| r.priority == Priority::Notice).filter_map(|r| r.message).collect();
    assert_eq!(notices, [format!("NRPC: duplicate call_id '{key}', method: 'slow'")]);
    let mut reply = req.reply;
    reply.body = b"done".to_vec();
    (req.done)(reply, 200);
    let (reply, code) = rx.recv().unwrap();
    assert_eq!((code, reply.body.as_slice()), (200, &b"done"[..]));
    assert_eq!(calls.deadline(key), None);
    // an id that is not a UUID gets a random one
    let called = calls.call(CallSpec { call_id: Some(b"tx=4"), ..spec(&r, b"slow") });
    assert_eq!(called.code, 200);
    let req = lock(&held.requests).pop().unwrap();
    assert!(req.call.key().len() == 32 && req.call.key().bytes().all(|c| c.is_ascii_hexdigit()));
    (req.done)(req.reply, 200);
}

/// Wait: the answer comes back with its code, cacheable when it expires; past the deadline and its grace the caller
/// gets 504 with no CANCEL, and the late answer retires the record; a caller that goes away gets 499 and the call is
/// cancelled once.
#[test]
fn a_wait_answers_times_out_or_is_cancelled() {
    let (calls, now) = calls();
    let r = Registry::default();
    let held = Arc::new(Held::default());
    register(&r, b"slow", Handler::Transport(Arc::clone(&held) as Arc<dyn Transport>), |d| d.timeout_s = 1);
    let answer = {
        let held = Arc::clone(&held);
        move |expires: i64| {
            let req = loop {
                if let Some(req) = lock(&held.requests).pop() {
                    break req;
                }
                std::thread::yield_now();
            };
            let mut reply = req.reply;
            (reply.body, reply.expires, reply.content_type) = (b"rows".to_vec(), expires, ContentType::ApplicationJson);
            (req.done)(reply, 202);
        }
    };
    let answering = std::thread::spawn({
        let answer = answer.clone();
        move || answer(5)
    });
    let called = calls.call(CallSpec { wait: true, ..spec(&r, b"slow") });
    answering.join().unwrap();
    let reply = called.reply.unwrap();
    assert_eq!(
        (called.code, reply.body.as_slice(), reply.content_type, reply.cacheable),
        (202, &b"rows"[..], ContentType::ApplicationJson, true)
    );
    assert!(lock(&calls.table).is_empty());

    // the deadline passes: 504, no CANCEL; the late answer retires the record
    let mover = {
        let (now, held) = (Arc::clone(&now), Arc::clone(&held));
        std::thread::spawn(move || {
            while lock(&held.requests).is_empty() {
                std::thread::yield_now();
            }
            now.fetch_add(2_000_001, Ordering::Relaxed);
        })
    };
    let called = calls.call(CallSpec { wait: true, ..spec(&r, b"slow") });
    mover.join().unwrap();
    assert_eq!(called.code, 504);
    assert_eq!(
        error_text(called.reply.as_ref().unwrap()),
        r#"{"status":504,"errorMessage":"Timeout while waiting for a response from the plugin that serves this features"}"#
    );
    assert!(lock(&held.hooked.0).is_empty(), "a timeout sends no CANCEL");
    assert_eq!(lock(&calls.table).len(), 1);
    answer(0);
    assert!(lock(&calls.table).is_empty(), "the late answer retires it");

    // the caller goes away
    let called = calls.call(CallSpec { wait: true, is_cancelled: Some(Arc::new(|| true)), ..spec(&r, b"slow") });
    assert_eq!((called.code, error_text(called.reply.as_ref().unwrap())), (499, r#"{"status":499,"errorMessage":"Request cancelled"}"#.into()));
    let key = lock(&held.requests)[0].call.key().to_string();
    assert_eq!(*lock(&held.hooked.0), [format!("cancel {key}")]);
    answer(0);
    assert!(lock(&calls.table).is_empty());
}

/// Cancel and progress: C's records for an unknown call, a repeated cancel, and a call whose serving thread has
/// finished; a progress pushes the deadline to 10 s from now when that is later.
#[test]
fn cancel_and_progress_as_c() {
    let (calls, now) = calls();
    let r = Arc::new(Registry::default());
    let held = Arc::new(Held::default());
    let (finish, finishing) = mpsc::channel::<()>();
    let (registered, is_registered) = mpsc::channel();
    let server = {
        let (r, held) = (Arc::clone(&r), Arc::clone(&held));
        std::thread::spawn(move || {
            register(&r, b"slow", Handler::Transport(held as Arc<dyn Transport>), |d| d.timeout_s = 1);
            registered.send(()).unwrap();
            finishing.recv().unwrap();
        })
    };
    is_registered.recv().unwrap();
    let debug = |f: &dyn Fn()| -> Vec<String> {
        let ((), records) = netdata_agent_log::capture(f);
        records.into_iter().filter(|r| r.priority == Priority::Debug).filter_map(|r| r.message).collect()
    };
    assert_eq!(
        debug(&|| calls.cancel("ab")),
        ["NRPC: received a CANCEL request for call_id 'ab', but the call_id is not running."]
    );
    assert_eq!(
        debug(&|| calls.progress("ab")),
        ["NRPC: received a PROGRESS request for call_id 'ab', but the call_id is not running."]
    );
    calls.call(spec(&r, b"slow"));
    let key = lock(&held.requests)[0].call.key().to_string();
    let start = now.load(Ordering::Relaxed);
    assert_eq!(debug(&|| calls.progress(&key)), ["Extending function timeout due to PROGRESS update..."]);
    assert_eq!(calls.deadline(&key), Some(start + 10_000_000));
    assert_eq!(debug(&|| calls.progress(&key)), ["Received PROGRESS update..."]);
    calls.cancel(&key);
    assert_eq!(
        debug(&|| calls.cancel(&key)),
        [format!("NRPC: received a CANCEL request for call_id '{key}', but it is already cancelled.")]
    );
    assert_eq!(*lock(&held.hooked.0), [format!("progress {key}"), format!("progress {key}"), format!("cancel {key}")]);
    // the serving thread finishes with a call in flight
    calls.call(spec(&r, b"slow"));
    let second = lock(&held.requests)[1].call.key().to_string();
    finish.send(()).unwrap();
    server.join().unwrap();
    assert_eq!(
        debug(&|| calls.cancel(&second)),
        [format!("NRPC: received a CANCEL request for call_id '{second}', but the serving thread is not running.")]
    );
    assert_eq!(
        debug(&|| calls.progress(&second)),
        [format!("NRPC: received a PROGRESS request for call_id '{second}', but the serving thread is not running.")]
    );
    assert_eq!(lock(&held.hooked.0).len(), 3);
    for req in std::mem::take(&mut *lock(&held.requests)) {
        (req.done)(req.reply, 200);
    }
}

/// A child's method on a parent before milestone 8 commit 7: C's answer of a transport that is gone.
#[test]
fn an_unwired_method_answers_503() {
    let (calls, _) = calls();
    let r = Registry::default();
    register(&r, b"child-fn", Handler::Unwired, |d| d.source = Source::Stream);
    let (done, rx) = done_channel();
    assert_eq!(calls.call(CallSpec { done: Some(done), ..spec(&r, b"child-fn") }).code, 503);
    let (reply, code) = rx.recv().unwrap();
    assert_eq!(
        (code, error_text(&reply)),
        (503, r#"{"status":503,"errorMessage":"The plugin that offered this function is not available."}"#.into())
    );
    assert!(lock(&calls.table).is_empty());
}
