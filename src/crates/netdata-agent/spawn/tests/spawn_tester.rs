//! C's `spawn-tester` (`src/libnetdata/spawn_server/spawn-tester.c`) ported, and the cases it does not cover: the
//! server's process as `/proc` shows it, its records, the kill ladder, the daemon's and the server's deaths, the lazy
//! unnamed server and the environment. `harness = false`: this binary is the driver, its own spawn server (re-executed
//! with the marker) and its own children (the `plugin-*` roles, as C's tester runs itself).

use std::ffi::OsString;
use std::fs::File;
use std::io::{BufRead, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use netdata_agent_log::{Captured, Priority, capture};
use netdata_agent_spawn::client::{Instance, Server, Start, Waited};
use netdata_agent_spawn::popen::{self, Popen};
use netdata_agent_spawn::{env, server, wire};
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;

const ENV_KEY: &str = "SPAWN_TESTER";
const ENV_VALUE: &str = "1234567890";
/// Set in the driver: a binary that finds it is a child that took the wrong path, never a second driver.
const DRIVER: &str = "NETDATA_SPAWN_TESTER_DRIVER";
/// Every child of this run carries it, so the end of the run can find strays.
const RUN_TAG: &str = "NETDATA_SPAWN_TESTER_RUN";
const ECHO_AND_EXIT_MSG: &[u8] = b"GOODBYE\n";
const HELLO: &[u8] = b"Hello World!\n";
const NEVER: &dyn Fn() -> bool = &|| false;

fn main() {
    server::run_if_requested();
    let args: Vec<String> = std::env::args().collect();
    if let Some(code) = args.get(1).and_then(|role| run_role(role, &args[2..])) {
        std::process::exit(code);
    }
    if std::env::var_os(DRIVER).is_some() {
        std::process::exit(2);
    }
    driver();
}

// ------------------------------------------------------------------------------------------------------------------
// the children

/// `child_check_fds()`: 0-2 open, nothing else (the listing's own descriptor is closed before the check).
fn child_check_fds() {
    for fd in 0..3 {
        assert!(std::fs::read_link(format!("/proc/self/fd/{fd}")).is_ok(), "fd No {fd} should be a valid file descriptor");
    }
    let listed: Vec<i32> = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect();
    for fd in listed.into_iter().filter(|&fd| fd >= 3) {
        if let Ok(target) = std::fs::read_link(format!("/proc/self/fd/{fd}")) {
            eprintln!("fd No {fd} ({}) is a valid file descriptor - it shouldn't.", target.display());
            std::process::exit(1);
        }
    }
}

/// `child_check_environment()`.
fn child_check_environment() {
    let value = std::env::var(ENV_KEY).unwrap_or_default();
    if value != ENV_VALUE {
        eprintln!("Wrong environment. Variable '{ENV_KEY}' should have value '{ENV_VALUE}' but it has '{value}'");
        std::process::exit(1);
    }
}

/// The `plugin-*` roles of C's tester, and the parent-death helper.
fn run_role(role: &str, rest: &[String]) -> Option<i32> {
    let echo = || {
        child_check_fds();
        child_check_environment();
        let mut out = std::io::stdout();
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    };
    match role {
        "plugin-kill-to-stop" => {
            echo();
            Some(0)
        }
        "plugin-close-to-stop" => {
            echo();
            eprintln!("child detected a closed pipe.");
            Some(1)
        }
        "plugin-echo-and-exit" => {
            child_check_fds();
            child_check_environment();
            let _ = std::io::stdout().write_all(ECHO_AND_EXIT_MSG);
            Some(0)
        }
        "plugin-sleep-to-stop" => {
            child_check_fds();
            child_check_environment();
            // C sleeps an hour; a stray must not outlive a failed run by that long
            std::thread::sleep(Duration::from_secs(120));
            Some(0)
        }
        "helper-parent-death" => Some(helper_parent_death(rest)),
        _ => None,
    }
}

/// Plays the daemon that dies: a server with one child (`rest[0]`, a shell command), its pids and the socket path on
/// stdout, then a wait to be killed.
fn helper_parent_death(rest: &[String]) -> i32 {
    let Some(server) = Server::create(Some("death"), false, &Start::default()) else {
        return 1;
    };
    let stderr = std::io::stderr();
    let Some(child) = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sh", "-c", &rest[0]]) else {
        return 1;
    };
    println!("{} {} {}", server.pid(), child.pid(), server.path().display());
    let _ = std::io::stdout().flush();
    std::thread::sleep(Duration::from_secs(120));
    drop(child);
    0
}

