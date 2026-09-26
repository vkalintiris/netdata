//! The exit sequence of `netdata_cleanup_and_exit()` (`src/daemon/daemon-shutdown.c`) and its watcher
//! (`daemon-shutdown-watcher.c`): the main thread runs C's 22 steps, doing the Rust agent's work in the matching ones,
//! and the `EXIT_WATCHER` thread logs each step as it starts and finishes.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use netdata_agent_log::{
    Field, Priority, Source, Value, msgid, nd_log, netdata_log_error, netdata_log_info, push,
};
use netdata_agent_text::duration::duration_to_string;

use crate::startup::now_ut;

/// `watcher_steps[].msg`, in `watcher_step_id_t` order.
pub const STEPS: [&str; 22] = [
    "close webrtc connections",
    "disable maintenance, new queries, new web requests, new streaming connections and aclk",
    "stop maintenance thread",
    "stop exporters, health and web servers threads",
    "stop websocket threads",
    "stop collectors and streaming threads",
    "stop replication threads",
    "disable ML detection and training threads",
    "stop context thread",
    "clear web client cache",
    "stop ACLK sync thread",
    "stop ACLK MQTT connection thread",
    "stop all remaining worker threads",
    "cancel main threads",
    "stop collection for all hosts",
    "wait for dbengine collectors to finish",
    "stop dbengine tiers",
    "stop metasync threads",
    "join static threads",
    "close SQL databases",
    "remove pid file",
    "free openssl structures",
];

/// The steps the Rust agent has work in (indices into [`STEPS`]).
pub const STOP_WEB_SERVERS: usize = 3;
pub const STOP_STREAMING: usize = 5;
pub const STOP_CONTEXT: usize = 8;
pub const CANCEL_MAIN_THREADS: usize = 13;
pub const STOP_COLLECTION: usize = 14;
pub const STOP_METASYNC_THREADS: usize = 17;
pub const JOIN_STATIC_THREADS: usize = 18;
pub const CLOSE_SQL_DATABASES: usize = 19;
pub const REMOVE_PID_FILE: usize = 20;

/// systemd allows 150 s; the watcher gives up at 135 s since the shutdown started.
const TIMEOUT_S: u64 = 135;

/// How long C's service waits give the threads of each step (`service_wait_exit()` in steps 4, 6 and 9).
pub const WEB_SERVERS_WAIT: Duration = Duration::from_secs(3);
pub const STREAMING_WAIT: Duration = Duration::from_secs(20);
pub const CONTEXT_WAIT: Duration = Duration::from_secs(5);

/// `exit_initiated`: set when the exit sequence starts.
static EXITING: AtomicBool = AtomicBool::new(false);

/// The `-P` pidfile, which step 21 removes on every exit, a fatal one included.
static PIDFILE: OnceLock<String> = OnceLock::new();

/// What the daemon's threads and databases need at each step of an exit (the step, and whether the exit is normal),
/// set once startup completed: an exit can start on the main thread (a signal), on `DAEMON_COMMAND` (`netdatacli
/// shutdown-agent`) or on any thread (a fatal error).
type Work = Box<dyn FnMut(usize, bool) + Send>;

static WORK: Mutex<Option<Work>> = Mutex::new(None);

pub fn set_work(work: Work) {
    *WORK.lock().unwrap_or_else(PoisonError::into_inner) = Some(work);
}

/// `netdata_exit_gracefully()`: the exit sequence with the daemon's work, taken by the first exit.
pub fn exit_gracefully(reason: &str) {
    let work = WORK.lock().unwrap_or_else(PoisonError::into_inner).take();
    let mut work = work.unwrap_or_else(|| Box::new(|_, _| {}));
    cleanup_and_exit(reason, true, &mut *work);
}

/// `netdata_exit_fatal()`, registered as `fatal()`'s final callback: the exit sequence as an abnormal exit.
pub fn exit_fatal() {
    let work = WORK.lock().unwrap_or_else(PoisonError::into_inner).take();
    let mut work = work.unwrap_or_else(|| Box::new(|_, _| {}));
    cleanup_and_exit("fatal", false, &mut *work);
}

/// `exit_initiated_get()`.
pub fn exiting() -> bool {
    EXITING.load(Ordering::Acquire)
}

pub fn set_pidfile(path: &str) {
    if !path.is_empty() {
        let _ = PIDFILE.set(path.to_string());
    }
}

#[derive(Default)]
struct State {
    begun: bool,
    done: [bool; STEPS.len()],
    ended: bool,
}

type Shared = Arc<(Mutex<State>, Condvar)>;

fn lock(shared: &Shared) -> MutexGuard<'_, State> {
    shared.0.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `duration_snprintf(value, "us", true)`.
fn duration(us: u64) -> String {
    duration_to_string(i64::try_from(us).unwrap_or(i64::MAX), "us", true).unwrap_or_default()
}

