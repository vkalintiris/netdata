//! This run's status file (`status-file.c`'s `last_session_status` and `session_status`): the last run's record
//! loaded at start and reported in the "Last exit status" record, this run's record carried over from it, refreshed
//! and saved at every startup and shutdown step, every 15 minutes and at exit. Before `init()` nothing is saved.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError, TryLockError};

use netdata_agent_inicfg::{Config, SECTION_GLOBAL};
use netdata_agent_log::{Field, Priority, Source, Value, msgid, nd_log, push};

use super::io::{self, Locations, STATUS_FILENAME};
use super::{DaemonStatus, Product, StatusFile, dmi, exit_reason, from_json, live, rfc3339, to_json};
use crate::build;

/// `STACK_TRACE_INFO_PREFIX`.
const INFO_PREFIX: &str = "info: ";
/// `DAEMON_STATUS_FILE_ROLLING_SHUTDOWN_TIMINGS_HEADER`.
pub const SHUTDOWN_TIMINGS_HEADER: &str = "info:  shutdown steps timings";

/// The records and where they are saved.
struct Files {
    loc: Locations,
    last: StatusFile,
    session: StatusFile,
}

/// The one lock of every refresh and save (D88.9).
static FILES: Mutex<Option<Files>> = Mutex::new(None);

/// `shutdown_timeout_spinlock`: kept once the shutdown timed out, so no later step overwrites its record.
static TIMEOUT: Mutex<()> = Mutex::new(());

fn files() -> MutexGuard<'static, Option<Files>> {
    FILES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `daemon_status_file_save()`: the record's JSON into the primary directory or a fallback, logged when `log`.
fn save(f: &Files, log: bool) {
    io::save(&f.loc, STATUS_FILENAME, &to_json(&f.session), log);
}

/// `daemon_status_file_update_status()` under the lock: a refresh, then a logged save.
fn update(f: &mut Files, status: DaemonStatus) {
    live::refresh(&mut f.session, status);
    save(f, true);
}

/// `stack_trace_is_empty()`: nothing, or only an informational text.
fn stack_trace_is_empty(ds: &StatusFile) -> bool {
    ds.fatal.stack_trace.as_bytes().starts_with(INFO_PREFIX.as_bytes()) || ds.fatal.stack_trace.is_empty()
}

/// `daemon_status_file_has_last_crashed()`: the record did not reach "exited", or its reason is not a normal one.
pub fn has_crashed(ds: &StatusFile) -> bool {
    !matches!(ds.status, DaemonStatus::None | DaemonStatus::Exited) || !exit_reason::is_normal(ds.exit_reason)
}

/// `daemon_status_file_init()` and `daemon_status_file_migrate_once()`: the newest saved record of `varlib`, the
/// cache directory or the fallbacks, and this run's record carried over from it. The machine GUID is read here,
/// falling back to the last record's (`machine_guid_get()`), before anything is saved.
pub fn init(varlib: &str, cache: &str, user_config: &str) {
    live::set_dirs(varlib, cache);
    let loc = Locations::new(varlib, cache);
    let mut last = StatusFile::default();
    io::load(&loc, STATUS_FILENAME, true, |path| {
        io::read_text(path, 65536).is_some_and(|text| from_json(&text, &mut last))
    });
    // what older versions of the file did not keep (a missing file is version 0)
    if last.v <= 26 {
        dmi::fill(&mut last);
    }
    if last.v <= 27 {
        last.system_cpus = live::system_cpus();
    }
    // the texts the signal handlers print, from the times the file keeps
    if last.timestamp_ut != 0 {
        last.timestamp_rfc3339 = rfc3339(last.timestamp_ut);
    }
    if last.host_id.last_modified_ut != 0 {
        last.host_id.last_modified_rfc3339 = rfc3339(last.host_id.last_modified_ut);
    }
    if last.disk_footprint.last_updated_ut != 0 {
        last.disk_footprint.last_updated_rfc3339 = rfc3339(last.disk_footprint.last_updated_ut);
    }

    let mut session = StatusFile { v: super::VERSION, ..Default::default() };
    migrate(&mut session, &last, varlib, user_config);
    *files() = Some(Files { loc, last, session });
}

