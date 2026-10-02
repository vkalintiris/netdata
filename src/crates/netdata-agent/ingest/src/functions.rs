//! The function calls of a parser (`src/plugins.d/pluginsd_functions.c`, its half of `pluginsd_internals.c`): the
//! transport the methods of a plugin, or of a child streaming to this parent, are registered with. A call is written
//! to the plugin's stdin or the child's socket as `FUNCTION` or `FUNCTION_PAYLOAD`, kept pending until the
//! `FUNCTION_RESULT_BEGIN` ... `FUNCTION_RESULT_END` that answers it, cancelled when its deadline passes (checked when
//! another call comes or a send fails, as C), and answered 503 when the run or the connection ends first.
//!
//! Locking (D147): the pending calls' lock is never held across a write to the plugin or a delivery; the dispatch
//! gate fails new dispatches once the run ends and never blocks one.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use netdata_agent_log::{Priority, Source, nd_log, netdata_log_error};
use netdata_agent_nrpc::call::{Calls, Done, Hooks, ProgressCb, Request, effective_deadline_ut};
use netdata_agent_nrpc::lifetime::Gate;
use netdata_agent_nrpc::reply::{ContentType, Reply};
use netdata_agent_pluginsd_proto::emit;
use netdata_agent_rrd::upstream::Traffic;

/// Where a parser's lines go (`send_to_plugin()`, the parser's `send_to_plugin_cb`): the plugin's stdin, `text`
/// written in one piece under the writer's lock, or the child's connection, `text` queued for its stream thread
/// (`send_to_child()`), as `traffic`. The bytes taken, 0 when there is nowhere to send, or C's negative code after its
/// warning.
pub trait Wire: Send + Sync {
    fn send(&self, text: &[u8], traffic: Traffic) -> isize;
}

/// `struct pluginsd_call`: a call sent to the plugin or child and not answered yet.
struct Pending {
    /// The compact call id (`transaction`).
    key: Arc<str>,
    call_id: [u8; 16],
    /// 503 until the plugin or child answers.
    code: u16,
    reply: Reply,
    done: Option<Done>,
    progress: Option<ProgressCb>,
    gc_collected: bool,
    /// The parser's RESULT span holds it (`parser->defer.item`): its removal waits for the span's end.
    held: bool,
    /// Removed while held: delivered when the span lets it go. C's GC has unlinked it, so only the span reaches it.
    removed: bool,
    /// Its deadline passed during the span and the GC answered 504 in its place: the span's later lines go nowhere
    /// (C appends them to the 504 body, a race, D147.6), but they count: C's buffer length, which its cap reads.
    replaced: Option<usize>,
}

impl Pending {
    /// `pluginsd_calls_delete_cb()`: a 503 without a body says the plugin went away.
    fn deliver(mut self) {
        if self.code == 503 && self.reply.body.is_empty() {
            self.reply.error("The plugin that was servicing this request, exited before responding.", 503);
        }
        if let Some(done) = self.done.take() {
            done(self.reply, self.code);
        }
    }
}

#[derive(Default)]
struct Table {
    /// In the order they came (C's dictionary).
    calls: Vec<Pending>,
    /// `smaller_monotonic_timeout_ut`: the earliest effective deadline, 0 for none.
    smaller_deadline_ut: u64,
}