// ------------------------------------------------------------------------------------------------------------------
// the driver's tools

struct Run {
    exe: String,
    run_dir: PathBuf,
    tag: String,
}

fn step(name: &str) {
    eprintln!("spawn-tester: {name}");
}

/// Reads until a newline (or the end): what `fgets()` returns.
fn read_line(from: &mut File) -> Vec<u8> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    while line.last() != Some(&b'\n') {
        match from.read(&mut byte) {
            Ok(1) => line.push(byte[0]),
            _ => break,
        }
    }
    line
}

/// `test_int_fds_echo_loop()`: each line written comes back.
fn echo_loop(child: &mut Instance, iterations: usize) {
    for _ in 0..iterations {
        child.stdin().unwrap().write_all(HELLO).unwrap();
        assert_eq!(read_line(child.stdout().unwrap()), HELLO);
    }
}

fn popen_echo_loop(child: &mut Popen, iterations: usize) {
    for _ in 0..iterations {
        let stdin = child.stdin().unwrap();
        stdin.write_all(HELLO).unwrap();
        stdin.flush().unwrap();
        assert_eq!(read_line(child.stdout().unwrap()), HELLO);
    }
}

/// A server whose stderr, and so its records (all of them), goes to `log`.
fn server_logging_to(name: &str, log: &Path) -> Server {
    let file = File::create(log).unwrap();
    let saved = std::io::stderr().as_fd().try_clone_to_owned().unwrap();
    nix::unistd::dup2_stderr(&file).unwrap();
    env::set("NETDATA_LOG_LEVEL", "debug").unwrap();
    let server = Server::create(Some(name), false, &Start::default());
    env::set("NETDATA_LOG_LEVEL", "err").unwrap();
    nix::unistd::dup2_stderr(&saved).unwrap();
    server.unwrap_or_else(|| panic!("cannot create the {name} server"))
}

/// The server's records (logfmt lines) holding `text` at `level`.
fn logged(log: &Path, level: &str, text: &str) -> bool {
    std::fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .any(|l| l.contains(&format!("level={level}")) && l.contains(text))
}

/// Waits up to `timeout` for `ok`.
fn eventually(timeout: Duration, ok: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    ok()
}

/// Waits until `pid` runs `comm` (a shell that set its traps and exec'd it).
fn runs(pid: i32, comm: &str) -> bool {
    eventually(Duration::from_secs(2), || {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim_end() == comm)
    })
}

/// A process reaped: no `/proc` entry left.
fn gone(pid: i32) -> bool {
    !Path::new(&format!("/proc/{pid}")).exists()
}

/// Waits until `pid` sleeps in `poll()`: a signal then interrupts the wait it is meant to.
fn in_poll(pid: i32) -> bool {
    eventually(Duration::from_secs(2), || {
        std::fs::read_to_string(format!("/proc/{pid}/wchan")).is_ok_and(|w| w.contains("poll"))
    })
}

/// A process that exists and is not a zombie.
fn alive(pid: i32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .is_ok_and(|s| s.rsplit_once(") ").is_some_and(|(_, rest)| !rest.starts_with('Z')))
}

/// Whether `pid` is a process this run started: its environment carries the run's tag (every child), or it is a child
/// of the driver running this binary (a server, a helper).
fn is_stray(pid: i32, tag: &str) -> bool {
    let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap_or_default();
    let tagged = environ.split(|&c| c == 0).any(|e| e == format!("{RUN_TAG}={tag}").as_bytes());
    let me = std::process::id().to_string();
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let child = stat.rsplit_once(") ").is_some_and(|(_, rest)| rest.split(' ').nth(1) == Some(me.as_str()));
    let ours = child && std::fs::read_link(format!("/proc/{pid}/exe")).ok() == std::env::current_exe().ok();
    tagged || ours
}

/// SIGKILL to `pid` if it is still one this run started.
fn kill_stray(pid: i32, tag: &str) {
    if is_stray(pid, tag) {
        let _ = kill(Pid::from_raw(pid), Signal::SIGKILL);
    }
}

