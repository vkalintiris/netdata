//! The calls (`src/nrpc/nrpc-calls.c`): authorization, the process-wide table of calls in flight keyed by their
//! compact call id, the three modes (sync, no-wait, wait), cancel, progress and deadlines.
//!
//! Locking: the table's lock is never held across a handler, a hook or a result; a hook is reached through the
//! method's serving gate, which a finished thread closes without waiting on its callers.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use netdata_agent_log::{Priority, Source as LogSource, nd_log};
use netdata_agent_text::parse::uuid_parse_flexi;
use netdata_agent_text::print::print_uuid_lower_compact;
use netdata_agent_text::sanitize::nrpc_sanitize_name;

use crate::reply::{Payload, Reply};
use crate::{FLAG_RESTRICTED, Handler, Method, NAME_MAX, Registry, access, sanitize_command};

/// `NRPC_DEADLINE_GRACE_UT`: a deadline is enforced a second after it passes.
pub const DEADLINE_GRACE_UT: u64 = 1_000_000;
/// `FUNCTIONS_EXTENDED_TIME_ON_PROGRESS_UT`.
pub const EXTENDED_ON_PROGRESS_UT: u64 = 10_000_000;
/// The waiter's poll (`netdata_cond_timedwait()` of 10 ms).
const WAIT_POLL: Duration = Duration::from_millis(10);

/// `nrpc_result_cb_t`: the call's answer and code, exactly once.
pub type Done = Box<dyn FnOnce(Reply, u16) + Send>;
/// `nrpc_progress_cb_t`: the call id, done and all.
pub type ProgressCb = Arc<dyn Fn(&[u8; 16], usize, usize) + Send + Sync>;
/// `nrpc_is_cancelled_cb_t`.
pub type IsCancelled = Arc<dyn Fn() -> bool + Send + Sync>;

/// `nrpc_effective_deadline_ut()`.
pub fn effective_deadline_ut(stop_ut: u64) -> u64 {
    stop_ut.wrapping_add(DEADLINE_GRACE_UT)
}

/// What a transport registers so a call reaches it (`nrpc_cancel_hook_cb_t`, `nrpc_progress_hook_cb_t`), by the
/// call's compact id.
pub trait Hooks: Send + Sync {
    fn cancel(&self, key: &str);
    fn progress(&self, key: &str);
}

/// The monotonic clock of the deadlines, which tests move.
pub trait Clock: Send + Sync {
    fn monotonic_ut(&self) -> u64;
}

/// `now_monotonic_usec()`.
#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn monotonic_ut(&self) -> u64 {
        netdata_agent_sys::now_monotonic_usec()
    }
}

/// A call's cancel and progress hooks.
#[derive(Default)]
struct HookSlots {
    cancel: Option<Arc<dyn Hooks>>,
    progress: Option<Arc<dyn Hooks>>,
}

/// `struct nrpc_call`: a call in flight.
pub struct Record {
    /// The compact lowercase call id (`call->call_id`).
    key: Arc<str>,
    method: Arc<Method>,
    cancelled: AtomicBool,
    stop_ut: AtomicU64,
    /// A sync call's cancellation is its caller's; an async one's is this record's flag.
    caller_is_cancelled: Option<IsCancelled>,
    asynchronous: bool,
    hooks: Mutex<HookSlots>,
}

impl Record {
    pub fn key(&self) -> &str {
        &self.key
    }

    /// `nrpc_call_is_cancelled()` for an async call, the caller's check for a sync one.
    pub fn is_cancelled(&self) -> bool {
        if self.asynchronous {
            self.cancelled.load(Ordering::Relaxed)
        } else {
            self.caller_is_cancelled.as_ref().is_some_and(|f| f())
        }
    }

    /// The deadline (`*req->stop_monotonic_ut`).
    pub fn deadline_ut(&self) -> u64 {
        self.stop_ut.load(Ordering::Relaxed)
    }

    /// `register_cancel_hook`: an async call's only (a sync call offers none).
    pub fn set_cancel_hook(&self, hooks: Arc<dyn Hooks>) {
        if self.asynchronous {
            lock(&self.hooks).cancel = Some(hooks);
        }
    }

    /// `register_progress_hook`.
    pub fn set_progress_hook(&self, hooks: Arc<dyn Hooks>) {
        if self.asynchronous {
            lock(&self.hooks).progress = Some(hooks);
        }
    }
}

/// `struct nrpc_request`: what a handler gets.
pub struct Request {
    pub call_id: [u8; 16],
    /// The sanitized command.
    pub function: Vec<u8>,
    pub payload: Option<Payload>,
    /// The caller's sanitized provenance.
    pub source: Vec<u8>,
    pub user_access: u32,
    /// Where the answer is written.
    pub reply: Reply,
    /// Hands the answer back; MUST be called exactly once.
    pub done: Done,
    /// The caller's progress callback.
    pub progress: Option<ProgressCb>,
    /// The call in flight: its id, cancellation, deadline and hooks.
    pub call: Arc<Record>,
}

