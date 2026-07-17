//! External command execution with a hard per-command timeout.
//!
//! On unix the child gets its own process group and a timeout SIGKILLs the
//! whole group — a stronger guarantee than the shell scripts' portable
//! watchdog, which could only kill the direct child. On Windows the direct
//! child is terminated.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub struct CmdOutput {
    /// stdout followed by stderr, capped at the requested byte budget.
    pub output: Vec<u8>,
    /// "0", "137", "signal 9", "124 (timeout)", or "?" when unknown.
    pub exit_desc: String,
    pub duration_secs: u64,
    /// true when the per-command timeout killed the child; the output may
    /// end mid-line or mid-record
    pub timed_out: bool,
}

fn reader_thread<R: Read + Send + 'static>(mut src: R, cap: usize) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut collected = Vec::new();
        let mut buf = [0u8; 65536];
        loop {
            match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    // keep draining past the cap so the child never blocks on
                    // a full pipe; bytes past the cap are discarded
                    if collected.len() < cap {
                        let take = n.min(cap - collected.len());
                        collected.extend_from_slice(&buf[..take]);
                    }
                }
            }
        }
        let _ = tx.send(collected);
    });
    rx
}

fn tail_reader_thread<R: Read + Send + 'static>(
    mut src: R,
    tail_bytes: usize,
) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut collected: Vec<u8> = Vec::new();
        let mut trimmed = false;
        let mut buf = [0u8; 65536];
        loop {
            match src.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    collected.extend_from_slice(&buf[..n]);
                    // keep only the most recent window (amortized trim)
                    if collected.len() > tail_bytes * 2 {
                        collected.drain(..collected.len() - tail_bytes);
                        trimmed = true;
                    }
                }
            }
        }
        if collected.len() > tail_bytes {
            collected.drain(..collected.len() - tail_bytes);
            trimmed = true;
        }
        if trimmed {
            // the front cut can split a line; a secret value orphaned from
            // its key would dodge the key-based sanitizer, so start at the
            // next line boundary (drop everything if none exists)
            match collected.iter().position(|&b| b == b'\n') {
                Some(nl) => {
                    collected.drain(..=nl);
                }
                None => collected.clear(),
            }
        }
        let _ = tx.send(collected);
    });
    rx
}

/// Which end of the output the byte budget keeps.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum CaptureMode {
    /// Keep the FIRST `cap_bytes` of stdout+stderr (stdout-then-stderr).
    Head,
    /// Keep the LAST `cap_bytes` of stdout — journal captures want the most
    /// recent history — and discard stderr. The tail restarts at a line
    /// boundary after a trim so a secret value cannot be orphaned from its
    /// key.
    #[cfg_attr(windows, allow(dead_code))] // only journal collectors tail
    Tail,
}

/// Run `argv` with the given timeout, capturing output up to `cap_bytes`
/// per `mode`. Never inherits the terminal; never blocks past timeout + a
/// small grace period.
pub fn run_capped(
    argv: &[&str],
    timeout: Duration,
    cap_bytes: usize,
    mode: CaptureMode,
) -> CmdOutput {
    let started = Instant::now();
    // a declared item with an empty argv is a programmer error (debug_assert
    // at declaration); in release it degrades like any failed spawn instead
    // of an index panic aborting the whole run
    let Some(program) = argv.first() else {
        return CmdOutput {
            output: b"(failed to run: empty argv)\n".to_vec(),
            exit_desc: "?".to_string(),
            duration_secs: 0,
            timed_out: false,
        };
    };
    let mut cmd = Command::new(program);
    cmd.args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // own process group, so a timeout can kill grandchildren too
        cmd.process_group(0);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CmdOutput {
                output: format!("(failed to run {}: {e})\n", argv[0]).into_bytes(),
                exit_desc: "?".to_string(),
                duration_secs: 0,
                timed_out: false,
            };
        }
    };
    let tail_mode = mode == CaptureMode::Tail;
    let out_rx = child.stdout.take().map(|s| {
        if tail_mode {
            tail_reader_thread(s, cap_bytes)
        } else {
            reader_thread(s, cap_bytes)
        }
    });
    let err_rx = child.stderr.take().map(|s| {
        // tail mode discards stderr (journal noise would pollute the tail)
        if tail_mode {
            reader_thread(s, 0)
        } else {
            reader_thread(s, cap_bytes)
        }
    });

    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() >= timeout || crate::util::interrupted() {
            timed_out = started.elapsed() >= timeout;
            kill_child(&mut child);
            break child.wait().ok();
        }
        std::thread::sleep(Duration::from_millis(50));
    };

    let mut output = Vec::new();
    for rx in [out_rx, err_rx].into_iter().flatten() {
        // readers finish once the pipes close (child and group are dead)
        if let Ok(mut chunk) = rx.recv_timeout(Duration::from_secs(5)) {
            let room = cap_bytes.saturating_sub(output.len());
            chunk.truncate(room);
            output.extend_from_slice(&chunk);
        }
    }

    let exit_desc = if timed_out {
        "124 (timeout)".to_string()
    } else {
        match status {
            Some(s) => describe_status(s),
            None => "?".to_string(),
        }
    };
    CmdOutput {
        output,
        exit_desc,
        duration_secs: started.elapsed().as_secs(),
        timed_out,
    }
}

#[cfg(unix)]
fn kill_child(child: &mut std::process::Child) {
    let pid = child.id() as libc::pid_t;
    unsafe {
        // negative pid: the whole process group set up at spawn
        libc::kill(-pid, libc::SIGKILL);
        libc::kill(pid, libc::SIGKILL);
    }
}

#[cfg(not(unix))]
fn kill_child(child: &mut std::process::Child) {
    let _ = child.kill();
}

#[cfg(unix)]
fn describe_status(s: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (s.code(), s.signal()) {
        (Some(c), _) => c.to_string(),
        (None, Some(sig)) => format!("signal {sig}"),
        _ => "?".to_string(),
    }
}

#[cfg(not(unix))]
fn describe_status(s: std::process::ExitStatus) -> String {
    match s.code() {
        Some(c) => c.to_string(),
        None => "?".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn captures_output_and_exit() {
        let r = run_capped(
            &["sh", "-c", "echo hi; exit 3"],
            Duration::from_secs(5),
            1024,
            CaptureMode::Head,
        );
        assert_eq!(String::from_utf8_lossy(&r.output), "hi\n");
        assert_eq!(r.exit_desc, "3");
        assert!(!r.timed_out);
    }

    #[cfg(unix)]
    #[test]
    fn times_out_and_kills_group() {
        let started = Instant::now();
        let r = run_capped(
            &["sh", "-c", "sleep 30 & sleep 30"],
            Duration::from_millis(300),
            1024,
            CaptureMode::Head,
        );
        assert!(r.timed_out);
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[cfg(unix)]
    #[test]
    fn caps_output() {
        let r = run_capped(
            &["sh", "-c", "yes 0123456789 | head -c 100000"],
            Duration::from_secs(10),
            1000,
            CaptureMode::Head,
        );
        assert!(r.output.len() <= 1000);
    }

    #[test]
    fn missing_binary_degrades() {
        let r = run_capped(
            &["definitely-not-a-real-tool-xyz"],
            Duration::from_secs(1),
            1024,
            CaptureMode::Head,
        );
        assert_eq!(r.exit_desc, "?");
        assert!(String::from_utf8_lossy(&r.output).contains("failed to run"));
    }
}
