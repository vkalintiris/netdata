//! The agent's logger, a port of `nd_log` (`src/libnetdata/log/`): six sources (daemon, collector, access, health,
//! aclk, debug), eight syslog priorities, 65 numbered fields written in logfmt, json or the journal's native protocol,
//! and per-thread frames of fields that every record logged under them carries (`ND_LOG_STACK`).
//!
//! The byte contract is `knowledge/spec-logging.md` in the status repository; decisions D27, D31 and D32 record the
//! choices made where C's behaviour cannot or should not be copied.

#![forbid(unsafe_code)]

mod config;
mod encode;
mod frame;
mod limit;
mod model;
mod output;

use std::cell::{Cell, RefCell};
use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

pub use config::{
    chown_log_files, chown_open_file, init_invocation_id, initialize, invocation_id, limits_reset,
    limits_unlimited, reopen_log_files, set_facility, set_flood_protection, set_host_prefix,
    set_priority_level, set_user_settings,
};
pub use encode::strerror;
pub use frame::{FrameGuard, Lazy, Value, push, push_shared};
pub use limit::{DEFAULT_THROTTLE_LOGS, DEFAULT_THROTTLE_PERIOD, ErrorLimit};
pub use model::{Field, Priority, Source, msgid};
pub use output::{is_stderr_connected_to_journal, set_default_log_dir};

/// What records print in place of a stream API key (D31, D34): in URLs, the receiver's records and the host record.
pub const REDACTED: &str = "[REDACTED]";

use encode::{Record, Slot};
use model::Format;
use output::{G, Target};

/// Where a record was logged from (`__FILE__`, `__LINE__`, `__FUNCTION__`); the journal's `CODE_*` fields.
#[derive(Clone, Copy)]
pub struct Location {
    pub file: &'static str,
    pub line: u32,
    /// Resolved only when a journal record needs it.
    pub function: fn() -> &'static str,
}

/// The call site's [`Location`].
#[macro_export]
macro_rules! here {
    () => {
        $crate::Location {
            file: ::core::file!(),
            line: ::core::line!(),
            function: {
                fn __function() -> &'static str {
                    fn f() {}
                    fn type_name_of<T>(_: T) -> &'static str {
                        ::core::any::type_name::<T>()
                    }
                    $crate::function_name(type_name_of(f))
                }
                __function
            },
        }
    };
}

/// The function a [`here!`] expansion is in, from the type name of an item nested in it.
#[doc(hidden)]
pub fn function_name(type_name: &'static str) -> &'static str {
    let path = type_name
        .strip_suffix("::__function::f")
        .unwrap_or(type_name);
    let mut path = path;
    while let Some(outer) = path.strip_suffix("::{{closure}}") {
        path = outer;
    }
    path.rsplit("::").next().unwrap_or(path)
}

/// `nd_log(source, priority, fmt, ...)`; `errno = <i32>;` before the format adds the `errno` field.
#[macro_export]
macro_rules! nd_log {
    ($source:expr, $priority:expr, errno = $errno:expr; $($arg:tt)+) => {
        $crate::logger($source, $priority, $errno, &$crate::here!(), ::core::option::Option::Some(::core::format_args!($($arg)+)))
    };
    ($source:expr, $priority:expr, $($arg:tt)+) => {
        $crate::logger($source, $priority, 0, &$crate::here!(), ::core::option::Option::Some(::core::format_args!($($arg)+)))
    };
}

/// `nd_log_daemon(priority, ...)`.
#[macro_export]
macro_rules! nd_log_daemon {
    ($priority:expr, $($arg:tt)+) => { $crate::nd_log!($crate::Source::Daemon, $priority, $($arg)+) };
}

/// `nd_log_collector(priority, ...)`.
#[macro_export]
macro_rules! nd_log_collector {
    ($priority:expr, $($arg:tt)+) => { $crate::nd_log!($crate::Source::Collector, $priority, $($arg)+) };
}

/// `netdata_log_info(...)`: daemon, info.
#[macro_export]
macro_rules! netdata_log_info {
    ($($arg:tt)+) => { $crate::nd_log!($crate::Source::Daemon, $crate::Priority::Info, $($arg)+) };
}

/// `netdata_log_error(...)`: daemon, error.
#[macro_export]
macro_rules! netdata_log_error {
    ($($arg:tt)+) => { $crate::nd_log!($crate::Source::Daemon, $crate::Priority::Err, $($arg)+) };
}

/// `collector_info(...)`: collector, info.
#[macro_export]
macro_rules! collector_info {
    ($($arg:tt)+) => { $crate::nd_log!($crate::Source::Collector, $crate::Priority::Info, $($arg)+) };
}

/// `collector_error(...)`: collector, error.
#[macro_export]
macro_rules! collector_error {
    ($($arg:tt)+) => { $crate::nd_log!($crate::Source::Collector, $crate::Priority::Err, $($arg)+) };
}

/// `nd_log_limit(&erl, source, priority, ...)`: at most once per the limiter's period.
#[macro_export]
macro_rules! nd_log_limit {
    ($limit:expr, $source:expr, $priority:expr, errno = $errno:expr; $($arg:tt)+) => {
        $crate::logger_with_limit($limit, $source, $priority, $errno, &$crate::here!(), ::core::format_args!($($arg)+))
    };
    ($limit:expr, $source:expr, $priority:expr, $($arg:tt)+) => {
        $crate::logger_with_limit($limit, $source, $priority, 0, &$crate::here!(), ::core::format_args!($($arg)+))
    };
}

/// `fatal(...)`: logs at alert with the fatal `MESSAGE_ID`, then exits with 1. Never returns.
#[macro_export]
macro_rules! fatal {
    (errno = $errno:expr; $($arg:tt)+) => { $crate::fatal($errno, &$crate::here!(), ::core::format_args!($($arg)+)) };
    ($($arg:tt)+) => { $crate::fatal(0, &$crate::here!(), ::core::format_args!($($arg)+)) };
}

static PROGRAM_NAME: OnceLock<&'static str> = OnceLock::new();

/// `program_name`, the `comm=` / `SYSLOG_IDENTIFIER=` of every record; empty until set.
pub fn set_program_name(name: &'static str) {
    let _ = PROGRAM_NAME.set(name);
}

fn program_name() -> &'static str {
    PROGRAM_NAME.get().copied().unwrap_or("")
}

