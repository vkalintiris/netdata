//! Netdata's alert notification dispatcher.
//!
//! This is the native replacement for `alarm-notify.sh`. It keeps that program's
//! externally visible contract intact, because the daemon, the analytics collector,
//! the documentation and every user's configuration all depend on it:
//!
//! * 33 positional arguments in a fixed order (`args`).
//! * The `test [role]`, `unittest` and `dump_methods` modes.
//! * Exit code 0 when at least one notification was delivered - the daemon stores
//!   this as the alert's `exec_code` and forwards it to Netdata Cloud.
//! * The journald record shape, including `MESSAGE_ID` and the `ND_ALERT_*` fields.
//! * `health_alarm_notify.conf`, including a user-supplied `custom_sender()`.
//!
//! Nothing is written to stdout during normal dispatch: the daemon creates a pipe
//! for it but never drains it, so a chatty notifier would eventually block. Only
//! `dump_methods` and `unittest`, whose callers do read stdout, print there.

pub mod args;
pub mod conf_parser;
pub mod config;
pub mod custom;
pub mod datefmt;
pub mod exec;
pub mod hostname;
pub mod http;
pub mod logging;
pub mod message;
pub mod paths;
pub mod recipients;
pub mod senders;
pub mod textutil;

use std::process::ExitCode;

use args::{AlertArgs, Invocation, Status};
use config::{Config, METHOD_NAMES};
use http::HttpClient;
use message::Message;
use paths::Paths;

/// Exit codes. The daemon only distinguishes zero from non-zero, but keeping the
/// script's two values avoids surprising anyone reading `exec_code` in the database.
const DELIVERED: u8 = 0;
const NOTHING_DELIVERED: u8 = 1;

/// Entry point shared by the binary and the integration tests.
pub fn run(argv: &[String], program_name: &str) -> ExitCode {
    let debug = std::env::var("NETDATA_ALARM_NOTIFY_DEBUG").unwrap_or_default() == "1";
    logging::init(logging::LogContext::from_args(argv, program_name), debug);

    match args::parse(argv) {
        Invocation::Test { role } => run_test_mode(&role),
        Invocation::DumpMethods => run_dump_methods(),
        Invocation::UnitTest {
            role,
            config_file,
            status,
            old_status,
        } => run_unittest(&role, &config_file, &status, &old_status),
        Invocation::Notify(alert) => notify(&alert, debug),
    }
}

/// Normal dispatch.
fn notify(alert: &AlertArgs, debug: bool) -> ExitCode {
    let paths = Paths::from_environment();
    let mut cfg = Config::load(&paths);

    // Only real transitions are notified. The configuration is loaded first so that
    // `clear_alarm_always` can actually take effect - the shell tested it before
    // sourcing the file, which meant the option never did anything.
    if !alert.status().is_notifiable() {
        tracing::debug!("not sending notification for status {}", alert.status);
        return ExitCode::from(NOTHING_DELIVERED);
    }
    if alert.status() == Status::Clear
        && !matches!(alert.old_status(), Status::Warning | Status::Critical)
        && cfg.str("clear_alarm_always") != "YES"
    {
        tracing::debug!(
            "not sending notification for a CLEAR that follows {}",
            alert.old_status
        );
        return ExitCode::from(NOTHING_DELIVERED);
    }

    let recipients = recipients::resolve(
        &mut cfg,
        &alert.roles,
        alert.status(),
        &alert.alarm_id,
        &paths,
    );

    if !cfg.any_enabled() {
        let msg = Message::build(alert, &cfg, &paths);
        if !recipients.have_to_send_something {
            tracing::debug!(
                "All notification methods are disabled; not sending {}.",
                msg.notification_description
            );
            return ExitCode::from(NOTHING_DELIVERED);
        }
        // Methods were enabled but nothing was addressable: a misconfiguration the
        // operator needs to see, which is why this is louder than the case above.
        tracing::error!(
            "All notification methods are disabled; not sending {}.",
            msg.notification_description
        );
        return ExitCode::from(NOTHING_DELIVERED);
    }

    cfg.check_supported_targets(false);
    let custom_sender = if cfg.enabled("custom") {
        custom::resolve_sender(&cfg)
    } else {
        None
    };

    let msg = Message::build(alert, &cfg, &paths);
    let http = match HttpClient::new(&cfg, debug) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("cannot initialise the HTTP client: {e:#}");
            return ExitCode::from(NOTHING_DELIVERED);
        }
    };

    let ctx = senders::Ctx {
        args: alert,
        cfg: &cfg,
        msg: &msg,
        http: &http,
        recipients: &recipients,
        debug,
    };

    // A current-thread runtime: this process sends a handful of requests and exits,
    // so a worker pool would cost more than it saves.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            tracing::error!("cannot start the async runtime: {e}");
            return ExitCode::from(NOTHING_DELIVERED);
        }
    };

    let delivered = runtime.block_on(senders::dispatch_all(&ctx, custom_sender.as_ref()));

    if delivered {
        ExitCode::from(DELIVERED)
    } else {
        ExitCode::from(NOTHING_DELIVERED)
    }
}

