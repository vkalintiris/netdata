//! Becoming a daemon, ported from `src/daemon/daemon.c`: the double fork, the pidfile, the umask, the OOM score, the
//! scheduling policy, the switch to the `run as user` account, and the ownership of the directories it writes.
//!
//! Not yet: the analytics report of the OOM score.

use netdata_agent_log::{Priority, Source, chown_open_file, fatal, nd_log};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

use netdata_agent_inicfg::{Config, SECTION_GLOBAL};
use netdata_agent_sys::{self as sys, Forked};
use nix::sys::stat::{Mode, umask};
use nix::unistd::{Gid, Uid, User};

use crate::conf::Dirs;
use crate::system;

/// `OOM_SCORE_ADJ_MIN` and `OOM_SCORE_ADJ_MAX`.
const OOM_SCORE_ADJ_MIN: i64 = -1000;
const OOM_SCORE_ADJ_MAX: i64 = 1000;

/// `fix_directory_file_permissions()`: regular files (and, recursively, directories) under `dir`.
fn fix_directory_file_permissions(dir: &str, uid: Uid, gid: Gid, recursive: bool) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = format!("{dir}/{}", entry.file_name().to_string_lossy());
        if kind.is_file() || recursive {
            if let Err(e) = nix::unistd::chown(path.as_str(), Some(uid), Some(gid)) {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = e as i32;
                    "Cannot chown {} '{path}' to {uid}:{gid}",
                    if kind.is_dir() { "directory" } else { "file" }
                );
            }
        }
        if kind.is_dir() && recursive {
            fix_directory_file_permissions(&path, uid, gid, recursive);
        }
    }
}

/// `change_dir_ownership()`.
fn change_dir_ownership(dir: &str, uid: Uid, gid: Gid, recursive: bool) {
    if dir.is_empty() {
        return;
    }
    if let Err(e) = nix::unistd::chown(dir, Some(uid), Some(gid)) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            errno = e as i32;
            "Cannot chown directory '{dir}' to {uid}:{gid}"
        );
    }
    fix_directory_file_permissions(dir, uid, gid, recursive);
}

/// `prepare_required_directories()`.
fn prepare_required_directories(dirs: &Dirs, uid: Uid, gid: Gid) {
    let run_dir = system::run_dir(true).unwrap_or_default();
    change_dir_ownership(&run_dir, uid, gid, false);
    change_dir_ownership(&dirs.cache, uid, gid, true);
    change_dir_ownership(&dirs.varlib, uid, gid, false);
    change_dir_ownership(&dirs.log, uid, gid, false);
    change_dir_ownership(&dirs.cloud, uid, gid, false);
    change_dir_ownership(&format!("{}/registry", dirs.varlib), uid, gid, false);
}