/// `daemon_status_file_migrate_once()`: this run's identity, and what it keeps of the last run.
fn migrate(s: &mut StatusFile, last: &StatusFile, varlib: &str, user_config: &str) {
    s.version.set(build::NETDATA_VERSION);
    s.machine_id = live::machine_id();
    if let Some(install_type) = live::install_type(user_config) {
        s.install_type.set(install_type);
    }
    s.sentry_available = false;
    s.boot_id = live::boot_id();
    if s.boot_id != last.boot_id && live::boot_ids_match(&s.boot_id, &last.boot_id) {
        // a slight difference, still the same boot
        s.boot_id = last.boot_id;
    }
    s.host_id = live::machine_guid(varlib, &last.host_id);
    carry_over(s, last);
    s.stack_traces.set(live::STACK_TRACE_BACKEND);
    dmi::fill(s);
}

/// `daemon_status_file_get_product_*()`: this run's product (empty before `init()`), which localhost's `_hw_*`
/// labels take when the system info is detected.
pub fn product() -> Product {
    files().as_ref().map_or_else(Product::default, |f| f.session.product)
}

/// What this run keeps of the last one: its ids, host strings and counters, one more restart, and the crash or
/// clean-run streak its reliability counts.
fn carry_over(s: &mut StatusFile, last: &StatusFile) {
    s.claim_id = last.claim_id;
    s.node_id = last.node_id;
    s.architecture = last.architecture;
    s.virtualization = last.virtualization;
    s.container = last.container;
    s.kernel_version = last.kernel_version;
    s.os_name = last.os_name;
    s.os_version = last.os_version;
    s.os_id = last.os_id;
    s.os_id_like = last.os_id_like;
    s.timezone = last.timezone;
    s.cloud_provider_type = last.cloud_provider_type;
    s.cloud_instance_type = last.cloud_instance_type;
    s.cloud_instance_region = last.cloud_instance_region;

    s.posts = last.posts;
    s.restarts = last.restarts.wrapping_add(1);
    s.crashes = last.crashes;
    s.reliability = last.reliability;
    // C's signed arithmetic, as its builds wrap
    if has_crashed(last) {
        s.crashes = s.crashes.wrapping_add(1);
        s.reliability = s.reliability.min(0).wrapping_sub(1);
    } else {
        s.reliability = s.reliability.max(0).wrapping_add(1);
    }
}

/// `daemon_status_file_update_status()`: refreshed with `status` (`None` keeps the current one) and saved.
pub fn update_status(status: DaemonStatus) {
    if let Some(f) = files().as_mut() {
        update(f, status);
    }
}

/// `daemon_status_file_startup_step()`: the step (or none) as the record's function, saved as initializing; nothing
/// once a fatal error is recorded.
pub fn startup_step(step: Option<&str>) {
    let mut guard = files();
    let Some(f) = guard.as_mut() else { return };
    if !f.session.fatal.filename.is_empty() {
        return;
    }
    f.session.fatal.function.set(step.unwrap_or_default());
    update(f, DaemonStatus::Initializing);
}

/// `daemon_status_file_shutdown_step()`: `shutdown(<step>)` (or none) as the record's function, the timings so far as
/// its stack trace while it has none, saved as exiting; nothing once a fatal error is recorded or the shutdown timed
/// out.
pub fn shutdown_step(step: Option<&str>, timings: &str) {
    let mut guard = files();
    let Some(f) = guard.as_mut() else { return };
    if !f.session.fatal.filename.is_empty() {
        return;
    }
    let _timeout = match TIMEOUT.try_lock() {
        Ok(held) => held,
        Err(TryLockError::Poisoned(held)) => held.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    let s = &mut f.session;
    match step {
        Some(step) => s.fatal.function.set(format!("shutdown({step})")),
        None => s.fatal.function.set(""),
    }
    if !timings.is_empty() && stack_trace_is_empty(s) {
        s.fatal.stack_trace.set(timings);
    }
    update(f, DaemonStatus::Exiting);
}

/// `daemon_status_file_shutdown_timeout()`: once, the shutdown-timeout reason, the timings as the stack trace, the
/// step in the message and `shutdown_timeout` as the function, saved as they are (no refresh); later steps are
/// not saved.
pub fn shutdown_timeout(step: &str, timings: &str) {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if ONCE.swap(true, Ordering::AcqRel) {
        return;
    }
    // held for good
    std::mem::forget(TIMEOUT.lock().unwrap_or_else(PoisonError::into_inner));
    exit_reason::add(exit_reason::SHUTDOWN_TIMEOUT);
    let mut guard = files();
    let Some(f) = guard.as_mut() else { return };
    timeout_record(&mut f.session, step, timings);
    save(f, false);
}

/// The record of a shutdown that timed out at `step`, with the steps' `timings` so far.
fn timeout_record(s: &mut StatusFile, step: &str, timings: &str) {
    s.exit_reason |= exit_reason::SHUTDOWN_TIMEOUT;
    if !timings.is_empty() && stack_trace_is_empty(s) {
        if let Some(rest) = timings.strip_prefix(SHUTDOWN_TIMINGS_HEADER) {
            s.fatal.stack_trace.set(format!("shutdown timings:{rest}"));
        } else if let Some(rest) = timings.strip_prefix(INFO_PREFIX) {
            s.fatal.stack_trace.set(format!("shutdown timings: {rest}"));
        } else {
            s.fatal.stack_trace.set(timings);
        }
    }
    if !step.is_empty() && s.fatal.message.is_empty() {
        s.fatal.message.set(format!("shutdown timed out at step: {step}"));
    }
    s.fatal.function.set("shutdown_timeout");
}

/// `DSF_REPORT_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashReports {
    Disabled,
    All,
    Crashes,
}