/// The processes this run left, killed by their verified pids; `(pid, stat)` of each.
fn kill_strays(tag: &str) -> Vec<String> {
    let me = std::process::id() as i32;
    let mut strays = Vec::new();
    for entry in std::fs::read_dir("/proc").unwrap().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else { continue };
        if pid != me && is_stray(pid, tag) {
            let stat = std::fs::read_to_string(entry.path().join("stat")).unwrap_or_default();
            strays.push(format!("{pid}: {}", stat.trim()));
            kill_stray(pid, tag);
        }
    }
    strays
}

/// A failed case panics past the end-of-run check: its strays are killed on the way out, or their copies of the
/// driver's stderr would keep `cargo test` waiting (R56 I2).
struct StrayGuard(String);

impl Drop for StrayGuard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let strays = kill_strays(&self.0);
            if !strays.is_empty() {
                eprintln!("spawn-tester: killed after the failure: {strays:#?}");
            }
        }
    }
}

fn messages(records: &[Captured]) -> Vec<(Priority, i32, String)> {
    records.iter().map(|r| (r.priority, r.errno, r.message.clone().unwrap_or_default())).collect()
}

/// A `/proc/<pid>/status` mask as a number.
fn status_mask(pid: &str, key: &str) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let line = status.lines().find(|l| l.starts_with(key)).unwrap();
    u64::from_str_radix(line[key.len()..].trim(), 16).unwrap()
}

fn bit(signal: Signal) -> u64 {
    1 << (signal as i32 - 1)
}

// ------------------------------------------------------------------------------------------------------------------
// the cases

/// `test_listener_backlog()`: idle clients queue on a server blocked in its first read, up to `SOMAXCONN`.
fn backlog() {
    step("listener backlog");
    let server = Server::create(None, true, &Start::default()).unwrap();
    let kernel = std::fs::read_to_string("/proc/sys/net/core/somaxconn").ok().and_then(|s| s.trim().parse().ok());
    let connections = kernel.unwrap_or(64usize).min(64).min(libc::SOMAXCONN as usize);
    assert!(connections > 5, "a listener cap of {connections} cannot tell the historical backlog of 5 apart");
    let addr = nix::sys::socket::UnixAddr::new(server.path()).unwrap();
    let mut clients = Vec::new();
    for i in 0..connections {
        use nix::sys::socket::{AddressFamily, SockFlag, SockType, connect, socket};
        let flags = SockFlag::SOCK_NONBLOCK | SockFlag::SOCK_CLOEXEC;
        let sock = socket(AddressFamily::Unix, SockType::Stream, flags, None).unwrap();
        match connect(sock.as_raw_fd(), &addr) {
            Ok(()) | Err(nix::errno::Errno::EINPROGRESS | nix::errno::Errno::EALREADY) => clients.push(sock),
            Err(e) => panic!("the listener rejected backlog test connection {}/{connections}: {e}", i + 1),
        }
    }
    // SA_RESTART restarts its blocked read on SIGTERM: kill it first so destroying it cannot hang (as C's tester)
    let _ = kill(Pid::from_raw(server.pid()), Signal::SIGKILL);
    drop(clients);
    server.destroy();
}

/// The server's process as `/proc` shows it, against C's.
fn server_process(server: &Server) {
    step("the server's process");
    let pid = server.pid().to_string();
    assert_eq!(std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap(), "spawn-test\n");
    let mut want = Vec::new();
    for (i, arg) in std::env::args_os().enumerate() {
        let len = arg.as_bytes().len();
        let mut a: Vec<u8> = if i == 0 { b"spawn-test".iter().copied().take(len).collect() } else { vec![] };
        a.resize(len, b' ');
        want.extend_from_slice(&a);
        want.push(0);
    }
    assert_eq!(std::fs::read(format!("/proc/{pid}/cmdline")).unwrap(), want);
    let mut fds: Vec<(i32, String)> = std::fs::read_dir(format!("/proc/{pid}/fd"))
        .unwrap()
        .filter_map(|e| {
            let e = e.ok()?;
            let target = std::fs::read_link(e.path()).ok()?;
            Some((e.file_name().to_str()?.parse().ok()?, target.to_string_lossy().into_owned()))
        })
        .collect();
    fds.sort();
    let our_stderr = std::fs::read_link("/proc/self/fd/2").unwrap().to_string_lossy().into_owned();
    let kinds: Vec<&str> = fds[3..].iter().map(|(_, t)| t.split(':').next().unwrap()).collect();
    assert_eq!(
        (&fds[..3], kinds),
        (
            &[(0, "/dev/null".into()), (1, "/dev/null".into()), (2, our_stderr)][..],
            vec!["socket", "pipe"]
        ),
        "{fds:?}"
    );
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    assert!(status.lines().any(|l| l == "Threads:\t1"), "{status}");
    assert_eq!(status_mask(&pid, "SigBlk:"), 0xffff_fffe_7ffa_beff);
    let caught = status_mask(&pid, "SigCgt:");
    assert_eq!(caught & (bit(Signal::SIGTERM) | bit(Signal::SIGCHLD)), bit(Signal::SIGTERM) | bit(Signal::SIGCHLD));
}

