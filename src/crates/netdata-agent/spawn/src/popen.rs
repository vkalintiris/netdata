//! The daemon's one spawn server and the `popen()`-like calls on it (`spawn_popen.c`): every plugin, script and helper
//! the agent runs is started here, with stderr on the collectors' log.

use std::fs::File;
use std::os::fd::AsFd;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::client::{Instance, Server, Start, Waited};
use crate::wire;

/// What every server below is started from.
static START: Mutex<Option<Start>> = Mutex::new(None);
/// `netdata_main_spawn_server`: a failed creation leaves it empty, so the next call tries again.
static MAIN: Mutex<Option<Arc<Server>>> = Mutex::new(None);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Sets what the main server is started from; [`Start::default`] until then.
pub fn configure(start: Start) {
    *lock(&START) = Some(start);
}

/// `netdata_main_spawn_server_init(name, argc, argv)` (`spawn_popen.c:14-23`): creates the main server unless it
/// exists; whether it does.
pub fn main_server_init(name: Option<&str>, title: bool) -> bool {
    let mut main = lock(&MAIN);
    if main.is_none() {
        let start = lock(&START).clone().unwrap_or_default();
        *main = Server::create(name, title, &start).map(Arc::new);
    }
    main.is_some()
}

/// `netdata_main_spawn_server_cleanup()`: stops the main server once no call is using it.
pub fn main_server_cleanup() {
    lock(&MAIN).take();
}

/// The main server's pid, when it runs.
pub fn main_server_pid() -> Option<i32> {
    lock(&MAIN).as_ref().map(|s| s.pid())
}

/// A child started on the main server (`POPEN_INSTANCE`).
#[derive(Debug)]
#[must_use = "a child must be waited for or killed"]
pub struct Popen {
    instance: Instance,
}

impl Popen {
    /// `spawn_popen_run(cmd)`: `/bin/sh -c cmd`; nothing for an empty command.
    pub fn run_shell(cmd: &str) -> Option<Popen> {
        if cmd.is_empty() {
            return None;
        }
        Popen::run_argv(&[b"/bin/sh".as_slice(), b"-c", cmd.as_bytes()])
    }

    /// `spawn_popen_run_argv(argv)` (`spawn_popen.c:60-71`): on the main server, created unnamed if it is not yet; the
    /// child's stderr is the collectors' log, and this process's stdin goes along as the custom descriptor.
    pub fn run_argv<S: AsRef<[u8]>>(argv: &[S]) -> Option<Popen> {
        main_server_init(None, false);
        let server = lock(&MAIN).clone()?;
        let collectors = netdata_agent_log::collectors_fd();
        let stderr = std::io::stderr();
        let stderr = collectors.as_ref().map_or_else(|| stderr.as_fd(), AsFd::as_fd);
        let instance = server.exec(stderr, std::io::stdin().as_fd(), argv)?;
        Some(Popen { instance })
    }

    /// `spawn_popen_pid()`.
    pub fn pid(&self) -> i32 {
        self.instance.pid()
    }

    /// The child's stdin (`spawn_popen_stdin()`).
    pub fn stdin(&mut self) -> Option<&mut File> {
        self.instance.stdin()
    }

    /// The child's stdout (`spawn_popen_stdout()`).
    pub fn stdout(&mut self) -> Option<&mut File> {
        self.instance.stdout()
    }

    /// `spawn_popen_wait()`: closes the pipes and blocks until the child ends; its exit code, 0 when it was killed by
    /// SIGTERM or SIGPIPE, else -1.
    pub fn wait(self) -> i32 {
        wire::status_rc(self.instance.wait())
    }

    /// `spawn_popen_timedwait()`: as [`Popen::wait`] for up to `timeout_ms`.
    pub fn timedwait(self, timeout_ms: i32, cancelled: &dyn Fn() -> bool) -> Waited<Popen> {
        match self.instance.timedwait(timeout_ms, cancelled) {
            Waited::Exited(raw) => Waited::Exited(wire::status_rc(raw)),
            Waited::Running(instance) => Waited::Running(Popen { instance }),
            Waited::Error(instance) => Waited::Error(Popen { instance }),
        }
    }

    /// `spawn_popen_kill()`: stops the child as [`Instance::kill`] does; its code as [`Popen::wait`]'s.
    pub fn kill(self, timeout_ms: i32, cancelled: &dyn Fn() -> bool) -> i32 {
        wire::status_rc(self.instance.kill(timeout_ms, cancelled))
    }
}