/// `struct nrpc_call_spec`: a caller's call.
pub struct CallSpec<'a> {
    /// The host's registry and name; none is "no host given".
    pub owner: Option<(&'a Registry, &'a str)>,
    pub cmd: &'a [u8],
    pub source: &'a [u8],
    pub user_access: u32,
    /// `<= 0` is the method's.
    pub timeout_s: i32,
    /// An async method: block until its answer.
    pub wait: bool,
    pub allow_restricted: bool,
    /// None or unparsable is a random one.
    pub call_id: Option<&'a [u8]>,
    pub payload: Option<Payload>,
    /// The caller's buffer.
    pub reply: Reply,
    pub done: Option<Done>,
    pub progress: Option<ProgressCb>,
    pub is_cancelled: Option<IsCancelled>,
}

/// What `Calls::call()` returned: the code, and the answer when no `done` took it (a wait, a sync call or a refusal
/// without one).
pub struct Called {
    pub code: u16,
    pub reply: Option<Reply>,
}

/// `struct nrpc_inflight_calls`.
pub struct Calls {
    table: Mutex<HashMap<Arc<str>, Arc<Record>>>,
    clock: Box<dyn Clock>,
}

static PROCESS: OnceLock<Arc<Calls>> = OnceLock::new();

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `nrpc_method_authorize()`'s checks on a sanitized command: the method, or the code and text C answers.
pub fn authorize(
    owner: Option<(&Registry, &str)>,
    command: &[u8],
    user_access: u32,
    allow_restricted: bool,
) -> Result<Arc<Method>, (u16, String)> {
    let Some((registry, host)) = owner else {
        return Err((500, "No host given for routing this request to.".into()));
    };
    let method = registry.find(host, command).map_err(|(code, text)| (code, text.to_string()))?;
    let denied = access::denied_code(user_access);
    if method.flags & FLAG_RESTRICTED != 0 && !allow_restricted {
        return Err((denied, "This feature is not available via this API.".into()));
    }
    if user_access & method.access != method.access {
        let lacks = |bit: u32| method.access & bit != 0 && user_access & bit == 0;
        let text = if lacks(access::SIGNED_ID) {
            "You need to be authenticated via Netdata Cloud Single-Sign-On (SSO) to access this feature. Sign-in on \
             this dashboard, or access your Netdata via https://app.netdata.cloud."
                .to_string()
        } else if lacks(access::SAME_SPACE) {
            "You need to login to the Netdata Cloud space this agent is claimed to, to access this feature.".to_string()
        } else if lacks(access::COMMERCIAL_SPACE) {
            "This feature is only available for commercial users and supporters of Netdata. To use it, please upgrade \
             your space. Thank you for supporting Netdata."
                .to_string()
        } else {
            let missing: Vec<_> = access::names(!user_access & method.access).collect();
            format!("This feature requires additional permissions: {}.", missing.join(", "))
        };
        return Err((denied, text));
    }
    Ok(method)
}

impl Calls {
    pub fn new(clock: Box<dyn Clock>) -> Arc<Calls> {
        Arc::new(Calls { table: Mutex::new(HashMap::new()), clock })
    }