/// The fds tests of C's tester on a named server.
fn fds(run: &Run, server: &Server) {
    let stderr = std::io::stderr();
    let exec = |role: &str| server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &[run.exe.as_str(), role]).unwrap();
    for i in 1..=5 {
        step(&format!("fds No {i} (kill to stop)"));
        let mut child = exec("plugin-kill-to-stop");
        echo_loop(&mut child, 30);
        let code = child.kill(0, NEVER);
        assert!(code == 0 || code == libc::SIGTERM, "child exited with code {code}");
    }
    for i in 1..=5 {
        step(&format!("fds No {i} (echo and exit)"));
        let mut child = exec("plugin-echo-and-exit");
        assert_eq!(read_line(child.stdout().unwrap()), ECHO_AND_EXIT_MSG);
        assert_eq!(read_line(child.stdout().unwrap()), b"");
        assert_eq!(child.wait(), 0);
    }
    for i in 1..=5 {
        step(&format!("fds No {i} (close to stop)"));
        let mut child = exec("plugin-close-to-stop");
        echo_loop(&mut child, 1);
        assert_eq!(child.wait(), 1 << 8);
    }
}

/// `test_exec_child_dies_when_stdout_closes()`: SIGPIPE is the child's again. A Rust role would ignore it (std does
/// before `main`), so the child is the shell's `yes`.
fn sigpipe() {
    step("termination contract (closing stdout must kill the child)");
    let server = Server::create(Some("test-sigpipe"), true, &Start::default()).unwrap();
    let stderr = std::io::stderr();
    let mut child = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sh", "-c", "exec yes"]).unwrap();
    let mut raw = None;
    for _ in 0..50 {
        match child.timedwait(100, NEVER) {
            Waited::Exited(status) => {
                raw = Some(status);
                break;
            }
            Waited::Running(c, _) => child = c,
            Waited::Error(_) => panic!("timedwait() returned ERROR"),
        }
    }
    assert_eq!(raw, Some(libc::SIGPIPE), "the child did not die of SIGPIPE when its stdout closed");
    let _ = kill(Pid::from_raw(server.pid()), Signal::SIGKILL);
    server.destroy();
}

/// The popen tests of C's tester, on the main server.
fn popen_tests(run: &Run) {
    let shell = |role: &str| Popen::run_shell(&format!("exec '{}' {role}", run.exe)).unwrap();
    for i in 1..=5 {
        step(&format!("popen No {i} (kill to stop)"));
        let mut child = shell("plugin-kill-to-stop");
        popen_echo_loop(&mut child, 30);
        assert_eq!(child.kill(0, NEVER), 0);
    }
    for i in 1..=5 {
        step(&format!("popen No {i} (echo and exit)"));
        let mut child = shell("plugin-echo-and-exit");
        assert_eq!(read_line(child.stdout().unwrap()), ECHO_AND_EXIT_MSG);
        assert_eq!(read_line(child.stdout().unwrap()), b"");
        assert_eq!(child.wait(), 0);
    }
    for i in 1..=5 {
        step(&format!("popen No {i} (close to stop)"));
        let mut child = shell("plugin-close-to-stop");
        popen_echo_loop(&mut child, 1);
        assert_eq!(child.wait(), 1);
    }
    for i in 1..=5 {
        step(&format!("popen No {i} (timedwait exits)"));
        let mut child = shell("plugin-echo-and-exit");
        let mut slices = 0;
        let code = loop {
            match child.timedwait(100, NEVER) {
                Waited::Exited(code) => break code,
                Waited::Running(c, _) => child = c,
                Waited::Error(_) => panic!("timedwait() returned ERROR for a child that should exit cleanly"),
            }
            slices += 1;
            assert!(slices <= 100, "timedwait() did not reap a child that exits immediately");
        };
        assert_eq!(code, 0);
    }
    for i in 1..=5 {
        step(&format!("popen No {i} (timedwait kill)"));
        let mut child = shell("plugin-sleep-to-stop");
        for _ in 0..5 {
            match child.timedwait(200, NEVER) {
                Waited::Running(c, _) => child = c,
                _ => panic!("timedwait() did not report RUNNING for a sleeping child"),
            }
        }
        assert_eq!(child.kill(0, NEVER), 0);
    }
}

