//! Child processes, started as C's `spawn_popen_run()` starts them (`src/libnetdata/spawn_server/spawn_popen.c`,
//! `spawn_server_nofork.c`): `/bin/sh -c <command>` with the spawn crate's child contract (an empty signal mask,
//! SIGPIPE at its default), the children's environment, stdin and stdout on pipes, stderr on the collectors' log.
//! In-process until the spawn server is wired in, then removed (decisions D12, D49, D136).

use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};

use netdata_agent_spawn::{env, exec, server, wire};
use nix::fcntl::OFlag;
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{Pid, pipe2};

/// The spawn server's request counter: every child gets the next number (`server->request_id`).
static REQUEST_ID: AtomicUsize = AtomicUsize::new(0);

/// A running child and the parent's ends of its pipes (`POPEN_INSTANCE`).
#[derive(Debug)]
#[must_use = "a child must be waited for"]
pub struct Popen {
    pid: Pid,
    request_id: usize,
    cmdline: String,
    /// Held open until `wait()`, as C keeps the child's stdin until `spawn_popen_wait()`.
    stdin: Option<OwnedFd>,
    stdout: File,
}

fn c_string(bytes: &[u8]) -> io::Result<CString> {
    CString::new(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

impl Popen {
    /// `spawn_popen_run(command)`.
    pub fn run(command: &str) -> io::Result<Popen> {
        let (stdin_child, stdin_parent) = pipe2(OFlag::O_CLOEXEC)?;
        let (stdout_parent, stdout_child) = pipe2(OFlag::O_CLOEXEC)?;
        // held open until the child exists; without a collectors log the child shares the daemon's stderr
        let stderr = netdata_agent_log::collectors_fd();
        let args = [
            c_string(b"/bin/sh")?,
            c_string(b"-c")?,
            c_string(command.as_bytes())?,
        ];
        let pid = exec::spawn_child(
            &args,
            &env::block(),
            stdin_child.as_raw_fd(),
            stdout_child.as_raw_fd(),
            stderr.as_ref().map_or(2, AsRawFd::as_raw_fd),
        )
        .map_err(|e| io::Error::from(e.errno))?;
        drop((stdin_child, stdout_child, stderr));
        Ok(Popen {
            pid,
            request_id: REQUEST_ID.fetch_add(1, Ordering::Relaxed) + 1,
            cmdline: wire::cmdline(&["/bin/sh", "-c", command]),
            stdin: Some(stdin_parent),
            stdout: File::from(stdout_parent),
        })
    }

    /// The child's stdout.
    pub fn stdout(&mut self) -> &mut File {
        &mut self.stdout
    }

    /// `spawn_popen_wait()`: closes the pipes and reaps the child, logging its end as C's spawn server does
    /// (collectors source). Returns `spawn_popen_status_rc()`: the exit code, 0 when killed by SIGTERM or SIGPIPE (how
    /// children are stopped on purpose), else -1.
    pub fn wait(mut self) -> io::Result<i32> {
        self.stdin.take();
        drop(self.stdout);
        let (pid, id, cmd) = (self.pid, self.request_id, &self.cmdline);
        loop {
            match waitpid(pid, None) {
                Ok(status @ (WaitStatus::Exited(..) | WaitStatus::Signaled(..))) => {
                    let raw = wire::raw_wait_status(status);
                    server::log_reaped(pid.as_raw(), id, raw, Some(cmd));
                    return Ok(wire::status_rc(raw));
                }
                Ok(_) | Err(nix::errno::Errno::EINTR) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use nix::sys::signal::{SigSet, Signal};

    use super::*;

    /// The ignored signals of this process, from `/proc/self/status`.
    fn ignored_here() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("SigIgn:")).unwrap();
        u64::from_str_radix(line["SigIgn:".len()..].trim(), 16).unwrap()
    }

    /// The child starts with nothing blocked, SIGPIPE at its default (other ignored signals are inherited: C's
    /// spawn server resets only SIGPIPE, the daemon's startup resets the ones C catches, D123) and this process's
    /// environment, whatever this process blocks; its exit code comes back and is logged.
    #[test]
    fn children_start_clean() {
        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGTERM);
        blocked.add(Signal::SIGINT);
        blocked.thread_block().unwrap();
        let command =
            "grep -E '^Sig(Blk|Ign):' /proc/self/status; echo \"PATH=${PATH:+set}\"; exit 3";
        let mut child = Popen::run(command).unwrap();
        let mut out = String::new();
        child.stdout().read_to_string(&mut out).unwrap();
        let (code, records) = netdata_agent_log::capture(|| child.wait().unwrap());
        blocked.thread_unblock().unwrap();
        assert_eq!(code, 3);
        let sigpipe = 1u64 << (Signal::SIGPIPE as i32 - 1);
        let ignored = format!("SigIgn:\t{:016x}", ignored_here() & !sigpipe);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            ["SigBlk:\t0000000000000000", ignored.as_str(), "PATH=set"],
            "{out}"
        );
        let messages: Vec<String> = records.into_iter().filter_map(|r| r.message).collect();
        let tail = format!(
            ") exited with exit code 3: {}",
            wire::cmdline(&["/bin/sh", "-c", command])
        );
        assert!(
            messages.len() == 1
                && messages[0].starts_with("SPAWN SERVER: child with pid ")
                && messages[0].ends_with(&tail),
            "{messages:?}"
        );
    }
}
