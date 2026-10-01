//! The child contract (`spawn_external_command()`, `spawn_server_nofork.c:293-366`): the program at `argv[0]` (no
//! PATH lookup) with its environment, the three descriptors on 0, 1 and 2, an empty signal mask, SIGPIPE back at its
//! default (the agent ignores it; C resets only it), and every other descriptor closed at the exec.

use std::ffi::CString;
use std::os::fd::RawFd;

use nix::errno::Errno;
use nix::spawn::{PosixSpawnAttr, PosixSpawnFileActions, PosixSpawnFlags, posix_spawn};
use nix::sys::signal::{SigSet, Signal};
use nix::unistd::Pid;

/// The step of `spawn_child()` that failed, by C's name, and its error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnError {
    pub step: &'static str,
    pub errno: Errno,
}

/// Starts `argv` with `envp` and `stdin`, `stdout`, `stderr` as its 0, 1 and 2 (C's `rq->fds[0..2]`).
pub fn spawn_child(
    argv: &[CString],
    envp: &[CString],
    stdin: RawFd,
    stdout: RawFd,
    stderr: RawFd,
) -> Result<Pid, SpawnError> {
    let fail = |step| move |errno| SpawnError { step, errno };
    let mut actions = PosixSpawnFileActions::init().map_err(fail("posix_spawn_file_actions_init()"))?;
    let fds = [stdin, stdout, stderr];
    for (target, &fd) in fds.iter().enumerate() {
        actions.add_dup2(fd, target as RawFd).map_err(fail("posix_spawn_file_actions_adddup2()"))?;
    }
    // C closes the three sources; one of them on 0-2 already would close the child's own (none is, from a request)
    for &fd in fds.iter().filter(|&&fd| fd > 2) {
        actions.add_close(fd).map_err(fail("posix_spawn_file_actions_addclose()"))?;
    }
    let mut attr = PosixSpawnAttr::init().map_err(fail("posix_spawnattr_init()"))?;
    attr.set_sigmask(&SigSet::empty()).map_err(fail("posix_spawnattr_setsigmask()"))?;
    // SIG_IGN survives exec: without this a child writing to a closed pipe gets EPIPE instead of dying
    let mut defaults = SigSet::empty();
    defaults.add(Signal::SIGPIPE);
    attr.set_sigdefault(&defaults).map_err(fail("posix_spawnattr_setsigdefault()"))?;
    attr.set_flags(PosixSpawnFlags::POSIX_SPAWN_SETSIGMASK | PosixSpawnFlags::POSIX_SPAWN_SETSIGDEF)
        .map_err(fail("posix_spawnattr_setflags()"))?;
    // the descriptors a request brought are the only ones the child keeps (no libsystemd journal fd, D135.3)
    let _ = netdata_agent_sys::close_fds_except(&fds, netdata_agent_sys::CloseMode::Cloexec);
    let path = argv.first().ok_or(SpawnError { step: "posix_spawn()", errno: Errno::EINVAL })?;
    posix_spawn(path.as_c_str(), &actions, &attr, argv, envp).map_err(fail("posix_spawn()"))
}