thread_local! {
    static TID: Cell<u64> = const { Cell::new(0) };
    static CAPTURE: RefCell<Option<Vec<Captured>>> = const { RefCell::new(None) };
    static IN_FATAL: Cell<bool> = const { Cell::new(false) };
}

/// `gettid_cached()`.
fn tid() -> u64 {
    TID.with(|tid| {
        if tid.get() == 0 {
            tid.set(nix::unistd::gettid().as_raw() as u64);
        }
        tid.get()
    })
}

/// `gettid_uncached()` after a `fork()`: the child's thread inherited the parent's cached id.
pub fn forked() {
    TID.with(|tid| tid.set(0));
}

/// `now_realtime_usec()`.
fn now_realtime_usec() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_micros() as u64)
}

/// A record a test captured instead of writing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub source: Source,
    pub priority: Priority,
    pub errno: i32,
    pub message: Option<String>,
}

/// Runs `f` with this thread's records captured instead of written (any priority, any source).
pub fn capture<R>(f: impl FnOnce() -> R) -> (R, Vec<Captured>) {
    let previous = CAPTURE.with(|c| c.borrow_mut().replace(Vec::new()));
    let result = f();
    let captured = CAPTURE.with(|c| std::mem::replace(&mut *c.borrow_mut(), previous));
    (result, captured.unwrap_or_default())
}

fn captured(
    source: Source,
    priority: Priority,
    errno: i32,
    message: Option<fmt::Arguments<'_>>,
) -> bool {
    if CAPTURE.with(|c| c.borrow().is_none()) {
        return false;
    }
    // formatted before the sink is borrowed: a Display that logs must not find it borrowed
    let record = Captured {
        source,
        priority,
        errno,
        message: message.map(|m| m.to_string()),
    };
    CAPTURE.with(|c| {
        if let Some(records) = c.borrow_mut().as_mut() {
            records.push(record);
        }
    });
    true
}

/// `netdata_logger()`: filtered by the source's minimum priority (except debug); daemon and collector records count
/// against flood protection. `errno` 0 adds no `errno` field; `message` `None` writes a record without `msg`.
pub fn logger(
    source: Source,
    priority: Priority,
    errno: i32,
    location: &Location,
    message: Option<fmt::Arguments<'_>>,
) {
    if captured(source, priority, errno, message) || output::filtered(source, priority) {
        return;
    }
    let limit = matches!(source, Source::Daemon | Source::Collector);
    log_record(source, priority, limit, errno, location, message);
}

