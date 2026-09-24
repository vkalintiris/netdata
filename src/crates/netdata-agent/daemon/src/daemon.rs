//! Becoming a daemon, ported from `src/daemon/daemon.c`: the double fork, the pidfile, the umask, the OOM score, the
//! scheduling policy, the switch to the `run as user` account, and the ownership of the directories it writes.
//!
//! Not yet: chowning log files (logging, B5) and the analytics report of the OOM score.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::fs::OpenOptionsExt;

use netdata_agent_inicfg::{Config, LogLevel, SECTION_GLOBAL};
use netdata_agent_sys::{self as sys, Forked};
use nix::sys::stat::{Mode, SFlag, umask};
use nix::unistd::{Gid, Uid, User};

use crate::conf::Dirs;
use crate::system;

/// `OOM_SCORE_ADJ_MIN` and `OOM_SCORE_ADJ_MAX`.
const OOM_SCORE_ADJ_MIN: i64 = -1000;
const OOM_SCORE_ADJ_MAX: i64 = 1000;

type Log<'a> = &'a mut dyn FnMut(LogLevel, &str);

/// `fix_directory_file_permissions()`: regular files (and, recursively, directories) under `dir`.
fn fix_directory_file_permissions(dir: &str, uid: Uid, gid: Gid, recursive: bool, log: Log) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = format!("{dir}/{}", entry.file_name().to_string_lossy());
        if kind.is_file() || recursive {
            if nix::unistd::chown(path.as_str(), Some(uid), Some(gid)).is_err() {
                log(
                    LogLevel::Error,
                    &format!(
                        "Cannot chown {} '{path}' to {uid}:{gid}",
                        if kind.is_dir() { "directory" } else { "file" }
                    ),
                );
            }
        }
        if kind.is_dir() && recursive {
            fix_directory_file_permissions(&path, uid, gid, recursive, log);
        }
    }
}

/// `change_dir_ownership()`.
fn change_dir_ownership(dir: &str, uid: Uid, gid: Gid, recursive: bool, log: Log) {
    if dir.is_empty() {
        return;
    }
    if nix::unistd::chown(dir, Some(uid), Some(gid)).is_err() {
        log(
            LogLevel::Error,
            &format!("Cannot chown directory '{dir}' to {uid}:{gid}"),
        );
    }
    fix_directory_file_permissions(dir, uid, gid, recursive, log);
}

/// `prepare_required_directories()`.
fn prepare_required_directories(dirs: &Dirs, uid: Uid, gid: Gid, log: Log) {
    let run_dir = system::run_dir(true).unwrap_or_default();
    change_dir_ownership(&run_dir, uid, gid, false, log);
    change_dir_ownership(&dirs.cache, uid, gid, true, log);
    change_dir_ownership(&dirs.varlib, uid, gid, false, log);
    change_dir_ownership(&dirs.log, uid, gid, false, log);
    change_dir_ownership(&dirs.cloud, uid, gid, false, log);
    change_dir_ownership(&format!("{}/registry", dirs.varlib), uid, gid, false, log);
}

fn is_regular(mode: u32) -> bool {
    mode & SFlag::S_IFMT.bits() == SFlag::S_IFREG.bits()
}

/// `chown_open_file()`: a regular file open on `fd` that another account owns.
fn chown_open_file(fd: impl AsFd, uid: Uid, gid: Gid, log: Log) {
    let raw = fd.as_fd().as_raw_fd();
    let Ok(st) = nix::sys::stat::fstat(&fd) else {
        log(LogLevel::Error, &format!("Cannot fstat() fd {raw}"));
        return;
    };
    let regular = is_regular(st.st_mode);
    if (st.st_uid != uid.as_raw() || st.st_gid != gid.as_raw()) && regular {
        if nix::unistd::fchown(&fd, Some(uid), Some(gid)).is_err() {
            log(LogLevel::Error, &format!("Cannot fchown() fd {raw}."));
        }
    }
}

