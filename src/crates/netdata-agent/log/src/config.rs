//! Configuring the sources (`nd_log-config.c`), opening them (`nd_log_initialize()`, `nd_log_reopen_log_files()`),
//! the flood-protection switches (`nd_log_limits_reset()`, `nd_log_limits_unlimited()`) and the invocation id.

use std::sync::RwLock;
use std::sync::atomic::Ordering;

use netdata_agent_text::duration::duration_parse_seconds;
use netdata_agent_text::parse::{str2u, uuid_parse_flexi};
use netdata_agent_text::print::print_uuid_lower_compact;

use crate::limit::{DEFAULT_THROTTLE_PERIOD, Limits, now_monotonic_usec};
use crate::model::{Format, Method, Priority, Source, facility_name, facility_parse};
use crate::output::{
    AfterOpen, G, OpenErrors, SourceState, journal_direct_init, lock, open_resolved, stdin_init,
    sync_priorities, syslog_init, write,
};

type Env = Vec<(&'static str, String)>;

/// `nd_setenv(key, value, 1)`. Refused once other threads run; every export happens during startup.
fn export(env: Env) {
    for (key, value) in env {
        let _ = netdata_agent_sys::setenv(key, &value);
    }
}

fn log_errors(errors: OpenErrors) {
    for (message, errno) in errors {
        crate::logger(
            Source::Daemon,
            Priority::Err,
            errno,
            &crate::here!(),
            Some(format_args!("{message}")),
        );
    }
}

/// `strsep_skip_consecutive_separators()`: the next token that is not empty, or `""` at the end.
fn strsep_skip<'a>(rest: &mut Option<&'a str>, separator: char) -> &'a str {
    let mut token = "";
    while token.is_empty() {
        let Some(text) = *rest else {
            break;
        };
        match text.find(separator) {
            Some(at) => {
                token = &text[..at];
                *rest = Some(&text[at + 1..]);
            }
            None => {
                token = text;
                *rest = None;
            }
        }
    }
    token
}

/// `nd_log_set_user_settings()` on one source: `[option[,option...]@]output`, split at the last `@`. Returns the
/// environment the collector source exports for plugins.
fn apply_user_settings(
    source: Source,
    e: &mut SourceState,
    limits: &mut Limits,
    setting: &str,
    errors: &mut OpenErrors,
) -> Env {
    let (options, output) = match setting.rfind('@') {
        Some(at) => (Some(&setting[..at]), &setting[at + 1..]),
        None => (None, setting),
    };
    let mut rest = options;
    while rest.is_some() {
        let item = strsep_skip(&mut rest, ',');
        if item.is_empty() {
            continue;
        }
        let mut value = Some(item);
        let name = strsep_skip(&mut value, '=');
        if name.is_empty() {
            continue;
        }
        let value = value.filter(|v| !v.is_empty());
        match (name, value) {
            ("logfmt", _) => e.format = Format::Logfmt,
            ("json", _) => e.format = Format::Json,
            ("journal", _) => e.format = Format::Journal,
            ("level", Some(value)) => e.min_priority = Priority::parse(value),
            ("protection", Some("off" | "none")) => limits.replace(Limits::unlimited()),
            ("protection", Some(value)) => {
                let mut l = Limits::default_limits();
                match value.split_once('/') {
                    Some((logs, period)) => {
                        l.logs_per_period = str2u(logs.as_bytes());
                        l.logs_per_period_backup = l.logs_per_period;
                        l.throttle_period = match duration_parse_seconds(period.as_bytes()) {
                            Some(seconds) => seconds as u32,
                            None => {
                                errors.push((format!("Error while parsing period '{period}'"), 0));
                                DEFAULT_THROTTLE_PERIOD
                            }
                        };
                    }
                    None => {
                        l.logs_per_period = str2u(value.as_bytes());
                        l.logs_per_period_backup = l.logs_per_period;
                        l.throttle_period = DEFAULT_THROTTLE_PERIOD;
                    }
                }
                limits.replace(l);
            }
            _ => errors.push((
                format!(
                    "Error while parsing configuration of log source '{}'. In config '{setting}', '{name}' is not \
                     understood.",
                    source.name()
                ),
                0,
            )),
        }
    }

    let (method, filename) = match output {
        "" | "none" | "off" => (Method::Disabled, Some("/dev/null")),
        "journal" => (Method::Journal, None),
        "syslog" => (Method::Syslog, None),
        "/dev/null" => (Method::DevNull, Some("/dev/null")),
        "system" if matches!(e.fd, crate::output::Fd::Stderr) => (Method::Stderr, None),
        "system" | "stdout" => (Method::Stdout, None),
        "stderr" => (Method::Stderr, None),
        path => (Method::File, Some(path)),
    };
    e.method = method;
    e.filename = filename.map(str::to_string);
    match method {
        Method::Stderr => e.fd = crate::output::Fd::Stderr,
        Method::Stdout => e.fd = crate::output::Fd::Stdout,
        _ => {}
    }

    let mut env = Env::new();
    if source == Source::Collector {
        // what the plugins we spawn log with
        let (method, format) = if e.method.valid_for_external_plugins() {
            (e.method, e.format)
        } else {
            (Method::Stderr, Format::Logfmt)
        };
        env.push(("NETDATA_LOG_METHOD", method.name().to_string()));
        env.push(("NETDATA_LOG_FORMAT", format.name().to_string()));
        env.push(("NETDATA_LOG_LEVEL", e.min_priority.name().to_string()));
    }
    env
}