/// The `EXIT_WATCHER` thread.
struct Watcher {
    shared: Shared,
    thread: JoinHandle<()>,
}

impl Watcher {
    /// `watcher_thread_start()`.
    fn start() -> std::io::Result<Watcher> {
        let shared: Shared = Arc::default();
        let theirs = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("EXIT_WATCHER".into())
            .spawn(move || {
                netdata_agent_log::thread_created();
                watch(&theirs);
                netdata_agent_log::thread_finished();
            })?;
        Ok(Watcher { shared, thread })
    }

    fn update(&self, f: impl FnOnce(&mut State)) {
        f(&mut lock(&self.shared));
        self.shared.1.notify_all();
    }
}

/// `watcher_main()`: waits for the shutdown to begin, then for each step in order, within the time left.
fn watch(shared: &Shared) {
    let wake = &shared.1;
    drop(
        wake.wait_while(lock(shared), |s| !s.begun)
            .unwrap_or_else(PoisonError::into_inner),
    );
    netdata_log_info!("Shutdown process started");
    let shutdown_start = now_ut();
    for (step, msg) in STEPS.iter().enumerate() {
        let step_start = now_ut();
        let since_start = step_start.saturating_sub(shutdown_start);
        let at = duration(since_start);
        let n = step + 1;
        let total = STEPS.len();
        netdata_log_info!("shutdown step: [{n}/{total}] - {{at {at}}} started '{msg}'...");
        let remaining = Duration::from_secs(TIMEOUT_S.saturating_sub(since_start / 1_000_000));
        let (state, _) = wake
            .wait_timeout_while(lock(shared), remaining, |s| !s.done[step])
            .unwrap_or_else(PoisonError::into_inner);
        let ok = state.done[step];
        drop(state);
        let took = duration(now_ut().saturating_sub(step_start));
        if ok {
            netdata_log_info!(
                "shutdown step: [{n}/{total}] - {{at {at}}} finished '{msg}' in {took}"
            );
        } else {
            netdata_log_error!(
                "shutdown step: [{n}/{total}] - {{at {at}}} timeout '{msg}' takes too long ({took}) - giving up..."
            );
            std::process::abort();
        }
    }
    drop(
        wake.wait_while(lock(shared), |s| !s.ended)
            .unwrap_or_else(PoisonError::into_inner),
    );
    netdata_log_info!(
        "Shutdown process ended in {}",
        duration(now_ut().saturating_sub(shutdown_start))
    );
}

/// `netdata_cleanup_and_exit()`: the shutdown record, then every step under the watcher. `work(step)` does what the
/// caller has for a step (its threads); the steps every exit shares (`cancel_main_threads()`, the pidfile) are done
/// here. `reason` is the exit reason's name; a normal one is logged as a notice, anything else as critical. A second
/// exit, such as a fatal on another thread while exiting, ends the process at once.
pub fn cleanup_and_exit(reason: &str, normal: bool, mut work: impl FnMut(usize, bool)) {
    if EXITING.swap(true, Ordering::AcqRel) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "EXIT: Recursion detected. Exiting immediately."
        );
        std::process::exit(1);
    }
    netdata_agent_log::limits_unlimited();
    {
        // netdata_log_exit_reason()
        let _frame = push(vec![(Field::MessageId, Value::Uuid(msgid::EXIT))]);
        let priority = if normal {
            Priority::Notice
        } else {
            Priority::Crit
        };
        nd_log!(
            Source::Daemon,
            priority,
            "NETDATA SHUTDOWN: initializing shutdown with code due to: {reason}"
        );
    }
    // C does not check the watcher's creation either: without it the steps still run, unlogged.
    let watcher = Watcher::start().ok();
    if let Some(w) = &watcher {
        w.update(|s| s.begun = true);
    }
    for step in 0..STEPS.len() {
        work(step, normal);
        match step {
            // cancel_main_threads(): no static thread of C's table runs in the Rust agent
            CANCEL_MAIN_THREADS => {
                netdata_log_info!("All threads finished.");
                // an abnormal exit leaves the command server alone: a command may hold what the fatal thread waits for
                if normal {
                    crate::command_server::exit();
                }
            }
            REMOVE_PID_FILE => {
                if let Some(pidfile) = PIDFILE.get()
                    && let Err(err) = std::fs::remove_file(pidfile)
                {
                    nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_log::errno_of(&err);
                        "EXIT: cannot unlink pidfile '{pidfile}'.");
                }
                crate::command_server::remove_socket_file();
            }
            _ => {}
        }
        if let Some(w) = &watcher {
            w.update(|s| s.done[step] = true);
        }
    }
    if let Some(w) = watcher {
        w.update(|s| s.ended = true);
        let _ = w.thread.join();
    }
}