/// The first popen of a process creates an unnamed server (`P:60-71`), then the named one replaces it.
fn lazy_unnamed(run: &Run) {
    step("lazy unnamed main server");
    let mut child = Popen::run_shell("echo lazy").unwrap();
    assert_eq!(read_line(child.stdout().unwrap()), b"lazy\n");
    assert_eq!(child.wait(), 0);
    let pid = popen::main_server_pid().unwrap();
    assert_eq!(std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap(), "spawn-unnamed\n");
    // unnamed keeps the arguments as they are
    let mut want = Vec::new();
    for arg in std::env::args_os() {
        want.extend_from_slice(arg.as_bytes());
        want.push(0);
    }
    assert_eq!(std::fs::read(format!("/proc/{pid}/cmdline")).unwrap(), want);
    let prefix = format!("netdata-spawn-{}-", std::process::id());
    let sockets = || -> Vec<OsString> {
        std::fs::read_dir(&run.run_dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.file_name()))
            .filter(|n| n.to_string_lossy().starts_with(&prefix) && n.to_string_lossy().ends_with(".sock"))
            .collect()
    };
    assert_eq!(sockets().len(), 1);
    popen::main_server_cleanup();
    assert!(sockets().is_empty());
    assert!(!alive(pid));
    assert!(Popen::run_shell("").is_none());
}

/// A child's view: the contract's mask and SIGPIPE, the rest of the server's state as it was at creation.
fn child_view() {
    step("a child's process");
    let server = Server::create(Some("view"), false, &Start::default()).unwrap();
    let cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir("/").unwrap();
    let mask = nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));
    let stderr = std::io::stderr();
    let script = "grep -E '^Sig(Blk|Ign):' /proc/self/status; pwd; umask; cut -d' ' -f5,6 /proc/$$/stat";
    let mut child = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sh", "-c", script]).unwrap();
    let mut out = String::new();
    child.stdout().unwrap().read_to_string(&mut out).unwrap();
    assert_eq!(child.wait(), 0);
    nix::sys::stat::umask(mask);
    std::env::set_current_dir(&cwd).unwrap();
    let ignored = status_mask("self", "SigIgn:") & !bit(Signal::SIGPIPE);
    let pgid = nix::unistd::getpgrp();
    let sid = nix::unistd::getsid(None).unwrap();
    let want = format!(
        "SigBlk:\t0000000000000000\nSigIgn:\t{ignored:016x}\n{}\n{:04o}\n{pgid} {sid}\n",
        cwd.display(),
        mask.bits()
    );
    let got: Vec<String> = out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    let want: Vec<String> = want.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect();
    assert_eq!(got, want);
    // empty arguments do not reach the child (`argv_encode()`)
    let mut child = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/echo", "", "x"]).unwrap();
    assert_eq!(read_line(child.stdout().unwrap()), b"x\n");
    assert_eq!(child.wait(), 0);
    server.destroy();
}

