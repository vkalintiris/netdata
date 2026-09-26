//! Child processes, started as C's `spawn_popen_run()` starts them (`src/libnetdata/spawn_server/spawn_popen.c`,
//! `spawn_server_nofork.c`): `/bin/sh -c <command>` with an empty signal mask and SIGPIPE at its default (the daemon
//! blocks or ignores both), the daemon's environment, stdin and stdout on pipes, stderr on the collectors' log.
//! Decisions D12 and D49 in the status repository.

use std::ffi::{CString, OsStr};
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;

use nix::fcntl::OFlag;
use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{SigSet, Signal};
use nix::sys::wait::{WaitStatus, waitpid};
use nix::unistd::{Pid, pipe2};

/// A running child and the parent's ends of its pipes (`POPEN_INSTANCE`).
#[derive(Debug)]
pub struct Popen {
    pid: Pid,
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
        let stderr = netdata_agent_log::collectors_fd();
        if stderr != 2 {
            actions.add_dup2(stderr, 2)?;
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
        drop((stdin_child, stdout_child));
        Ok(Popen {
            pid,
            stdin: Some(stdin_parent),
            stdout: File::from(stdout_parent),
        })
    }

    /// The child's stdout.
    pub fn stdout(&mut self) -> &mut File {
        &mut self.stdout
    }

    /// `spawn_popen_wait()`: closes the pipes and reaps the child; its exit code (128 + the signal when killed).
    pub fn wait(mut self) -> io::Result<i32> {
        self.stdin.take();
        drop(self.stdout);
        loop {
            match waitpid(self.pid, None) {
                Ok(WaitStatus::Exited(_, code)) => return Ok(code),
                Ok(WaitStatus::Signaled(_, signal, _)) => return Ok(128 + signal as i32),
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

    /// The child starts with nothing blocked, SIGPIPE at its default and this process's environment, whatever this
    /// process blocks.
    #[test]
    fn children_start_clean() {
        let mut blocked = SigSet::empty();
        blocked.add(Signal::SIGTERM);
        blocked.add(Signal::SIGINT);
        blocked.thread_block().unwrap();
        let mut child = Popen::run(
            "grep -E '^Sig(Blk|Ign):' /proc/self/status; echo \"PATH=${PATH:+set}\"; exit 3",
        )
        .unwrap();
        let mut out = String::new();
        child.stdout().read_to_string(&mut out).unwrap();
        let code = child.wait().unwrap();
        blocked.thread_unblock().unwrap();
        assert_eq!(code, 3);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            [
                "SigBlk:\t0000000000000000",
                "SigIgn:\t0000000000000000",
                "PATH=set"
            ],
            "{out}"
        );
    }
}