/// `parser->inflight`: the transport of one plugin run or one child connection.
pub struct PluginsdTransport {
    me: Weak<PluginsdTransport>,
    gate: Gate,
    wire: Arc<dyn Wire>,
    /// Progress requests go down: always to a plugin, to a child only with PROGRESS negotiated.
    progress: bool,
    calls: Arc<Calls>,
    table: Mutex<Table>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl PluginsdTransport {
    /// `pluginsd_calls_init()`: writes to `wire`, reads the deadlines of `calls`; `progress`: whether the other end
    /// takes progress requests.
    pub fn new(wire: Arc<dyn Wire>, calls: Arc<Calls>, progress: bool) -> Arc<Self> {
        Arc::new_cyclic(|me| PluginsdTransport {
            me: me.clone(),
            gate: Gate::default(),
            wire,
            progress,
            calls,
            table: Mutex::new(Table::default()),
        })
    }

    /// The parser's own lines (`send_to_plugin()`), down the same wire as the calls.
    pub(crate) fn send(&self, text: &[u8], traffic: Traffic) -> isize {
        self.wire.send(text, traffic)
    }

    fn hooks(&self) -> Option<Arc<dyn Hooks>> {
        self.me.upgrade().map(|me| me as Arc<dyn Hooks>)
    }

    /// `pluginsd_calls_garbage_collect()`: every call past its deadline and grace is answered 504 (a 2xx or an empty
    /// body is replaced) and cancelled at the plugin; the cancels go out in the calls' order, then the answers in the
    /// reverse order, as C's. A call the parser's span holds is answered at the span's end.
    fn collect(&self, now_ut: u64) {
        let (mut cancels, mut victims) = (Vec::new(), Vec::new());
        {
            let mut t = lock(&self.table);
            t.smaller_deadline_ut = 0;
            let mut i = 0;
            while i < t.calls.len() {
                // a pending call's record outlives it (its answer retires both), so C's record-less skip never runs
                let Some(stop_ut) = self.calls.deadline(&t.calls[i].key) else {
                    i += 1;
                    continue;
                };
                let effective = effective_deadline_ut(stop_ut);
                if effective >= now_ut {
                    if t.smaller_deadline_ut == 0 || effective < t.smaller_deadline_ut {
                        t.smaller_deadline_ut = effective;
                    }
                    i += 1;
                    continue;
                }
                let p = &mut t.calls[i];
                if p.gc_collected {
                    i += 1;
                    continue;
                }
                p.gc_collected = true;
                if p.reply.body.is_empty() || p.code == 200 {
                    p.code = p.reply.error("Timeout waiting for a response.", 504);
                    p.replaced = p.held.then_some(p.reply.body.len());
                }
                cancels.push(Arc::clone(&p.key));
                if p.held {
                    p.removed = true;
                    i += 1;
                } else {
                    victims.push(t.calls.remove(i));
                }
            }
        }
        for key in cancels {
            self.wire.send(emit::function_cancel(&key).as_bytes(), Traffic::Functions);
        }
        for victim in victims.into_iter().rev() {
            victim.deliver();
        }
    }

    /// `FUNCTION_RESULT_BEGIN` of a pending call: its code, content type (none keeps it) and expiry (one that passed
    /// is none: never cacheable); the parser's span holds it until `result_end()`. False for an unknown call.
    pub fn result_begin(&self, key: &[u8], code: u16, format: Option<&[u8]>, expires_s: i64, now_s: i64) -> bool {
        let mut t = lock(&self.table);
        let Some(p) = t.calls.iter_mut().find(|p| p.key.as_bytes() == key) else {
            return false;
        };
        if let Some(format) = format.filter(|f| !f.is_empty()) {
            p.reply.content_type = ContentType::from_name(format);
        }
        p.code = code;
        (p.reply.expires, p.reply.cacheable) = if expires_s <= now_s { (0, false) } else { (expires_s, true) };
        p.held = true;
        true
    }

    /// One line of the answer, newline included: the length of C's buffer after it (C's limit is the parser's).
    pub fn result_line(&self, key: &[u8], line: &[u8]) -> usize {
        let mut t = lock(&self.table);
        let Some(p) = t.calls.iter_mut().find(|p| p.key.as_bytes() == key) else {
            return 0;
        };
        match &mut p.replaced {
            Some(len) => {
                *len += line.len();
                *len
            }
            None => {
                p.reply.body.extend_from_slice(line);
                p.reply.body.len()
            }
        }
    }

    /// `pluginsd_function_result_end()`: the span lets the call go, which is answered.
    pub fn result_end(&self, key: &[u8]) {
        self.let_go(key, |p| p.removed = true);
    }

    /// `pluginsd_calls_release_deferred()`: the run ended inside the call's span; a 2xx becomes 503 (a cut answer
    /// must not report success), the partial body kept, and the call stays pending for the sweep.
    pub fn release_span(&self, key: &[u8]) {
        self.let_go(key, |p| {
            if (200..300).contains(&p.code) {
                p.code = 503;
            }
        });
    }

    fn let_go(&self, key: &[u8], f: impl FnOnce(&mut Pending)) {
        let gone = {
            let mut t = lock(&self.table);
            let Some(i) = t.calls.iter().position(|p| p.key.as_bytes() == key) else {
                return;
            };
            let p = &mut t.calls[i];
            p.held = false;
            f(p);
            p.removed.then(|| t.calls.remove(i))
        };
        if let Some(p) = gone {
            p.deliver();
        }
    }

    /// `FUNCTION_PROGRESS tx done all` from the plugin: the caller's progress callback. False for an unknown call.
    pub fn progress_from_plugin(&self, key: &[u8], done: usize, all: usize) -> bool {
        let found = lock(&self.table)
            .calls
            .iter()
            .find(|p| p.key.as_bytes() == key && !p.removed)
            .map(|p| (p.call_id, p.progress.clone()));
        let Some((call_id, progress)) = found else {
            return false;
        };
        if let Some(progress) = progress {
            progress(&call_id, done, all);
        }
        true
    }

    /// The run's end (`parser_destroy()` after the span's release): no dispatch passes from now on and the ones in
    /// flight are waited for; every call still pending is answered (503, or C's "exited" text without a body).
    pub fn shutdown(&self) {
        self.gate.retire();
        let calls = std::mem::take(&mut lock(&self.table).calls);
        for p in calls {
            p.deliver();
        }
    }
}

impl netdata_agent_nrpc::Transport for PluginsdTransport {
    /// `pluginsd_nrpc_handler()`.
    fn dispatch(&self, req: Request) -> u16 {
        let Some(_pass) = self.gate.try_acquire() else {
            let mut reply = req.reply;
            let code = reply.error("The plugin that offered this function is not available.", 503);
            (req.done)(reply, code);
            return code;
        };
        let now_ut = self.calls.now_ut();
        let stop_ut = req.call.deadline_ut();
        // C's unsigned arithmetic, rounded half up, then an int
        let timeout_s = (stop_ut.wrapping_sub(now_ut).wrapping_add(500_000) / 1_000_000) as i32;
        let key: Arc<str> = Arc::from(req.call.key());
        let function = String::from_utf8_lossy(&req.function).into_owned();
        let source = String::from_utf8_lossy(&req.source);
        let line = match &req.payload {
            Some(payload) if !payload.body.is_empty() => emit::function_payload(
                &key,
                timeout_s,
                &function,
                req.user_access,
                &source,
                payload.content_type.name(),
                &payload.body,
            ),
            _ => emit::function(&key, timeout_s, &function, req.user_access, &source).into_bytes(),
        };
        lock(&self.table).calls.push(Pending {
            key: Arc::clone(&key),
            call_id: req.call_id,
            code: 503,
            reply: req.reply,
            done: Some(req.done),
            progress: req.progress,
            gc_collected: false,
            held: false,
            removed: false,
            replaced: None,
        });
        let sent = self.wire.send(&line, Traffic::Functions);
        if sent < 0 {
            netdata_log_error!("PLUGINSD: FUNCTION '{function}': failed to send it to the plugin, error {sent}");
            let failed = {
                let mut t = lock(&self.table);
                let i = t.calls.iter().position(|p| p.key == key);
                i.map(|i| t.calls.remove(i))
            };
            if let Some(mut p) = failed {
                p.reply.error("Failed to send this request to the plugin that offered it.", 503);
                p.deliver();
            }
            self.collect(now_ut);
            return 503;
        }
        if let Some(hooks) = self.hooks() {
            // a plugin's progress pings always go to it, a child's only with PROGRESS (pluginsd_functions.c:469-475)
            req.call.set_cancel_hook(Arc::clone(&hooks));
            if self.progress {
                req.call.set_progress_hook(hooks);
            }
        }
        let collect = {
            let mut t = lock(&self.table);
            let effective = effective_deadline_ut(stop_ut);
            if t.smaller_deadline_ut == 0 || effective < t.smaller_deadline_ut {
                t.smaller_deadline_ut = effective;
            }
            t.smaller_deadline_ut < now_ut
        };
        if collect {
            self.collect(now_ut);
        }
        200
    }
}

impl Hooks for PluginsdTransport {
    /// `pluginsd_function_cancel_to_plugin()`.
    fn cancel(&self, key: &str) {
        let Some(_pass) = self.gate.try_acquire() else {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "PLUGINSD: FUNCTION_CANCEL for transaction '{key}', but the plugin is not running."
            );
            return;
        };
        let pending = !key.is_empty() && lock(&self.table).calls.iter().any(|p| &*p.key == key && !p.removed);
        if pending {
            self.wire.send(emit::function_cancel(key).as_bytes(), Traffic::Functions);
        } else {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "PLUGINSD: FUNCTION_CANCEL request didn't match any pending function requests in pluginsd.d."
            );
        }
    }

    /// `pluginsd_function_progress_to_plugin()`.
    fn progress(&self, key: &str) {
        if key.is_empty() {
            nd_log!(Source::Daemon, Priority::Err, "PLUGINSD: FUNCTION_PROGRESS request without transaction!");
            return;
        }
        let Some(_pass) = self.gate.try_acquire() else {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "PLUGINSD: FUNCTION_PROGRESS for transaction '{key}', but the plugin is not running."
            );
            return;
        };
        if !lock(&self.table).calls.iter().any(|p| &*p.key == key && !p.removed) {
            nd_log!(
                Source::Daemon,
                Priority::Debug,
                "PLUGINSD: FUNCTION_PROGRESS request for transaction '{key}' that is not in progress!"
            );
            return;
        }
        let line = emit::function_progress(key);
        if self.wire.send(line.as_bytes(), Traffic::Functions) != line.len() as isize {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "PLUGINSD: FUNCTION_PROGRESS request failed to send to plugin for transaction '{key}'"
            );
        }
    }
}

#[cfg(test)]
mod tests;