/// `nd_log_set_user_settings()`.
pub fn set_user_settings(source: Source, setting: &str) {
    let mut errors = OpenErrors::new();
    let env = {
        let mut sources = write(&G.sources);
        let mut limits = lock(&G.limits[source as usize]);
        let env = apply_user_settings(
            source,
            &mut sources[source as usize],
            &mut limits,
            setting,
            &mut errors,
        );
        sync_priorities(&sources);
        env
    };
    log_errors(errors);
    export(env);
}

/// `nd_log_set_priority_level()`: every source but debug; empty is info.
pub fn set_priority_level(setting: &str) {
    let priority = Priority::parse(if setting.is_empty() { "info" } else { setting });
    {
        let mut sources = write(&G.sources);
        for (i, e) in sources.iter_mut().enumerate() {
            if i != Source::Debug as usize {
                e.min_priority = priority;
            }
        }
        sync_priorities(&sources);
    }
    export(vec![("NETDATA_LOG_LEVEL", priority.name().to_string())]);
}

/// `nd_log_set_facility()`: empty is daemon.
pub fn set_facility(facility: &str) {
    let facility = facility_parse(if facility.is_empty() {
        "daemon"
    } else {
        facility
    });
    G.facility.store(facility, Ordering::Relaxed);
    export(vec![(
        "NETDATA_SYSLOG_FACILITY",
        facility_name(facility).to_string(),
    )]);
}

/// `nd_log_set_flood_protection(size_t logs, time_t period)` for the daemon and collector sources.
pub fn set_flood_protection(logs: u64, period: i64) {
    for source in [Source::Daemon, Source::Collector] {
        let mut l = lock(&G.limits[source as usize]);
        l.logs_per_period = logs as u32;
        l.logs_per_period_backup = logs as u32;
        l.throttle_period = period as u32;
    }
    export(vec![
        (
            "NETDATA_ERRORS_THROTTLE_PERIOD",
            (period as u64).to_string(),
        ),
        ("NETDATA_ERRORS_PER_PERIOD", logs.to_string()),
    ]);
}

/// `nd_log_limits_reset()`: a new period for every source, with its configured logs per period.
pub fn limits_reset() {
    let now = now_monotonic_usec();
    for limits in &G.limits {
        lock(limits).reset(now);
    }
}

/// `nd_log_limits_unlimited()`: no flood protection until the next reset.
pub fn limits_unlimited() {
    limits_reset();
    for limits in &G.limits {
        lock(limits).logs_per_period = 0;
    }
}

/// `nd_log_open()`.
fn open_source(source: Source) {
    let mut settings_errors = OpenErrors::new();
    let mut open_errors = OpenErrors::new();
    let mut env = Env::new();
    let after = {
        let mut sources = write(&G.sources);
        let e = &mut sources[source as usize];
        if e.method == Method::Default {
            let setting = e.filename.clone().unwrap_or_default();
            let mut limits = lock(&G.limits[source as usize]);
            env = apply_user_settings(source, e, &mut limits, &setting, &mut settings_errors);
        }
        let after = open_resolved(e, &mut open_errors);
        sync_priorities(&sources);
        after
    };
    log_errors(settings_errors);
    export(env);
    match after {
        AfterOpen::Nothing => {}
        AfterOpen::Syslog => syslog_init(),
        AfterOpen::Journal => {
            journal_direct_init(None);
        }
    }
    log_errors(open_errors);
}

/// `nd_log_initialize()`: stdin on `/dev/null`, then every source in id order.
pub fn initialize() {
    stdin_init();
    for source in Source::ALL {
        open_source(source);
    }
}

/// `nd_log_reopen_log_files()` (SIGHUP, `netdatacli reopen-logs`).
pub fn reopen_log_files(log: bool) {
    if log {
        crate::netdata_log_info!("Reopening all log files.");
    }
    initialize();
    if log {
        crate::netdata_log_info!("Log files re-opened.");
    }
}

/// `nd_log_chown_log_files()`, after the user switch.
pub fn chown_log_files(uid: u32, gid: u32) {
    log_errors(crate::output::chown_log_files(uid, gid));
}

/// `netdata_configured_host_prefix` as the journal socket search sees it.
pub fn set_host_prefix(prefix: &str) {
    *write(&G.host_prefix) = Some(prefix.to_string());
}

static INVOCATION_ID: RwLock<[u8; 16]> = RwLock::new([0; 16]);