/// The server's records of its children, and of a child it cannot start.
fn records(run: &Run) {
    step("the server's records");
    let log = run.run_dir.join("records.log");
    let server = server_logging_to("records", &log);
    let stderr = std::io::stderr();
    let exec = |argv: &[&str]| server.exec(stderr.as_fd(), std::io::stdin().as_fd(), argv);
    let child = exec(&["/bin/false"]).unwrap();
    let pid = child.pid();
    assert_eq!(child.wait(), 1 << 8);
    let child = exec(&["/bin/sleep", "120"]).unwrap();
    let term_pid = child.pid();
    assert_eq!(child.kill(0, NEVER), libc::SIGTERM);
    let child = exec(&["/bin/sleep", "120"]).unwrap();
    let kill_pid = child.pid();
    kill(Pid::from_raw(kill_pid), Signal::SIGKILL).unwrap();
    assert_eq!(child.wait(), libc::SIGKILL);
    // a real-time signal's death is reaped and answered as any other (R56 B1: nix cannot decode it)
    let child = exec(&["/bin/sleep", "120"]).unwrap();
    let rt_pid = child.pid();
    assert!(Command::new("kill").args(["-34", &rt_pid.to_string()]).status().unwrap().success());
    // bounded: an unanswered request must fail the case, not hang it
    match child.timedwait(5000, NEVER) {
        Waited::Exited(raw) => assert_eq!(raw, 34),
        _ => panic!("the real-time signal's death was never answered"),
    }
    let (none, client) = capture(|| exec(&["/nonexistent/x"]).map(|c| c.pid()));
    assert_eq!(none, None);
    assert_eq!(
        messages(&client),
        [(Priority::Err, 0, "SPAWN PARENT: Failed to exec spawn request 5 (server reports failure, errno is updated)".into())]
    );
    for (level, text) in [
        ("debug", format!("SPAWN SERVER: process created with pid {pid}: /bin/false")),
        ("warning", format!("SPAWN SERVER: child with pid {pid} (request 1) exited with exit code 1: /bin/false")),
        ("debug", format!("SPAWN SERVER: child with pid {term_pid} (request 2) killed by signal 15: /bin/sleep 120")),
        ("warning", format!("SPAWN SERVER: child with pid {kill_pid} (request 3) killed by signal 9: /bin/sleep 120")),
        ("warning", format!("SPAWN SERVER: child with pid {rt_pid} (request 4) killed by signal 34: /bin/sleep 120")),
        ("err", "SPAWN SERVER: posix_spawn() failed: /nonexistent/x".to_string()),
    ] {
        assert!(logged(&log, level, &text), "{level} {text}:\n{}", std::fs::read_to_string(&log).unwrap());
    }
    server.destroy();
}

/// `spawn_server_exec_kill()`'s ladder: SIGTERM, 2 s, SIGKILL; a cancelled wait does not wait; a child already
/// reaped is not there to signal.
fn ladder() {
    step("the kill ladder");
    let server = Server::create(Some("ladder"), false, &Start::default()).unwrap();
    let stderr = std::io::stderr();
    let exec = |script: &str| server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sh", "-c", script]).unwrap();
    let deaf = "trap '' TERM; exec /bin/sleep 120";
    let child = exec(deaf);
    assert!(runs(child.pid(), "sleep"));
    let started = Instant::now();
    assert_eq!(child.kill(0, NEVER), libc::SIGKILL);
    let took = started.elapsed();
    assert!(took >= Duration::from_millis(1900) && took < Duration::from_millis(3000), "{took:?}");

    let child = exec(deaf);
    let (pid, request) = (child.pid(), 2);
    assert!(runs(pid, "sleep"));
    let started = Instant::now();
    let (code, records) = capture(|| child.kill(0, &|| true));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(code, -1);
    let cmdline = wire::cmdline(&["/bin/sh", "-c", deaf]);
    assert_eq!(
        messages(&records),
        [(
            Priority::Err,
            libc::ECANCELED,
            format!(
                "SPAWN PARENT: giving up waiting for pid {pid} after SIGKILL (request No {request}) - reclaiming, the \
                 child is left running: {cmdline}"
            )
        )]
    );
    assert!(eventually(Duration::from_secs(2), || !alive(pid)));

    let child = exec("exit 0");
    let pid = child.pid();
    // reaped (on SIGCHLD, or at the server's 500 ms poll timeout); its report waits on the socket
    assert!(eventually(Duration::from_secs(2), || gone(pid)));
    let (code, records) = capture(|| child.kill(0, NEVER));
    assert_eq!(code, 0);
    let cmdline = wire::cmdline(&["/bin/sh", "-c", "exit 0"]);
    assert_eq!(
        messages(&records),
        [(
            Priority::Debug,
            libc::ESRCH,
            format!("SPAWN PARENT: cannot send signal 15 to child pid {pid} (request 3): No such process: {cmdline}")
        )]
    );

    // a child that ends within the caller's grace is not signalled
    let child = exec("sleep 0.3; exit 7");
    assert_eq!(child.kill(2000, NEVER), 7 << 8);

    // an instance dropped unwaited sends its child SIGTERM (`spawn_server_exec_destroy()`)
    let child = exec("exec /bin/sleep 120");
    let pid = child.pid();
    assert!(runs(pid, "sleep"));
    drop(child);
    assert!(eventually(Duration::from_secs(2), || !alive(pid)), "the dropped instance's child lives");

    // a zero timeout is a minimal bounded wait, never "forever" (`spawn_server.h`); it leaves ETIMEDOUT for the
    // caller's record
    let child = exec("exec /bin/sleep 120");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(match child.timedwait(0, NEVER) {
            Waited::Running(c, errno) => Some((c, errno)),
            _ => None,
        });
    });
    let waited = rx.recv_timeout(Duration::from_secs(2)).expect("timedwait(0) did not return");
    let (running, errno) = waited.expect("not running");
    assert_eq!(errno, libc::ETIMEDOUT);
    // a cancelled wait returns at its next look, with ECANCELED
    let started = Instant::now();
    let (running, errno) = match running.timedwait(5000, &|| true) {
        Waited::Running(c, errno) => (c, errno),
        _ => panic!("a cancelled timedwait() did not report RUNNING"),
    };
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(errno, libc::ECANCELED);
    assert_eq!(running.kill(0, NEVER), libc::SIGTERM);
    server.destroy();
}