/// `become_user()`: the directories, the pidfile and stdout/stderr go to the account, then its groups and ids.
fn become_user(
    username: &str,
    pidfile: Option<&str>,
    pid_file: Option<&File>,
    dirs: &Dirs,
    log: Log,
) -> Result<(), ()> {
    let am_i_root = nix::unistd::getuid().is_root();
    let Some(pw) = User::from_name(username).ok().flatten() else {
        log(LogLevel::Error, &format!("User {username} is not present."));
        return Err(());
    };
    let (uid, gid) = (pw.uid, pw.gid);
    prepare_required_directories(dirs, uid, gid, log);
    if let Some(pidfile) = pidfile.filter(|p| !p.is_empty()) {
        if nix::unistd::chown(pidfile, Some(uid), Some(gid)).is_err() {
            log(
                LogLevel::Error,
                &format!("Cannot chown '{pidfile}' to {uid}:{gid}"),
            );
        }
    }
    let name = std::ffi::CString::new(username).map_err(|_| ())?;
    let groups = match nix::unistd::getgrouplist(&name, gid) {
        Ok(groups) => groups,
        Err(_) => {
            if am_i_root {
                log(
                    LogLevel::Error,
                    &format!("Cannot get supplementary groups of user '{username}'."),
                );
            }
            Vec::new()
        }
    };
    chown_open_file(std::io::stdout(), uid, gid, log);
    chown_open_file(std::io::stderr(), uid, gid, log);
    if let Some(f) = pid_file {
        chown_open_file(f, uid, gid, log);
    }
    if !groups.is_empty() && nix::unistd::setgroups(&groups).is_err() && am_i_root {
        log(
            LogLevel::Error,
            &format!("Cannot set supplementary groups for user '{username}'"),
        );
    }
    let fail = |log: Log, message: String| {
        log(LogLevel::Error, &message);
        Err(())
    };
    if nix::unistd::setresgid(gid, gid, gid).is_err() {
        return fail(
            log,
            format!("Cannot switch to user's {username} group (gid: {gid})."),
        );
    }
    if nix::unistd::setresuid(uid, uid, uid).is_err() {
        return fail(
            log,
            format!("Cannot switch to user {username} (uid: {uid})."),
        );
    }
    if nix::unistd::setgid(gid).is_err() {
        return fail(
            log,
            format!("Cannot switch to user's {username} group (gid: {gid})."),
        );
    }
    if nix::unistd::setegid(gid).is_err() {
        return fail(
            log,
            format!("Cannot effectively switch to user's {username} group (gid: {gid})."),
        );
    }
    if nix::unistd::setuid(uid).is_err() {
        return fail(
            log,
            format!("Cannot switch to user {username} (uid: {uid})."),
        );
    }
    if nix::unistd::seteuid(uid).is_err() {
        return fail(
            log,
            format!("Cannot effectively switch to user {username} (uid: {uid})."),
        );
    }
    Ok(())
}

/// `read_single_signed_number_file()`.
fn read_signed(path: &str) -> Option<i64> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(netdata_agent_text::parse::str2ll(text.as_bytes()).0)
}

/// `oom_score_adj()`: `[global] OOM score`, defaulting to `$OOMScoreAdjust` or the current score; `keep` keeps it.
fn oom_score_adj(c: &mut Config, log: Log) {
    const PATH: &str = "/proc/self/oom_score_adj";
    let Some(old_score) = read_signed(PATH) else {
        log(
            LogLevel::Error,
            "Out-Of-Memory (OOM) score setting is not supported on this system.",
        );
        return;
    };
    let mut wanted = old_score;
    let default = std::env::var("OOMScoreAdjust")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| (wanted as i32).to_string());
    let s = String::from_utf8_lossy(
        &c.get(SECTION_GLOBAL, "OOM score", Some(&default))
            .unwrap_or_default(),
    )
    .into_owned();
    match s.bytes().next() {
        Some(b) if b.is_ascii_digit() || b == b'-' || b == b'+' => {
            wanted = netdata_agent_text::parse::str2ll(s.as_bytes()).0;
        }
        _ if s == "keep" => {
            log(
                LogLevel::Info,
                &format!(
                    "Out-Of-Memory (OOM) kept as-is (running with {})",
                    old_score as i32
                ),
            );
            return;
        }
        _ => {
            log(
                LogLevel::Info,
                &format!(
                    "Out-Of-Memory (OOM) score not changed due to non-numeric setting: '{s}' (running with {})",
                    old_score as i32
                ),
            );
            return;
        }
    }
    if wanted < OOM_SCORE_ADJ_MIN {
        log(
            LogLevel::Error,
            &format!(
                "Wanted Out-Of-Memory (OOM) score {} is too small. Using {OOM_SCORE_ADJ_MIN}",
                wanted as i32
            ),
        );
        wanted = OOM_SCORE_ADJ_MIN;
    }
    if wanted > OOM_SCORE_ADJ_MAX {
        log(
            LogLevel::Error,
            &format!(
                "Wanted Out-Of-Memory (OOM) score {} is too big. Using {OOM_SCORE_ADJ_MAX}",
                wanted as i32
            ),
        );
        wanted = OOM_SCORE_ADJ_MAX;
    }
    if old_score == wanted {
        log(
            LogLevel::Info,
            &format!(
                "Out-Of-Memory (OOM) score is already set to the wanted value {}",
                old_score as i32
            ),
        );
        return;
    }
    let Ok(mut f) = OpenOptions::new().write(true).open(PATH) else {
        log(
            LogLevel::Error,
            "Failed to adjust my Out-Of-Memory (OOM) score. Cannot open /proc/self/oom_score_adj for writing.",
        );
        return;
    };
    let buf = (wanted as i32).to_string();
    if f.write(buf.as_bytes()).is_ok_and(|n| n == buf.len()) {
        drop(f);
        match read_signed(PATH) {
            None => log(
                LogLevel::Error,
                &format!(
                    "Adjusted my Out-Of-Memory (OOM) score to {}, but cannot verify it.",
                    wanted as i32
                ),
            ),
            Some(final_score) if final_score == wanted => log(
                LogLevel::Info,
                &format!(
                    "Adjusted my Out-Of-Memory (OOM) score from {} to {}.",
                    old_score as i32, final_score as i32
                ),
            ),
            Some(final_score) => log(
                LogLevel::Error,
                &format!(
                    "Adjusted my Out-Of-Memory (OOM) score from {} to {}, but it has been set to {}.",
                    old_score as i32, wanted as i32, final_score as i32
                ),
            ),
        }
    } else {
        log(
            LogLevel::Error,
            &format!(
                "Failed to adjust my Out-Of-Memory (OOM) score to {}. Running with {}. (systemd systems may change it via netdata.service)",
                wanted as i32, old_score as i32
            ),
        );
    }
}

