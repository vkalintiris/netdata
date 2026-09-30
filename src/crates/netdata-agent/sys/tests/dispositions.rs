//! `default_dispositions()` in a single-threaded process (decision D123; the test harness runs tests on threads of its
//! own): started with SIGHUP and SIGCHLD ignored, as `nohup` or a supervisor leaves them, the process blocks them and
//! resets them; they are no longer ignored, and a SIGHUP raised before the reset is still pending after it.

use std::process::Command;

use nix::sys::signal::{SigSet, Signal};

const CHILD: &str = "NETDATA_SYS_DISPOSITIONS_CHILD";

/// A mask line of `/proc/self/status` (`SigIgn`, `SigPnd`).
fn mask(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find_map(|l| l.strip_prefix(field)).unwrap();
    u64::from_str_radix(line.trim_start_matches(':').trim(), 16).unwrap()
}

fn bit(signal: Signal) -> u64 {
    1 << (signal as i32 - 1)
}

fn child() {
    let both = bit(Signal::SIGHUP) | bit(Signal::SIGCHLD);
    assert_eq!(mask("SigIgn") & both, both, "the launcher left them ignored");
    let mut set = SigSet::empty();
    set.add(Signal::SIGHUP);
    set.add(Signal::SIGCHLD);
    set.thread_block().unwrap();
    // a signal that is not blocked is refused: its default action could end the process
    let refused = netdata_agent_sys::default_dispositions(&[Signal::SIGTERM]).unwrap_err();
    assert!(refused.to_string().contains("SIGTERM blocked"), "{refused}");
    // blocked, an ignored signal is still queued
    nix::sys::signal::raise(Signal::SIGHUP).unwrap();
    assert_ne!(mask("SigPnd") & bit(Signal::SIGHUP), 0);
    netdata_agent_sys::default_dispositions(&[Signal::SIGHUP, Signal::SIGCHLD]).unwrap();
    assert_eq!(mask("SigIgn") & both, 0);
    assert_ne!(mask("SigPnd") & bit(Signal::SIGHUP), 0, "the pending SIGHUP survives");
}

fn main() {
    if std::env::var_os(CHILD).is_some() {
        child();
        return;
    }
    // `env --ignore-signal` is GNU coreutils 8.31 or later
    if !Command::new("env").args(["--ignore-signal=HUP", "true"]).status().is_ok_and(|s| s.success()) {
        eprintln!("dispositions: skipped, `env --ignore-signal` is not supported here (GNU coreutils 8.31+)");
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let out = Command::new("env")
        .arg("--ignore-signal=HUP,CHLD")
        .arg(&exe)
        .env(CHILD, "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