/// `dump_methods`: the enabled `SEND_*` variables, one per line, for the Agent's
/// anonymous-statistics collector (`src/daemon/analytics.c`).
fn run_dump_methods() -> ExitCode {
    let paths = Paths::from_environment();
    let mut cfg = Config::load(&paths);
    // Quiet: this runs on every analytics cycle and must not fill the log.
    cfg.check_supported_targets(true);
    for name in cfg.enabled_send_variables() {
        println!("{name}");
    }
    ExitCode::from(DELIVERED)
}

/// `unittest`: report the recipients each method would use for one role.
fn run_unittest(role: &str, config_file: &str, status: &str, old_status: &str) -> ExitCode {
    let paths = Paths::from_environment();
    let mut cfg = Config::load_file(std::path::Path::new(config_file));
    let _ = old_status;
    let recipients = recipients::resolve(&mut cfg, role, Status::parse(status), "0", &paths);
    for method in METHOD_NAMES {
        println!("results: {method}: {}", recipients.joined(method));
    }
    ExitCode::from(DELIVERED)
}

/// `test [role]`: send one WARNING, one CRITICAL and one CLEAR through the real
/// dispatch path, exactly as the shell script's self-test did.
fn run_test_mode(role: &str) -> ExitCode {
    let program = std::env::current_exe().unwrap_or_else(|_| "alarm-notify".into());
    let host = hostname::full();
    let now = datefmt::now_secs().to_string();
    let mut failed = false;
    let mut last = "CLEAR".to_string();

    for (i, status) in ["WARNING", "CRITICAL", "CLEAR"].into_iter().enumerate() {
        let id = (i + 1).to_string();
        eprintln!();
        eprintln!("# SENDING TEST {status} ALARM TO ROLE: {role}");

        let argv = vec![
            role.to_string(),
            host.clone(),
            "1".to_string(),
            "1".to_string(),
            id.clone(),
            now.clone(),
            "test_alarm".to_string(),
            "test.chart".to_string(),
            status.to_string(),
            last.clone(),
            "100".to_string(),
            "90".to_string(),
            program.display().to_string(),
            "1".to_string(),
            id.clone(),
            "units".to_string(),
            "this is a test alarm to verify notifications work".to_string(),
            "new value".to_string(),
            "old value".to_string(),
            "evaluated expression".to_string(),
            "expression variable values".to_string(),
            "0".to_string(),
            "0".to_string(),
            String::new(),
            String::new(),
            "Test".to_string(),
            format!("command to edit the alarm=0={host}"),
            String::new(),
            String::new(),
            "a test alarm".to_string(),
        ];

        let ok = match std::process::Command::new(&program).args(&argv).status() {
            Ok(st) => st.success(),
            Err(e) => {
                eprintln!("# FAILED to run {}: {e}", program.display());
                false
            }
        };
        if ok {
            eprintln!("# OK");
        } else {
            eprintln!("# FAILED");
            failed = true;
        }

        last = status.to_string();
    }

    if failed {
        ExitCode::from(NOTHING_DELIVERED)
    } else {
        ExitCode::from(DELIVERED)
    }
}