/// `SCHED_FLAG_*`.
const PRIORITY_CONFIGURABLE: u8 = 0x01;
const KEEP_AS_IS: u8 = 0x04;
const USE_NICE: u8 = 0x08;

/// `scheduler_defaults[]`, in C's order: the report names the first entry whose policy matches, so a process under
/// SCHED_OTHER reports `keep`.
const SCHEDULERS: [(&str, i32, i32, u8); 8] = [
    ("keep", 0, 0, KEEP_AS_IS),
    ("none", 0, 0, KEEP_AS_IS),
    ("batch", sys::SCHED_BATCH, 0, USE_NICE),
    ("other", sys::SCHED_OTHER, 0, USE_NICE),
    ("nice", sys::SCHED_OTHER, 0, USE_NICE),
    ("idle", sys::SCHED_IDLE, 0, 0),
    ("rr", sys::SCHED_RR, 0, PRIORITY_CONFIGURABLE),
    ("fifo", sys::SCHED_FIFO, 0, PRIORITY_CONFIGURABLE),
];

/// `process_nice_level()`: `[global] process nice level`.
fn process_nice_level(c: &mut Config, log: Log) {
    let level = c.get_number(SECTION_GLOBAL, "process nice level", 0) as i32;
    if sys::nice(level).is_err() {
        log(
            LogLevel::Error,
            &format!("Cannot set netdata CPU nice level to {level}."),
        );
    }
}

/// `sched_getscheduler_report()`.
fn sched_getscheduler_report(log: Log) {
    let Ok(policy) = sys::sched_getscheduler() else {
        log(
            LogLevel::Error,
            "Cannot get my current process scheduling policy.",
        );
        return;
    };
    let Some(&(name, _, _, flags)) = SCHEDULERS.iter().find(|s| s.1 == policy) else {
        return;
    };
    if flags & PRIORITY_CONFIGURABLE != 0 {
        match sys::sched_getparam_priority() {
            Err(_) => log(
                LogLevel::Error,
                &format!("Cannot get the process scheduling priority for my policy '{name}'"),
            ),
            Ok(priority) => log(
                LogLevel::Info,
                &format!("Running with process scheduling policy '{name}', priority {priority}"),
            ),
        }
    } else if flags & USE_NICE != 0 {
        let n = sys::getpriority_self().unwrap_or(-1);
        log(
            LogLevel::Info,
            &format!("Running with process scheduling policy '{name}', nice level {n}"),
        );
    } else {
        log(
            LogLevel::Info,
            &format!("Running with process scheduling policy '{name}'"),
        );
    }
}