    /// `nrpc_inflight_calls_create()`: the process's table, made once at startup (or by its first user).
    pub fn process() -> &'static Arc<Calls> {
        PROCESS.get_or_init(|| Calls::new(Box::new(SystemClock)))
    }

    /// The table's clock.
    pub fn now_ut(&self) -> u64 {
        self.clock.monotonic_ut()
    }

    fn get(&self, key: &str) -> Option<Arc<Record>> {
        lock(&self.table).get(key).cloned()
    }

    fn remove(&self, key: &str) {
        let record = lock(&self.table).remove(key);
        drop(record);
    }

    /// `nrpc_call_deadline()`: a call's deadline while it is in flight.
    pub fn deadline(&self, key: &str) -> Option<u64> {
        self.get(key).map(|r| r.deadline_ut())
    }

    /// `nrpc_call()`.
    pub fn call(self: &Arc<Self>, spec: CallSpec) -> Called {
        let CallSpec { owner, cmd, source, user_access, timeout_s, wait, allow_restricted, call_id, payload, .. } = spec;
        let (mut reply, done, progress, is_cancelled) = (spec.reply, spec.done, spec.progress, spec.is_cancelled);
        let source = nrpc_sanitize_name(&source[..source.len().min(NAME_MAX)], source.len().min(NAME_MAX) + 1);
        let function = sanitize_command(cmd);
        let refuse = |mut reply: Reply, code: u16, text: &str, done: Option<Done>| {
            reply.error(text, code);
            match done {
                Some(done) => {
                    done(reply, code);
                    Called { code, reply: None }
                }
                None => Called { code, reply: Some(reply) },
            }
        };
        let method = match authorize(owner, &function, user_access, allow_restricted) {
            Ok(method) => method,
            Err((code, text)) => return refuse(reply, code, &text, done),
        };
        let timeout_s = if timeout_s <= 0 { method.timeout_s } else { timeout_s };
        let id = call_id.and_then(uuid_parse_flexi).unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());
        let mut compact = Vec::with_capacity(32);
        print_uuid_lower_compact(&mut compact, &id);
        let key: Arc<str> = Arc::from(String::from_utf8_lossy(&compact).as_ref());
        // C multiplies an int by an unsigned microsecond count: a negative timeout wraps
        let stop_ut = self.now_ut().wrapping_add((i64::from(timeout_s) * 1_000_000) as u64);
        let record = Arc::new(Record {
            key: Arc::clone(&key),
            method: Arc::clone(&method),
            cancelled: AtomicBool::new(false),
            stop_ut: AtomicU64::new(stop_ut),
            caller_is_cancelled: is_cancelled.clone(),
            asynchronous: !method.sync,
            hooks: Mutex::default(),
        });
        {
            let mut table = lock(&self.table);
            if table.contains_key(&key) {
                drop(table);
                nd_log!(
                    LogSource::Daemon,
                    Priority::Notice,
                    "NRPC: duplicate call_id '{key}', method: '{}'",
                    String::from_utf8_lossy(&function)
                );
                return refuse(reply, 400, "Duplicate transaction.", done);
            }
            table.insert(Arc::clone(&key), Arc::clone(&record));
        }
        reply.body.clear();
        let request = |reply: Reply, done: Done| Request {
            call_id: id,
            function: function.clone(),
            payload: payload.clone(),
            source: source.clone(),
            user_access,
            reply,
            done,
            progress: progress.clone(),
            call: Arc::clone(&record),
        };
        if method.sync {
            // the caller's buffer and result callback, its cancellation; the record leaves after the handler
            let kept: Arc<Mutex<Option<Reply>>> = Arc::new(Mutex::new(None));
            let sync_done: Done = match done {
                Some(done) => done,
                None => {
                    let kept = Arc::clone(&kept);
                    Box::new(move |reply, _| *lock(&kept) = Some(reply))
                }
            };
            let code = dispatch(&method, request(reply, sync_done));
            self.remove(&key);
            return Called { code, reply: lock(&kept).take() };
        }
        if !wait {
            // nrpc_call_nowait_finished(): the caller's callback, then the record leaves
            let calls = Arc::clone(self);
            let finished: Done = Box::new(move |reply, code| {
                if let Some(done) = done {
                    done(reply, code);
                }
                calls.remove(&key);
            });
            let code = dispatch(&method, request(reply, finished));
            return Called { code, reply: None };
        }
        self.wait(&method, &record, reply, is_cancelled, request)
    }

    /// `nrpc_call_async_wait()`: a temporary buffer, polled every 10 ms until its answer, the deadline (re-read each
    /// pass, a progress extends it) or the caller's cancellation; a late answer then retires the record.
    fn wait(
        self: &Arc<Self>,
        method: &Arc<Method>,
        record: &Arc<Record>,
        reply: Reply,
        is_cancelled: Option<IsCancelled>,
        request: impl Fn(Reply, Done) -> Request,
    ) -> Called {
        struct Waiting {
            answer: Option<(Reply, u16)>,
            gave_up: bool,
        }
        let state = Arc::new((Mutex::new(Waiting { answer: None, gave_up: false }), Condvar::new()));
        let signal: Done = {
            let (state, calls, key) = (Arc::clone(&state), Arc::clone(self), Arc::clone(&record.key));
            Box::new(move |reply, code| {
                let gave_up = {
                    let mut w = lock(&state.0);
                    w.answer = Some((reply, code));
                    state.1.notify_one();
                    w.gave_up
                };
                if gave_up {
                    calls.remove(&key);
                }
            })
        };
        let content_type = reply.content_type;
        let mut caller = reply;
        let code = dispatch(method, request(Reply::new(content_type), signal));
        let mut w = lock(&state.0);
        if code != 200 && w.answer.is_none() {
            w.gave_up = true;
            return Called { code, reply: Some(caller) };
        }
        let mut cancelled = false;
        while w.answer.is_none() {
            if self.now_ut() > effective_deadline_ut(record.deadline_ut()) {
                break;
            }
            w = state.1.wait_timeout(w, WAIT_POLL).unwrap_or_else(std::sync::PoisonError::into_inner).0;
            if w.answer.is_none() && is_cancelled.as_ref().is_some_and(|f| f()) {
                cancelled = true;
                drop(w);
                cancel_record(record);
                w = lock(&state.0);
                break;
            }
        }
        // C holds the wait's mutex from the cancel to this check, so no answer beats its 499: one that came meanwhile
        // is dropped, and its record removed here, as its signal saw the waiter still waiting
        if cancelled && w.answer.take().is_some() {
            drop(w);
            self.remove(&record.key);
            return Called { code: caller.error("Request cancelled", 499), reply: Some(caller) };
        }
        if let Some((answer, code)) = w.answer.take() {
            drop(w);
            caller.body = answer.body;
            caller.content_type = answer.content_type;
            caller.expires = answer.expires;
            caller.cacheable = caller.expires != 0;
            self.remove(&record.key);
            return Called { code, reply: Some(caller) };
        }
        w.gave_up = true;
        drop(w);
        let code = if cancelled {
            caller.error("Request cancelled", 499)
        } else {
            caller.error("Timeout while waiting for a response from the plugin that serves this features", 504)
        };
        Called { code, reply: Some(caller) }
    }

    /// `nrpc_call_cancel()`.
    pub fn cancel(&self, key: &str) {
        match self.get(key) {
            Some(record) => cancel_record(&record),
            None => nd_log!(
                LogSource::Daemon,
                Priority::Debug,
                "NRPC: received a CANCEL request for call_id '{key}', but the call_id is not running."
            ),
        }
    }

    /// `nrpc_call_progress()`: the deadline extended to 10 s from now when that is later, then the transport told.
    pub fn progress(&self, key: &str) {
        let Some(record) = self.get(key) else {
            nd_log!(
                LogSource::Daemon,
                Priority::Debug,
                "NRPC: received a PROGRESS request for call_id '{key}', but the call_id is not running."
            );
            return;
        };
        let Some(_pass) = record.method.serving.try_dispatch() else {
            nd_log!(
                LogSource::Daemon,
                Priority::Debug,
                "NRPC: received a PROGRESS request for call_id '{key}', but the serving thread is not running."
            );
            return;
        };
        let now = self.now_ut();
        if now + EXTENDED_ON_PROGRESS_UT > record.deadline_ut() {
            nd_log!(LogSource::Daemon, Priority::Debug, "Extending function timeout due to PROGRESS update...");
            record.stop_ut.store(now + EXTENDED_ON_PROGRESS_UT, Ordering::Relaxed);
        } else {
            nd_log!(LogSource::Daemon, Priority::Debug, "Received PROGRESS update...");
        }
        let hook = lock(&record.hooks).progress.clone();
        if let Some(hook) = hook {
            hook.progress(key);
        }
    }

    /// `nrpc_call_request_progress()`.
    pub fn request_progress(&self, call_id: &[u8; 16]) {
        if *call_id == [0; 16] {
            return;
        }
        let mut compact = Vec::with_capacity(32);
        print_uuid_lower_compact(&mut compact, call_id);
        self.progress(&String::from_utf8_lossy(&compact));
    }
}