/// The server's own deaths: SIGTERM stops its children first; SIGKILL orphans them and its socket goes dead.
fn server_deaths(run: &Run) {
    step("the server's deaths");
    let log = run.run_dir.join("term.log");
    let server = server_logging_to("term", &log);
    let stderr = std::io::stderr();
    let child = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sleep", "120"]).unwrap();
    // a SIGTERM met outside poll() ends the loop without the record, in C too
    assert!(in_poll(server.pid()));
    kill(Pid::from_raw(server.pid()), Signal::SIGTERM).unwrap();
    assert_eq!(child.wait(), libc::SIGTERM);
    assert!(eventually(Duration::from_secs(2), || !server.path().exists()));
    assert!(logged(&log, "err", "SPAWN SERVER: poll() failed"), "{}", std::fs::read_to_string(&log).unwrap());
    server.destroy();

    let server = Server::create(Some("kill"), false, &Start::default()).unwrap();
    let child = server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/sleep", "120"]).unwrap();
    let (pid, request) = (child.pid(), 1);
    kill(Pid::from_raw(server.pid()), Signal::SIGKILL).unwrap();
    let (code, records) = capture(|| child.wait());
    assert_eq!(code, -1);
    assert_eq!(
        messages(&records),
        [(Priority::Err, 0, format!("SPAWN PARENT: failed to read final status report for child {pid}, request {request}"))]
    );
    assert!(alive(pid), "the orphan should outlive its server");
    kill_stray(pid, &run.tag);
    // its listener goes with it: the request socket can be released first
    assert!(eventually(Duration::from_secs(2), || !alive(server.pid())));
    let (none, records) = capture(|| server.exec(stderr.as_fd(), std::io::stdin().as_fd(), &["/bin/true"]).is_none());
    assert!(none);
    assert_eq!(
        messages(&records),
        [(
            Priority::Err,
            libc::ECONNREFUSED,
            format!("SPAWN PARENT: cannot connect() to spawn server on path '{}': Connection refused (111).", server.path().display())
        )]
    );
    server.destroy();
}

/// A second server on a live one's path fails, and its cleanup unlinks the live one's socket (C's).
fn already_listening() {
    step("already listening");
    let first = Server::create(Some("twice"), false, &Start::default()).unwrap();
    let (second, records) = capture(|| Server::create(Some("twice"), false, &Start::default()).is_none());
    assert!(second);
    assert_eq!(
        messages(&records),
        [(Priority::Err, 0, format!("SPAWN SERVER: Server is already listening on path '{}'", first.path().display()))]
    );
    assert!(!first.path().exists());
    first.destroy();
}