/// `sched_setscheduler_set()`: `[global] process scheduling policy` (and priority), falling back to nice.
fn sched_setscheduler_set(c: &mut Config, log: Log) {
    let name = String::from_utf8_lossy(
        &c.get(
            SECTION_GLOBAL,
            "process scheduling policy",
            Some(SCHEDULERS[0].0),
        )
        .unwrap_or_default(),
    )
    .into_owned();
    let found = SCHEDULERS.iter().find(|s| s.0 == name);
    let Some(&(_, policy, mut priority, flags)) = found else {
        log(
            LogLevel::Error,
            &format!("Unknown scheduling policy '{name}' - falling back to nice"),
        );
        process_nice_level(c, log);
        sched_getscheduler_report(log);
        return;
    };
    if flags & KEEP_AS_IS != 0 {
        sched_getscheduler_report(log);
        return;
    }
    if flags & PRIORITY_CONFIGURABLE != 0 {
        priority = c.get_number(
            SECTION_GLOBAL,
            "process scheduling priority",
            i64::from(priority),
        ) as i32;
    }
    let min = sys::sched_get_priority_min(policy);
    if priority < min {
        log(
            LogLevel::Error,
            &format!(
                "scheduler {name} ({policy}) priority {priority} is below the minimum {min}. Using the minimum."
            ),
        );
        priority = min;
    }
    let max = sys::sched_get_priority_max(policy);
    if priority > max {
        log(
            LogLevel::Error,
            &format!(
                "scheduler {name} ({policy}) priority {priority} is above the maximum {max}. Using the maximum."
            ),
        );
        priority = max;
    }
    match sys::sched_setscheduler(policy, priority) {
        Err(_) => {
            log(
                LogLevel::Error,
                &format!(
                    "Cannot adjust netdata scheduling policy to {name} ({policy}), with priority {priority}. Falling back to nice."
                ),
            );
            process_nice_level(c, log);
        }
        Ok(()) => {
            log(
                LogLevel::Info,
                &format!(
                    "Adjusted netdata scheduling policy to {name} ({policy}), with priority {priority}."
                ),
            );
            if flags & USE_NICE != 0 {
                process_nice_level(c, log);
            }
        }
    }
    sched_getscheduler_report(log);
}

/// What `become_daemon()` leaves to its caller.
pub enum Outcome {
    /// This process continues as the daemon.
    Continue,
    /// A parent of the forks: it exits with status 0 at once.
    ExitParent,
}

/// `become_daemon()`: unless `dont_fork`, fork twice around `setsid()`; then the pidfile, umask 0007, the OOM score,
/// the scheduling policy, and the switch to `user` (or the directory ownership for the current account).
pub fn become_daemon(
    dont_fork: bool,
    user: &str,
    pidfile: Option<&str>,
    c: &mut Config,
    dirs: &Dirs,
    log: Log,
) -> Result<Outcome, String> {
    if !dont_fork {
        match sys::fork().map_err(|e| format!("cannot fork: {e}"))? {
            Forked::Parent { .. } => return Ok(Outcome::ExitParent),
            Forked::Child => {}
        }
        nix::unistd::setsid().map_err(|_| "Cannot become session leader.".to_string())?;
        match sys::fork().map_err(|e| format!("cannot fork for a second time: {e}"))? {
            Forked::Parent { .. } => return Ok(Outcome::ExitParent),
            Forked::Child => {}
        }
    }
    let mut pid_file = None;
    if let Some(path) = pidfile.filter(|p| !p.is_empty()) {
        // std opens with O_CLOEXEC.
        match OpenOptions::new()
            .write(true)
            .create(true)
            // C opens without O_TRUNC and truncates with ftruncate(), whose failure it reports.
            .truncate(false)
            .mode(0o644)
            .open(path)
        {
            Ok(mut f) => {
                if f.set_len(0).is_err() {
                    log(
                        LogLevel::Error,
                        &format!("Cannot truncate pidfile '{path}'."),
                    );
                }
                if !f
                    .write(format!("{}\n", std::process::id()).as_bytes())
                    .is_ok_and(|n| n > 0)
                {
                    log(LogLevel::Error, &format!("Cannot write pidfile '{path}'."));
                }
                pid_file = Some(f);
            }
            Err(_) => log(
                LogLevel::Error,
                &format!("Failed to open pidfile '{path}'."),
            ),
        }
    }
    umask(Mode::from_bits_truncate(0o007));
    oom_score_adj(c, log);
    sched_setscheduler_set(c, log);
    if !user.is_empty() {
        match become_user(user, pidfile, pid_file.as_ref(), dirs, log) {
            Err(()) => log(
                LogLevel::Error,
                &format!("Cannot switch to user '{user}'. Continuing with current privileges."),
            ),
            Ok(()) => log(
                LogLevel::Info,
                &format!("Successfully switched to user '{user}'."),
            ),
        }
    } else {
        prepare_required_directories(dirs, nix::unistd::getuid(), nix::unistd::getgid(), log);
    }
    drop(pid_file);
    Ok(Outcome::Continue)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scheduler_table_reports_keep_for_sched_other() {
        let first = SCHEDULERS.iter().find(|s| s.1 == sys::SCHED_OTHER).unwrap();
        assert_eq!(first.0, "keep");
    }

    #[test]
    fn regular_files_are_recognized() {
        let f = File::open("/proc/self/status").unwrap();
        assert!(is_regular(nix::sys::stat::fstat(&f).unwrap().st_mode));
        let d = File::open("/proc/self").unwrap();
        assert!(!is_regular(nix::sys::stat::fstat(&d).unwrap().st_mode));
    }
}