/// `nrpc_call_cancel_internal()`: the first cancel wins; the transport is told through the method's serving gate.
fn cancel_record(record: &Record) {
    if record.cancelled.swap(true, Ordering::Relaxed) {
        nd_log!(
            LogSource::Daemon,
            Priority::Debug,
            "NRPC: received a CANCEL request for call_id '{}', but it is already cancelled.",
            record.key
        );
        return;
    }
    let Some(_pass) = record.method.serving.try_dispatch() else {
        nd_log!(
            LogSource::Daemon,
            Priority::Debug,
            "NRPC: received a CANCEL request for call_id '{}', but the serving thread is not running.",
            record.key
        );
        return;
    };
    let hook = lock(&record.hooks).cancel.clone();
    if let Some(hook) = hook {
        hook.cancel(&record.key);
    }
}

/// The method's handler on a request.
fn dispatch(method: &Method, mut req: Request) -> u16 {
    match &method.handler {
        Handler::Transport(transport) => transport.dispatch(req),
        Handler::Builtin(builtin) => {
            // nrpc_builtin_handler(): a cancelled call answers 499 with an empty body
            let mut code = if req.call.is_cancelled() {
                499
            } else {
                builtin(&mut req.reply, &req.function, req.payload.as_ref(), &req.source)
            };
            if code == 499 || req.call.is_cancelled() {
                req.reply.body.clear();
                code = 499;
            }
            (req.done)(req.reply, code);
            code
        }
        Handler::Unwired => {
            let code = req.reply.error("The plugin that offered this function is not available.", 503);
            (req.done)(req.reply, code);
            code
        }
    }
}

#[cfg(test)]
mod tests;