/// `check_crash_reports_config()`: `[global] crash reports`, by default "all" when analytics is on or a node or claim
/// id is known, else "off".
fn crash_reports(conf: &mut Config, analytics: bool, last: &StatusFile, session: &StatusFile) -> CrashReports {
    let known = |id: &[u8; 16]| *id != [0; 16];
    let default_enabled = analytics
        || known(&session.node_id)
        || known(&last.node_id)
        || known(&session.claim_id)
        || known(&last.claim_id);
    let default = if default_enabled { "all" } else { "off" };
    match conf.get(SECTION_GLOBAL, "crash reports", Some(default)).as_deref() {
        None | Some(b"") if default_enabled => CrashReports::All,
        Some(b"all") => CrashReports::All,
        Some(b"crashes") => CrashReports::Crashes,
        _ => CrashReports::Disabled,
    }
}

/// The user's and the report's priorities of a last exit (`struct log_priority`).
type Pri = (Priority, Priority);
const ALL_NORMAL: Pri = (Priority::Notice, Priority::Debug);
const USER_SHOULD_FIX: Pri = (Priority::Warning, Priority::Info);
const FATAL: Pri = (Priority::Err, Priority::Err);
const DEADLY_SIGNAL: Pri = (Priority::Crit, Priority::Crit);
const KILLED_HARD: Pri = (Priority::Err, Priority::Warning);

/// What the last run's record says about how it ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LastExit {
    pub cause: &'static str,
    pub msg: &'static str,
    pub pri: (Priority, Priority),
    pub crash: bool,
    pub no_previous_status: bool,
    pub dump_json: bool,
}