/// `netdata_logger_with_limit()`.
pub fn logger_with_limit(
    limit: &ErrorLimit,
    source: Source,
    priority: Priority,
    errno: i32,
    location: &Location,
    message: fmt::Arguments<'_>,
) {
    if captured(source, priority, errno, Some(message)) || output::filtered(source, priority) {
        return;
    }
    let Some(now) = limit.admit() else {
        return;
    };
    let flood = matches!(source, Source::Daemon | Source::Collector);
    log_record(source, priority, flood, errno, location, Some(message));
    limit.logged(now);
}

/// `nd_logger()`.
fn log_record(
    source: Source,
    priority: Priority,
    limit: bool,
    errno: i32,
    location: &Location,
    message: Option<fmt::Arguments<'_>>,
) {
    let (target, format) = output::select(source);
    if matches!(target, Target::Disabled) {
        return;
    }
    let message = message.map(|m| m.to_string());
    let invocation = invocation_id();
    let thread = std::thread::current();
    // the main thread has no tag in C
    let tag = match thread.name() {
        Some("main") | None => "",
        Some(name) => name,
    };

    let routed = frame::with_fields(|frames| {
        let mut record = Record::new();
        for (slot, value) in record.slots.iter_mut().zip(frames) {
            *slot = value.map(Slot::from);
        }
        let mut source = source;
        let (mut target, mut format) = (target, format);
        let slots = &mut record.slots;
        fn set(slots: &[Option<Slot<'_>>], field: Field) -> bool {
            slots[field as usize].is_none()
        }

        if set(slots, Field::InvocationId) {
            slots[Field::InvocationId as usize] = Some(Slot::Uuid(&invocation));
        }
        match slots[Field::LogSource as usize] {
            None => slots[Field::LogSource as usize] = Some(Slot::Txt(source.name())),
            Some(slot) => {
                // a frame can re-route the record to another source
                let routed = match slot {
                    Slot::Txt(name) => Source::parse(name, source),
                    // C re-routes only to a valid source
                    Slot::U64(id) => Source::from_id(id).unwrap_or(source),
                    _ => source,
                };
                if routed != source {
                    source = routed;
                    (target, format) = output::select(source);
                    if matches!(target, Target::Disabled) {
                        return None;
                    }
                }
            }
        }
        if set(slots, Field::SyslogIdentifier) {
            slots[Field::SyslogIdentifier as usize] = Some(Slot::Txt(program_name()));
        }
        if set(slots, Field::Line) {
            slots[Field::Line as usize] = Some(Slot::U64(u64::from(location.line)));
            slots[Field::File as usize] = Some(Slot::Txt(location.file));
            slots[Field::Func as usize] = Some(Slot::Txt((location.function)()));
        }
        if set(slots, Field::Priority) {
            slots[Field::Priority as usize] = Some(Slot::U64(priority as u64));
        }
        if set(slots, Field::Tid) {
            slots[Field::Tid as usize] = Some(Slot::U64(tid()));
        }
        if set(slots, Field::ThreadTag) {
            slots[Field::ThreadTag as usize] = Some(Slot::Txt(tag));
        }
        if set(slots, Field::TimestampRealtimeUsec) {
            slots[Field::TimestampRealtimeUsec as usize] = Some(Slot::U64(now_realtime_usec()));
        }
        if errno != 0 && set(slots, Field::Errno) {
            slots[Field::Errno as usize] = Some(Slot::I64(i64::from(errno)));
        }
        if let Some(message) = &message {
            if set(slots, Field::Message) {
                slots[Field::Message as usize] = Some(Slot::Txt(message.as_str()));
            }
        }
        write_record(&target, format, source, priority, limit, &record);
        Some((source, target, format))
    });
    let Some((source, target, format)) = routed else {
        return;
    };

    // The flood-protection message goes right after the record that triggered it; if another thread holds the
    // limits, a later record writes it.
    let pending = G.limits[source as usize]
        .try_lock()
        .ok()
        .and_then(|mut l| l.pending.take());
    if let Some(pending) = pending {
        let mut record = Record::new();
        let msgid = limit::PENDING_MSGID;
        record.slots[Field::TimestampRealtimeUsec as usize] = Some(Slot::U64(now_realtime_usec()));
        record.slots[Field::LogSource as usize] = Some(Slot::Txt(source.name()));
        record.slots[Field::SyslogIdentifier as usize] = Some(Slot::Txt(program_name()));
        record.slots[Field::Message as usize] = Some(Slot::Txt(&pending));
        record.slots[Field::MessageId as usize] = Some(Slot::Uuid(&msgid));
        write_record(&target, format, source, priority, false, &record);
    }
}

/// `nd_logger_log_fields()`.
fn write_record(
    target: &Target,
    format: Format,
    source: Source,
    priority: Priority,
    limit: bool,
    record: &Record<'_>,
) {
    if limit
        && output::lock(&G.limits[source as usize])
            .reached(limit::now_monotonic_usec(), program_name())
    {
        return;
    }
    let text_line = |format: Format| {
        let mut line = Vec::with_capacity(256);
        match format {
            Format::Json => encode::json(record, &mut line),
            Format::Logfmt | Format::Journal => encode::logfmt(record, &mut line),
        }
        line.push(b'\n');
        line
    };
    match target {
        Target::Disabled => {}
        Target::Journal => {
            let mut datagram = Vec::with_capacity(512);
            encode::journal(record, &mut datagram);
            if !output::journal_send(&datagram) {
                // the journal is gone: this record goes to stderr
                output::write_stderr(&text_line(format));
            }
        }
        Target::Syslog => {
            // always logfmt, no newline
            let mut line = Vec::with_capacity(256);
            encode::logfmt(record, &mut line);
            output::syslog_send(priority, &line, program_name());
        }
        Target::Fd { fd, lock } => {
            output::write_fd(fd, *lock, &text_line(format));
        }
    }
}

/// `fatal()` (`netdata_logger_fatal()`): logs at alert with the fatal `MESSAGE_ID` (ignoring the minimum priority,
/// subject to flood protection), runs the final callback and exits with 1. A fatal inside a fatal, on the same
/// thread or on another one, writes C's raw line to fd 2 and exits at once.
pub fn fatal(errno: i32, location: &Location, message: fmt::Arguments<'_>) -> ! {
    static THREADS_IN_FATAL: AtomicUsize = AtomicUsize::new(0);
    let function = (location.function)();
    if IN_FATAL.with(|f| f.replace(true)) {
        output::write_stderr_raw(
            format!(
                "\nRECURSIVE FATAL STATEMENTS, latest from {function}() of {}@{}, EXITING NOW! \
                 23e93dfccbf64e11aac858b9410d8a82\n",
                location.line, location.file
            )
            .as_bytes(),
        );
        std::process::exit(1);
    }
    if THREADS_IN_FATAL.fetch_add(1, Ordering::SeqCst) + 1 > 1 {
        output::write_stderr_raw(
            format!(
                "\nCONCURRENT FATAL from {function}() of {}@{}, deferring to the first fatal and exiting.\n",
                location.line, location.file
            )
            .as_bytes(),
        );
        std::thread::sleep(std::time::Duration::from_secs(2));
        std::process::exit(1);
    }
    if !captured(Source::Daemon, Priority::Alert, errno, Some(message)) {
        let _msgid = push(vec![(Field::MessageId, Value::Uuid(msgid::FATAL))]);
        log_record(
            Source::Daemon,
            Priority::Alert,
            true,
            errno,
            location,
            Some(message),
        );
    }
    if let Some(callback) = FATAL_FINAL.get() {
        callback();
    }
    std::process::exit(1);
}

static FATAL_FINAL: OnceLock<fn()> = OnceLock::new();

/// `nd_log_register_fatal_final_cb()`: what `fatal()` runs before exiting.
pub fn register_fatal_final_callback(callback: fn()) {
    let _ = FATAL_FINAL.set(callback);
}

/// The `errno` of an I/O error, 0 when it has none.
pub fn errno_of(err: &std::io::Error) -> i32 {
    err.raw_os_error().unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn here_names_the_enclosing_function() {
        fn outer() -> &'static str {
            (here!().function)()
        }
        assert_eq!(outer(), "outer");
        let closure = || (here!().function)();
        assert_eq!(closure(), "here_names_the_enclosing_function");
    }

    #[test]
    fn capture_keeps_records_away_from_the_outputs() {
        let ((), records) = capture(|| {
            netdata_log_error!(errno = 2; "cannot open '{}'", "x");
            nd_log!(Source::Access, Priority::Debug, "hidden by default");
        });
        assert_eq!(
            records,
            [
                Captured {
                    source: Source::Daemon,
                    priority: Priority::Err,
                    errno: 2,
                    message: Some("cannot open 'x'".to_string()),
                },
                Captured {
                    source: Source::Access,
                    priority: Priority::Debug,
                    errno: 0,
                    message: Some("hidden by default".to_string()),
                },
            ]
        );
    }
}
