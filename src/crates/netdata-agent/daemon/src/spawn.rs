//! Child processes, started as C's `spawn_popen_run()` starts them (`src/libnetdata/spawn_server/spawn_popen.c`,
//! `spawn_server_nofork.c`): `/bin/sh -c <command>` with an empty signal mask and SIGPIPE at its default (the daemon
//! blocks or ignores both), the daemon's environment, stdin and stdout on pipes, stderr on the collectors' log.
//! Decisions D12 and D49 in the status repository.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::atomic::{AtomicUsize, Ordering};

use netdata_agent_log::{Priority, Source, nd_log};
use nix::fcntl::OFlag;
use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{SigSet, Signal};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{Pid, pipe2};

/// The spawn server's request counter: every child gets the next number (`server->request_id`).
static REQUEST_ID: AtomicUsize = AtomicUsize::new(0);

/// `argv_to_cmdline_buffer()`: the arguments joined by spaces; one with a blank or a `"` is quoted after the first
/// argument (only its closing quote before it) and its `"` escaped.
fn cmdline(args: &[&str]) -> String {
    let mut out = String::new();
    for arg in args {
        let quote = arg.contains([' ', '\u{b}', '\t', '\n', '"']);
        if !out.is_empty() {
            out.push_str(if quote { " \"" } else { " " });
        }
        out.push_str(&arg.replace('"', "\\\""));
        if quote {
            out.push('"');
        }
    }
    out
}

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
        let mut actions = PosixSpawnFileActions::init()?;
        actions.add_dup2(stdin_child.as_raw_fd(), 0)?;
        actions.add_dup2(stdout_child.as_raw_fd(), 1)?;
        // held open until the child exists
        let stderr = netdata_agent_log::collectors_fd();
        if let Some(fd) = &stderr {
            actions.add_dup2(fd.as_raw_fd(), 2)?;
        }
        let mut attr = PosixSpawnAttr::init()?;
        attr.set_sigmask(&SigSet::empty())?;
        // SIG_IGN survives exec: without this a child writing to a closed pipe gets EPIPE instead of dying
        let mut defaults = SigSet::empty();
        defaults.add(Signal::SIGPIPE);
        attr.set_sigdefault(&defaults)?;
        attr.set_flags(
            PosixSpawnFlags::POSIX_SPAWN_SETSIGMASK | PosixSpawnFlags::POSIX_SPAWN_SETSIGDEF,
        )?;
        let shell = c_string(b"/bin/sh")?;
        let args = [
            shell.clone(),
            c_string(b"-c")?,
            c_string(command.as_bytes())?,
        ];
        let env = std::env::vars_os()
            .filter_map(|(key, value)| {
                let mut pair = key.as_bytes().to_vec();
                pair.push(b'=');
                pair.extend_from_slice(OsStr::as_bytes(&value));
                CString::new(pair).ok()
            })
            .collect::<Vec<_>>();
        let pid = posix_spawn(shell.as_c_str(), &actions, &attr, &args, &env)?;
        drop((stdin_child, stdout_child, stderr));
        Ok(Popen {
            pid,
            request_id: REQUEST_ID.fetch_add(1, Ordering::Relaxed) + 1,
            cmdline: cmdline(&["/bin/sh", "-c", command]),
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
                Ok(WaitStatus::Exited(_, code)) => {
                    if code != 0 {
                        nd_log!(
                            Source::Collector,
                            Priority::Warning,
                            "SPAWN SERVER: child with pid {pid} (request {id}) exited with exit code {code}: {cmd}"
                        );
                    }
                    return Ok(code);
                }
                Ok(WaitStatus::Signaled(_, signal, core_dumped)) => {
                    let on_purpose = matches!(signal, Signal::SIGPIPE | Signal::SIGTERM);
                    let sig = signal as i32;
                    if core_dumped {
                        nd_log!(
                            Source::Collector,
                            Priority::Warning,
                            "SPAWN SERVER: child with pid {pid} (request {id}) coredump'd due to signal {sig}: {cmd}"
                        );
                    } else {
                        nd_log!(
                            Source::Collector,
                            if on_purpose {
                                Priority::Debug
                            } else {
                                Priority::Warning
                            },
                            "SPAWN SERVER: child with pid {pid} (request {id}) killed by signal {sig}: {cmd}"
                        );
                    }
                    return Ok(if on_purpose { 0 } else { -1 });
                }
                Ok(_) => {}
                Err(nix::errno::Errno::EINTR) => {}
                Err(e) => return Err(e.into()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::*;

    #[test]
    fn command_lines_as_c_prints_them() {
        assert_eq!(
            cmdline(&["/bin/sh", "-c", "/x/system-info.sh"]),
            "/bin/sh -c /x/system-info.sh"
        );
        assert_eq!(
            cmdline(&["/bin/sh", "-c", "a \"b\""]),
            r#"/bin/sh -c "a \"b\"""#
        );
        assert_eq!(cmdline(&["a b"]), "a b\"");
    }

    /// The ignored signals of this process, from `/proc/self/status`.
    fn ignored_here() -> u64 {
        let status = std::fs::read_to_string("/proc/self/status").unwrap();
        let line = status.lines().find(|l| l.starts_with("SigIgn:")).unwrap();
        u64::from_str_radix(line["SigIgn:".len()..].trim(), 16).unwrap()
    }

    /// The child starts with nothing blocked, SIGPIPE at its default (other ignored signals are inherited, as in C)
    /// and this process's environment, whatever this process blocks; its exit code comes back and is logged.
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
            cmdline(&["/bin/sh", "-c", command])
        );
        assert!(
            messages.len() == 1
                && messages[0].starts_with("SPAWN SERVER: child with pid ")
                && messages[0].ends_with(&tail),
            "{messages:?}"
        );
    }
}