/// `initialize_invocation_id()` (a constructor in C): `NETDATA_INVOCATION_ID`, else systemd's `INVOCATION_ID`,
/// else random; exported as 32 lowercase hex digits. Call it first, while the process has one thread.
pub fn init_invocation_id() {
    let parse =
        |name: &str| std::env::var_os(name).and_then(|v| uuid_parse_flexi(v.as_encoded_bytes()));
    let id = parse("NETDATA_INVOCATION_ID")
        .or_else(|| parse("INVOCATION_ID"))
        .unwrap_or_else(|| *uuid::Uuid::new_v4().as_bytes());
    *write(&INVOCATION_ID) = id;
    let mut hex = Vec::with_capacity(32);
    print_uuid_lower_compact(&mut hex, &id);
    export(vec![(
        "NETDATA_INVOCATION_ID",
        String::from_utf8_lossy(&hex).into_owned(),
    )]);
}

/// `nd_log_get_invocation_id()`.
pub fn invocation_id() -> [u8; 16] {
    *crate::output::read(&INVOCATION_ID)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::Fd;

    fn state(fd: Fd) -> SourceState {
        SourceState {
            method: Method::Default,
            format: Format::Logfmt,
            filename: None,
            fd,
            min_priority: Priority::Info,
        }
    }

    fn apply(source: Source, e: &mut SourceState, setting: &str) -> (Limits, OpenErrors, Env) {
        let mut limits = Limits::default_limits();
        let mut errors = OpenErrors::new();
        let env = apply_user_settings(source, e, &mut limits, setting, &mut errors);
        (limits, errors, env)
    }

    #[test]
    fn outputs_split_at_the_last_at_sign() {
        let cases: [(&str, Method, Option<&str>); 9] = [
            ("", Method::Disabled, Some("/dev/null")),
            ("off", Method::Disabled, Some("/dev/null")),
            ("journal", Method::Journal, None),
            ("syslog", Method::Syslog, None),
            ("/dev/null", Method::DevNull, Some("/dev/null")),
            ("system", Method::Stdout, None),
            ("syslog:local0", Method::File, Some("syslog:local0")),
            ("stderr,json", Method::File, Some("stderr,json")),
            (
                "json@a@/var/log/x.log",
                Method::File,
                Some("/var/log/x.log"),
            ),
        ];
        for (setting, method, filename) in cases {
            let mut e = state(Fd::Unset);
            apply(Source::Daemon, &mut e, setting);
            assert_eq!(
                (e.method, e.filename.as_deref()),
                (method, filename),
                "{setting}"
            );
        }
        let mut collector = state(Fd::Stderr);
        apply(Source::Collector, &mut collector, "system");
        assert_eq!(collector.method, Method::Stderr);
    }

    #[test]
    fn options_set_format_level_and_protection() {
        let mut e = state(Fd::Unset);
        let (limits, errors, _) = apply(
            Source::Collector,
            &mut e,
            "json,,level=warn,protection=5/1m@/tmp/c.log",
        );
        assert_eq!(
            (e.format, e.min_priority),
            (Format::Json, Priority::Warning)
        );
        assert_eq!(
            (
                limits.logs_per_period,
                limits.logs_per_period_backup,
                limits.throttle_period
            ),
            (5, 5, 60)
        );
        assert!(errors.is_empty());

        let (limits, _, _) = apply(Source::Daemon, &mut e, "protection=off@x");
        assert_eq!((limits.logs_per_period, limits.throttle_period), (0, 0));
    }

    #[test]
    fn unknown_options_and_bad_periods_are_logged() {
        let mut e = state(Fd::Unset);
        let (limits, errors, _) = apply(
            Source::Access,
            &mut e,
            "color,level,protection=10/soon@/tmp/a.log",
        );
        let messages: Vec<&str> = errors.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Error while parsing configuration of log source 'access'. In config \
                 'color,level,protection=10/soon@/tmp/a.log', 'color' is not understood.",
                "Error while parsing configuration of log source 'access'. In config \
                 'color,level,protection=10/soon@/tmp/a.log', 'level' is not understood.",
                "Error while parsing period 'soon'",
            ]
        );
        assert_eq!((limits.logs_per_period, limits.throttle_period), (10, 60));
    }

    #[test]
    fn the_collector_exports_what_plugins_can_use() {
        let mut e = state(Fd::Stderr);
        let (_, _, env) = apply(Source::Collector, &mut e, "json@/tmp/collector.log");
        assert_eq!(
            env,
            [
                ("NETDATA_LOG_METHOD", "stderr".to_string()),
                ("NETDATA_LOG_FORMAT", "logfmt".to_string()),
                ("NETDATA_LOG_LEVEL", "info".to_string()),
            ]
        );
        let (_, _, env) = apply(Source::Collector, &mut e, "json@journal");
        assert_eq!(
            env[..2],
            [
                ("NETDATA_LOG_METHOD", "journal".to_string()),
                ("NETDATA_LOG_FORMAT", "json".to_string()),
            ]
        );
    }
}