/// `daemon_status_file_check_crash()`'s classification; "exit and updated" adds the update reason to `last`.
pub fn last_exit(last: &mut StatusFile, session: &StatusFile) -> LastExit {
    let new_version = last.version != session.version;
    let reason = last.exit_reason;
    let deadly = reason & exit_reason::DEADLY_SIGNAL != 0;
    let abnormal = reason != 0 && !exit_reason::is_normal(reason);
    let power_off = reason == 0
        && session.boot_id != [0; 16]
        && last.boot_id != [0; 16]
        && !live::boot_ids_match(&session.boot_id, &last.boot_id);
    // OS_SYSTEM_DISK_SPACE_OK()
    let disk = last.var_cache.total_bytes > 0;
    let mut e = LastExit {
        cause: "",
        msg: "",
        pri: ALL_NORMAL,
        crash: false,
        no_previous_status: false,
        dump_json: true,
    };
    let mut set = |cause, msg, pri: Pri, crash| {
        e.cause = cause;
        e.msg = msg;
        e.pri = pri;
        e.crash = crash;
    };
    match last.status {
        DaemonStatus::None => {
            set(
                "no last status",
                "No status found for the previous Netdata session (new Netdata, or older version)",
                ALL_NORMAL,
                false,
            );
            e.no_previous_status = true;
        }
        DaemonStatus::Exited => {
            if reason == 0 {
                set(
                    "exit no reason",
                    "Netdata was last stopped gracefully, without setting a reason",
                    ALL_NORMAL,
                    false,
                );
                e.dump_json = last.timestamp_ut != 0;
            } else if deadly {
                set(
                    "deadly signal and exit",
                    "Netdata was last stopped gracefully after receiving a deadly signal",
                    DEADLY_SIGNAL,
                    true,
                );
            } else if abnormal {
                set(
                    "fatal and exit",
                    "Netdata was last stopped gracefully after it encountered a fatal error",
                    FATAL,
                    true,
                );
            } else if reason & exit_reason::SYSTEM_SHUTDOWN != 0 {
                set(
                    "exit on system shutdown",
                    "Netdata has gracefully stopped due to system shutdown",
                    ALL_NORMAL,
                    false,
                );
            } else if reason & exit_reason::UPDATE != 0 {
                set("exit to update", "Netdata has gracefully restarted to update to a new version", ALL_NORMAL, false);
            } else if new_version {
                set(
                    "exit and updated",
                    "Netdata has gracefully restarted and updated to a new version",
                    ALL_NORMAL,
                    false,
                );
                last.exit_reason |= exit_reason::UPDATE;
            } else {
                set("exit instructed", "Netdata was last stopped gracefully", ALL_NORMAL, false);
            }
        }
        DaemonStatus::Initializing => {
            if power_off {
                set(
                    "abnormal power off",
                    "The system was abnormally powered off while Netdata was starting",
                    USER_SHOULD_FIX,
                    true,
                );
            } else if deadly {
                set(
                    "deadly signal on start",
                    "Netdata was last crashed while starting after receiving a deadly signal",
                    DEADLY_SIGNAL,
                    true,
                );
            } else if reason & exit_reason::OUT_OF_MEMORY != 0 {
                set(
                    "out of memory",
                    "Netdata was last crashed while starting, because it couldn't allocate memory",
                    USER_SHOULD_FIX,
                    true,
                );
            } else if reason & exit_reason::ALREADY_RUNNING != 0 {
                set("already running", "Netdata couldn't start, because it was already running", USER_SHOULD_FIX, true);
            } else if disk && last.var_cache.read_only {
                set("disk read-only", "Netdata couldn't start because the disk is readonly", USER_SHOULD_FIX, true);
            } else if disk && last.var_cache.free_bytes == 0 {
                set("disk full", "Netdata couldn't start because the disk is full", USER_SHOULD_FIX, true);
            } else if disk && last.var_cache.free_bytes < 10 * 1024 * 1024 {
                set("disk almost full", "Netdata couldn't start while the disk is almost full", USER_SHOULD_FIX, true);
            } else if abnormal {
                set("fatal on start", "Netdata was last crashed while starting, because of a fatal error", FATAL, true);
            } else {
                set("killed hard on start", "Netdata was last killed/crashed while starting", KILLED_HARD, true);
            }
        }
        DaemonStatus::Exiting => {
            if deadly {
                set(
                    "deadly signal on exit",
                    "Netdata was last crashed while exiting after receiving a deadly signal",
                    DEADLY_SIGNAL,
                    true,
                );
            } else if reason & exit_reason::SHUTDOWN_TIMEOUT != 0 {
                set("exit timeout", "Netdata was last killed because it couldn't shutdown on time", FATAL, true);
            } else if abnormal {
                set(
                    "fatal on exit",
                    "Netdata was last killed/crashed while exiting after encountering an error",
                    FATAL,
                    true,
                );
            } else if reason & exit_reason::SYSTEM_SHUTDOWN != 0 {
                set(
                    "killed hard on shutdown",
                    "Netdata was last killed/crashed while exiting due to system shutdown",
                    KILLED_HARD,
                    true,
                );
            } else if new_version || reason & exit_reason::UPDATE != 0 {
                set(
                    "killed hard on update",
                    "Netdata was last killed/crashed while exiting to update to a new version",
                    KILLED_HARD,
                    true,
                );
            } else {
                set(
                    "killed hard on exit",
                    "Netdata was last killed/crashed while it was instructed to exit",
                    KILLED_HARD,
                    true,
                );
            }
        }
        DaemonStatus::Running => {
            if power_off {
                set(
                    "abnormal power off",
                    "The system was abnormally powered off while Netdata was running",
                    USER_SHOULD_FIX,
                    false,
                );
            } else if reason & exit_reason::OUT_OF_MEMORY != 0 {
                set(
                    "out of memory",
                    "Netdata was last crashed because it couldn't allocate memory",
                    USER_SHOULD_FIX,
                    false,
                );
            } else if deadly {
                set("deadly signal", "Netdata was last crashed after receiving a deadly signal", DEADLY_SIGNAL, true);
            } else if abnormal {
                set("killed fatal", "Netdata was last crashed due to a fatal error", FATAL, false);
            } else if last.memory.total > 0 && last.memory.available <= last.oom_protection {
                set(
                    "killed hard low ram",
                    "Netdata was last killed/crashed while available memory was critically low",
                    KILLED_HARD,
                    true,
                );
            } else {
                set("killed hard", "Netdata was last killed/crashed while operating normally", KILLED_HARD, true);
            }
        }
    }
    e
}