/// The daemon dies: the server stops its children (SIGTERM, then SIGKILL after 2 s for one that ignores it), unlinks
/// its socket and exits.
fn parent_death(run: &Run) {
    let ready = run.run_dir.join("death-ready");
    let termed = run.run_dir.join("death-term");
    let polite = format!(
        "trap 'echo TERM > {}; exit 0' TERM; touch {}; while :; do sleep 0.05; done",
        termed.display(),
        ready.display()
    );
    for (name, script, deaf) in [
        ("parent death", polite.as_str(), false),
        ("parent death, a child that ignores SIGTERM", "trap '' TERM; exec /bin/sleep 120", true),
    ] {
        step(name);
        let mut helper = Command::new(&run.exe)
            .args(["helper-parent-death", script])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(helper.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let fields: Vec<&str> = line.split_whitespace().collect();
        let (server, child, path) = (fields[0].parse::<i32>().unwrap(), fields[1].parse::<i32>().unwrap(), fields[2]);
        // the shell's trap is set
        assert!(if deaf { runs(child, "sleep") } else { eventually(Duration::from_secs(2), || ready.exists()) });
        helper.kill().unwrap();
        helper.wait().unwrap();
        let killed = Instant::now();
        assert!(eventually(Duration::from_secs(1), || !Path::new(path).exists()), "{name}: the socket stays");
        if deaf {
            std::thread::sleep(Duration::from_millis(1500));
            assert!(alive(child), "{name}: SIGKILL came before the grace");
        }
        assert!(eventually(Duration::from_secs(3), || !alive(child)), "{name}: the child outlived its server");
        if deaf {
            assert!(killed.elapsed() < Duration::from_millis(2600), "{name}: {:?}", killed.elapsed());
        } else {
            assert_eq!(std::fs::read_to_string(&termed).unwrap(), "TERM\n", "{name}");
        }
        assert!(eventually(Duration::from_secs(3), || !alive(server)), "{name}: the server stays");
    }
}

/// The environment children get: frozen, then changed only in the snapshot.
fn environment() {
    step("environment");
    env::freeze();
    env::set("SPAWN_TESTER_LATE", "late").unwrap();
    let mut child = Popen::run_shell("echo $SPAWN_TESTER_LATE").unwrap();
    assert_eq!(read_line(child.stdout().unwrap()), b"late\n");
    assert_eq!(child.wait(), 0);
    assert!(std::env::var_os("SPAWN_TESTER_LATE").is_none());
}

/// Nothing this run started is left: no process carrying its tag, no child of the driver.
fn no_strays(run: &Run) {
    step("no strays");
    let strays = kill_strays(&run.tag);
    assert!(strays.is_empty(), "strays: {strays:#?}");
}

fn driver() {
    let run_dir = tempfile::Builder::new().prefix("spawn-tester-").tempdir().unwrap();
    let run = Run {
        exe: std::env::current_exe().unwrap().to_string_lossy().into_owned(),
        run_dir: run_dir.path().to_path_buf(),
        tag: format!("{}-{}", std::process::id(), std::time::UNIX_EPOCH.elapsed().unwrap().as_nanos()),
    };
    let _guard = StrayGuard(run.tag.clone());
    for (key, value) in [
        (ENV_KEY, ENV_VALUE),
        (DRIVER, "1"),
        (RUN_TAG, run.tag.as_str()),
        ("NETDATA_RUN_DIR", run.run_dir.to_str().unwrap()),
        ("NETDATA_LOG_LEVEL", "err"),
        ("NETDATA_LOG_METHOD", "stderr"),
        ("NETDATA_LOG_FORMAT", "logfmt"),
    ] {
        env::set(key, value).unwrap();
    }
    let started = Instant::now();

    lazy_unnamed(&run);
    backlog();
    step("fds");
    // inheritable, as a launcher's descriptor: the server closes it at its start
    let inheritable = nix::unistd::pipe().unwrap();
    let server = Server::create(Some("test"), true, &Start::default()).unwrap();
    server_process(&server);
    drop(inheritable);
    fds(&run, &server);
    server.destroy();
    sigpipe();
    child_view();
    records(&run);
    ladder();
    server_deaths(&run);
    already_listening();
    parent_death(&run);
    step("popen");
    assert!(popen::main_server_init(Some("test"), true));
    popen_tests(&run);
    environment();
    popen::main_server_cleanup();
    no_strays(&run);
    eprintln!("spawn-tester: passed in {:?}", started.elapsed());
}