/// `become_user()`: the directories, the pidfile and stdout/stderr go to the account, then its groups and ids.
fn become_user(
    username: &str,
    pidfile: Option<&str>,
    pid_file: Option<&File>,
    dirs: &Dirs,
) -> Result<(), ()> {
    let am_i_root = nix::unistd::getuid().is_root();
    let Some(pw) = User::from_name(username).ok().flatten() else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "User {username} is not present."
        );
        return Err(());
    };
    let (uid, gid) = (pw.uid, pw.gid);
    prepare_required_directories(dirs, uid, gid);
    if let Some(pidfile) = pidfile.filter(|p| !p.is_empty()) {
        if let Err(e) = nix::unistd::chown(pidfile, Some(uid), Some(gid)) {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                errno = e as i32;
                "Cannot chown '{pidfile}' to {uid}:{gid}"
            );
        }
    }
    let name = std::ffi::CString::new(username).map_err(|_| ())?;
    let groups = match nix::unistd::getgrouplist(&name, gid) {
        Ok(groups) => groups,
        Err(_) => {
            if am_i_root {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Cannot get supplementary groups of user '{username}'."
                );
            }
            Vec::new()
        }
    };
    netdata_agent_log::chown_log_files(uid.as_raw(), gid.as_raw());
    chown_open_file(std::io::stdout(), uid.as_raw(), gid.as_raw());
    chown_open_file(std::io::stderr(), uid.as_raw(), gid.as_raw());
    if let Some(f) = pid_file {
        chown_open_file(f, uid.as_raw(), gid.as_raw());
    }
    if !groups.is_empty() && nix::unistd::setgroups(&groups).is_err() && am_i_root {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Cannot set supplementary groups for user '{username}'"
        );
    }
    let fail = |errno: nix::errno::Errno, message: String| {
        nd_log!(Source::Daemon, Priority::Err, errno = errno as i32; "{message}");
        Err(())
    };
    if let Err(errno) = nix::unistd::setresgid(gid, gid, gid) {
        return fail(
            errno,
            format!("Cannot switch to user's {username} group (gid: {gid})."),
        );
    }
    if let Err(errno) = nix::unistd::setresuid(uid, uid, uid) {
        return fail(
            errno,
            format!("Cannot switch to user {username} (uid: {uid})."),
        );
    }
    if let Err(errno) = nix::unistd::setgid(gid) {
        return fail(
            errno,
            format!("Cannot switch to user's {username} group (gid: {gid})."),
        );
    }
    if let Err(errno) = nix::unistd::setegid(gid) {
        return fail(
            errno,
            format!("Cannot effectively switch to user's {username} group (gid: {gid})."),
        );
    }
    if let Err(errno) = nix::unistd::setuid(uid) {
        return fail(
            errno,
            format!("Cannot switch to user {username} (uid: {uid})."),
        );
    }
    if let Err(errno) = nix::unistd::seteuid(uid) {
        return fail(
            errno,
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
fn oom_score_adj(c: &mut Config) {
    const PATH: &str = "/proc/self/oom_score_adj";
    let Some(old_score) = read_signed(PATH) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Out-Of-Memory (OOM) score setting is not supported on this system."
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
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "Out-Of-Memory (OOM) kept as-is (running with {})",
                old_score as i32
            );
            return;
        }
        _ => {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "Out-Of-Memory (OOM) score not changed due to non-numeric setting: '{s}' (running with {})",
                old_score as i32
            );
            return;
        }
    }
    if wanted < OOM_SCORE_ADJ_MIN {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Wanted Out-Of-Memory (OOM) score {} is too small. Using {OOM_SCORE_ADJ_MIN}",
            wanted as i32
        );
        wanted = OOM_SCORE_ADJ_MIN;
    }
    if wanted > OOM_SCORE_ADJ_MAX {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Wanted Out-Of-Memory (OOM) score {} is too big. Using {OOM_SCORE_ADJ_MAX}",
            wanted as i32
        );
        wanted = OOM_SCORE_ADJ_MAX;
    }
    if old_score == wanted {
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "Out-Of-Memory (OOM) score is already set to the wanted value {}",
            old_score as i32
        );
        return;
    }
    let Ok(mut f) = OpenOptions::new().write(true).open(PATH) else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Failed to adjust my Out-Of-Memory (OOM) score. Cannot open /proc/self/oom_score_adj for writing."
        );
        return;
    };
    let buf = (wanted as i32).to_string();
    if f.write(buf.as_bytes()).is_ok_and(|n| n == buf.len()) {
        drop(f);
        match read_signed(PATH) {
            None => nd_log!(
                Source::Daemon,
                Priority::Err,
                "Adjusted my Out-Of-Memory (OOM) score to {}, but cannot verify it.",
                wanted as i32
            ),
            Some(final_score) if final_score == wanted => nd_log!(
                Source::Daemon,
                Priority::Info,
                "Adjusted my Out-Of-Memory (OOM) score from {} to {}.",
                old_score as i32,
                final_score as i32
            ),
            Some(final_score) => nd_log!(
                Source::Daemon,
                Priority::Err,
                "Adjusted my Out-Of-Memory (OOM) score from {} to {}, but it has been set to {}.",
                old_score as i32,
                wanted as i32,
                final_score as i32
            ),
        }
    } else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Failed to adjust my Out-Of-Memory (OOM) score to {}. Running with {}. (systemd systems may change it via netdata.service)",
            wanted as i32,
            old_score as i32
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
fn process_nice_level(c: &mut Config) {
    let level = c.get_number(SECTION_GLOBAL, "process nice level", 0) as i32;
    if let Err(e) = sys::nice(level) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            errno = netdata_agent_log::errno_of(&e);
            "Cannot set netdata CPU nice level to {level}."
        );
    }
}

/// `sched_getscheduler_report()`.
fn sched_getscheduler_report() {
    let Ok(policy) = sys::sched_getscheduler() else {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Cannot get my current process scheduling policy."
        );
        return;
    };
    let Some(&(name, _, _, flags)) = SCHEDULERS.iter().find(|s| s.1 == policy) else {
        return;
    };
    if flags & PRIORITY_CONFIGURABLE != 0 {
        match sys::sched_getparam_priority() {
            Err(_) => nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot get the process scheduling priority for my policy '{name}'"
            ),
            Ok(priority) => nd_log!(
                Source::Daemon,
                Priority::Info,
                "Running with process scheduling policy '{name}', priority {priority}"
            ),
        }
    } else if flags & USE_NICE != 0 {
        let n = sys::getpriority_self().unwrap_or(-1);
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "Running with process scheduling policy '{name}', nice level {n}"
        );
    } else {
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "Running with process scheduling policy '{name}'"
        );
    }
}