/// `daemon_status_file_check_crash()`: the "Last exit status" record with the last run's record, saved as the
/// "crash reports check" step; `[global] crash reports` is read (the report itself is not ported).
pub fn check_crash(conf: &mut Config, analytics: bool) {
    let (exit, dump) = {
        let mut guard = files();
        let Some(f) = guard.as_mut() else { return };
        let exit = last_exit(&mut f.last, &f.session);
        let dump = if exit.dump_json {
            to_json(&f.last)
        } else {
            let mut w = netdata_agent_text::json::JsonWriter::new(netdata_agent_text::json::JsonOptions::DEFAULT);
            w.finalize();
            w.into_bytes()
        };
        (exit, dump)
    };
    {
        let _frame = push(vec![(Field::MessageId, Value::Uuid(msgid::STARTUP))]);
        nd_log!(
            Source::Daemon,
            exit.pri.0,
            "Netdata Agent version '{}' is starting...\nLast exit status: {} ({}):\n\n{}",
            build::NETDATA_VERSION,
            exit.msg,
            exit.cause,
            String::from_utf8_lossy(&dump)
        );
    }
    startup_step(Some("startup(crash reports check)"));
    let guard = files();
    if let Some(f) = guard.as_ref() {
        let (last, session) = (f.last, f.session);
        drop(guard);
        let _ = crash_reports(conf, analytics, &last, &session);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{DiskSpace, FixedStr, Memory};
    use super::*;

    fn last(status: DaemonStatus, exit_reason: u32) -> StatusFile {
        let mut ds = StatusFile { status, exit_reason, timestamp_ut: 1, ..Default::default() };
        ds.version.set(build::NETDATA_VERSION);
        ds
    }

    /// Every branch of `daemon_status_file_check_crash()`, in C's order of precedence.
    #[test]
    fn classifies_the_last_exit_as_c() {
        use DaemonStatus::*;
        use exit_reason::*;
        let session = StatusFile { boot_id: [2; 16], ..last(Running, 0) };
        let other_boot = |mut ds: StatusFile| {
            ds.boot_id = [1; 16];
            ds
        };
        let disk =
            |free, read_only| DiskSpace { total_bytes: 1 << 40, free_bytes: free, read_only, ..Default::default() };
        let cases: Vec<(StatusFile, &str, bool)> = vec![
            (last(None, 0), "no last status", false),
            (last(Exited, 0), "exit no reason", false),
            (last(Exited, SIGSEGV | SIGTERM), "deadly signal and exit", true),
            (last(Exited, FATAL), "fatal and exit", true),
            (last(Exited, SIGTERM | SYSTEM_SHUTDOWN), "exit on system shutdown", false),
            (last(Exited, SIGTERM | UPDATE), "exit to update", false),
            (StatusFile { version: FixedStr::from("v0"), ..last(Exited, SIGTERM) }, "exit and updated", false),
            (last(Exited, SIGTERM), "exit instructed", false),
            (other_boot(last(Initializing, 0)), "abnormal power off", true),
            (last(Initializing, SIGBUS), "deadly signal on start", true),
            (last(Initializing, OUT_OF_MEMORY), "out of memory", true),
            (last(Initializing, ALREADY_RUNNING), "already running", true),
            (StatusFile { var_cache: disk(1 << 30, true), ..last(Initializing, 0) }, "disk read-only", true),
            (StatusFile { var_cache: disk(0, false), ..last(Initializing, 0) }, "disk full", true),
            (StatusFile { var_cache: disk(1 << 20, false), ..last(Initializing, 0) }, "disk almost full", true),
            (last(Initializing, FATAL), "fatal on start", true),
            (last(Initializing, 0), "killed hard on start", true),
            (last(Exiting, SIGILL | SHUTDOWN_TIMEOUT), "deadly signal on exit", true),
            (last(Exiting, SHUTDOWN_TIMEOUT), "exit timeout", true),
            (last(Exiting, FATAL), "fatal on exit", true),
            (last(Exiting, SIGTERM | SYSTEM_SHUTDOWN), "killed hard on shutdown", true),
            (last(Exiting, SIGTERM | UPDATE), "killed hard on update", true),
            (last(Exiting, SIGTERM), "killed hard on exit", true),
            (other_boot(last(Running, 0)), "abnormal power off", false),
            (last(Running, OUT_OF_MEMORY), "out of memory", false),
            (last(Running, SIGFPE), "deadly signal", true),
            (last(Running, FATAL), "killed fatal", false),
            (
                StatusFile {
                    memory: Memory { total: 8 << 30, available: 1 << 20 },
                    oom_protection: 1 << 30,
                    ..last(Running, 0)
                },
                "killed hard low ram",
                true,
            ),
            (last(Running, 0), "killed hard", true),
        ];
        for (mut ds, cause, crash) in cases {
            let e = last_exit(&mut ds, &session);
            assert_eq!((e.cause, e.crash), (cause, crash), "{cause}");
        }
        // the only one that writes back: a new version adds the update reason to the dump
        let mut ds = StatusFile { version: FixedStr::from("v0"), ..last(Exited, SIGTERM) };
        last_exit(&mut ds, &session);
        assert_eq!(ds.exit_reason, SIGTERM | UPDATE);
        // no dump for a record that never saved a time
        let mut ds = StatusFile { timestamp_ut: 0, ..last(Exited, 0) };
        assert!(!last_exit(&mut ds, &session).dump_json);
    }

    #[test]
    fn carries_the_counters_over_as_c() {
        let run = |status, exit_reason, reliability| {
            let last = StatusFile { restarts: 4, crashes: 2, posts: 3, reliability, ..last(status, exit_reason) };
            let mut s = StatusFile::default();
            carry_over(&mut s, &last);
            (s.restarts, s.crashes, s.posts, s.reliability)
        };
        assert_eq!(run(DaemonStatus::Exited, exit_reason::SIGTERM, 5), (5, 2, 3, 6));
        assert_eq!(run(DaemonStatus::Exited, exit_reason::SIGTERM, -3), (5, 2, 3, 1));
        assert_eq!(run(DaemonStatus::Running, 0, 5), (5, 3, 3, -1));
        assert_eq!(run(DaemonStatus::Exited, exit_reason::FATAL, -3), (5, 3, 3, -4));
        assert_eq!(run(DaemonStatus::None, 0, 0), (5, 2, 3, 1));
    }

    #[test]
    fn a_timeout_keeps_the_steps_so_far() {
        let mut s = StatusFile::default();
        timeout_record(&mut s, "stop replication threads", &format!("{SHUTDOWN_TIMINGS_HEADER}\n#1 'a': 5us"));
        assert_eq!(s.fatal.stack_trace.as_bytes(), b"shutdown timings:\n#1 'a': 5us");
        assert_eq!(s.fatal.message.as_bytes(), b"shutdown timed out at step: stop replication threads");
        assert_eq!(s.fatal.function.as_bytes(), b"shutdown_timeout");
        assert_eq!(s.exit_reason, exit_reason::SHUTDOWN_TIMEOUT);
        // a stack trace that is more than information, and a message, stay
        let mut s = StatusFile::default();
        s.fatal.stack_trace.set("#0 main");
        s.fatal.message.set("earlier");
        timeout_record(&mut s, "x", "info: other");
        assert_eq!((s.fatal.stack_trace.as_bytes(), s.fatal.message.as_bytes()), (&b"#0 main"[..], &b"earlier"[..]));
        let mut s = StatusFile::default();
        timeout_record(&mut s, "", "info: other");
        let got = (s.fatal.stack_trace.as_bytes(), s.fatal.message.as_bytes());
        assert_eq!(got, (&b"shutdown timings: other"[..], &b""[..]));
    }
}