/// `sched_setscheduler_set()`: `[global] process scheduling policy` (and priority), falling back to nice.
fn sched_setscheduler_set(c: &mut Config) {
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
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Unknown scheduling policy '{name}' - falling back to nice"
        );
        process_nice_level(c);
        sched_getscheduler_report();
        return;
    };
    if flags & KEEP_AS_IS != 0 {
        sched_getscheduler_report();
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
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "scheduler {name} ({policy}) priority {priority} is below the minimum {min}. Using the minimum."
        );
        priority = min;
    }
    let max = sys::sched_get_priority_max(policy);
    if priority > max {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "scheduler {name} ({policy}) priority {priority} is above the maximum {max}. Using the maximum."
        );
        priority = max;
    }
    match sys::sched_setscheduler(policy, priority) {
        Err(_) => {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot adjust netdata scheduling policy to {name} ({policy}), with priority {priority}. Falling back to nice."
            );
            process_nice_level(c);
        }
        Ok(()) => {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "Adjusted netdata scheduling policy to {name} ({policy}), with priority {priority}."
            );
            if flags & USE_NICE != 0 {
                process_nice_level(c);
            }
        }
    }
    sched_getscheduler_report();
}

/// What `become_daemon()` leaves to its caller.
pub enum Outcome {
    /// This process continues as the daemon.
    Continue,
    /// A parent of the forks: it exits with status 0 at once.
    ExitParent,
}

/// `become_daemon()`: unless `dont_fork`, fork twice around `setsid()`; then the pidfile, umask 0007, the OOM score,
/// the scheduling policy, and the switch to `user` (or the directory ownership for the current account). A failed
/// fork or `setsid()` is C's `fatal()`.
pub fn become_daemon(
    dont_fork: bool,
    user: &str,
    pidfile: Option<&str>,
    c: &mut Config,
    dirs: &Dirs,
) -> Outcome {
    if !dont_fork {
        match sys::fork() {
            Ok(Forked::Parent { .. }) => return Outcome::ExitParent,
            Ok(Forked::Child) => netdata_agent_log::forked(),
            Err(err) => fatal!(errno = netdata_agent_log::errno_of(&err); "cannot fork"),
        }
        if let Err(errno) = nix::unistd::setsid() {
            fatal!(errno = errno as i32; "Cannot become session leader.");
        }
        match sys::fork() {
            Ok(Forked::Parent { .. }) => return Outcome::ExitParent,
            Ok(Forked::Child) => netdata_agent_log::forked(),
            Err(err) => {
                fatal!(errno = netdata_agent_log::errno_of(&err); "cannot fork for a second time")
            }
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
                if let Err(e) = f.set_len(0) {
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        errno = netdata_agent_log::errno_of(&e);
                        "Cannot truncate pidfile '{path}'."
                    );
                }
                if !f
                    .write(format!("{}\n", std::process::id()).as_bytes())
                    .is_ok_and(|n| n > 0)
                {
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "Cannot write pidfile '{path}'."
                    );
                }
                pid_file = Some(f);
            }
            Err(_) => nd_log!(
                Source::Daemon,
                Priority::Err,
                "Failed to open pidfile '{path}'."
            ),
        }
    }
    umask(Mode::from_bits_truncate(0o007));
    oom_score_adj(c);
    sched_setscheduler_set(c);
    if !user.is_empty() {
        match become_user(user, pidfile, pid_file.as_ref(), dirs) {
            Err(()) => nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot switch to user '{user}'. Continuing with current privileges."
            ),
            Ok(()) => nd_log!(
                Source::Daemon,
                Priority::Info,
                "Successfully switched to user '{user}'."
            ),
        }
    } else {
        prepare_required_directories(dirs, nix::unistd::getuid(), nix::unistd::getgid());
    }
    drop(pid_file);
    Outcome::Continue
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scheduler_table_reports_keep_for_sched_other() {
        let first = SCHEDULERS.iter().find(|s| s.1 == sys::SCHED_OTHER).unwrap();
        assert_eq!(first.0, "keep");
    }
}
