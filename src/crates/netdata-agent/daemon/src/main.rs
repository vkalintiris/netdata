//! The netdata agent daemon. The startup sequence follows `netdata_main()` in `src/daemon/main.c`, step by step, for
//! the subsystems ported so far (`agent/plan.md` in the status repository tracks the rest).

#![forbid(unsafe_code)]

mod api;
mod build;
mod cli;
mod conf;
mod guid;
mod listen;
mod router;
mod server;
mod static_file;

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use netdata_agent_evloop::Pool;
use netdata_agent_inicfg::{LogLevel, SECTION_GLOBAL, SECTION_WEB};
use netdata_agent_web::request::Settings;
use nix::sys::signal::{SigSet, Signal};

use crate::cli::Opt;
use crate::conf::Conf;

/// Web workers until `[web] web server threads` is ported.
const WEB_SERVER_THREADS: usize = 6;

/// A daemon log line in the logfmt layout of `nd_log` (time, program, source, level, message).
fn log(level: LogLevel, message: &str) {
    let level = match level {
        LogLevel::Error => "error",
        LogLevel::Info => "info",
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let message = message.trim_end_matches('\n').replace('"', "\\\"");
    let _ = writeln!(
        std::io::stderr(),
        "time={}.{:03}Z comm=netdata source=daemon level={level} msg=\"{message}\"",
        now.as_secs(),
        now.subsec_millis()
    );
}

fn run(argv: Vec<String>) -> i32 {
    let prog = argv
        .first()
        .cloned()
        .unwrap_or_else(|| "netdata".to_string());
    let mut conf = Conf::default();
    let mut config_loaded = false;
    let mut dont_fork = false;
    let mut pidfile: Option<String> = None;
    let mut logger = |level: LogLevel, message: &str| log(level, message);

    let (opts, _operands) = cli::getopt(&prog, &argv[1.min(argv.len())..]);
    for opt in opts {
        match opt {
            Opt::WithArg('c', file) => {
                if !conf.netdata_conf_load(Some(&file), true, &mut logger) {
                    log(
                        LogLevel::Error,
                        &format!("Cannot load configuration file {file}."),
                    );
                    return 1;
                }
                conf.cloud_conf_load(true, &mut logger);
                config_loaded = true;
            }
            Opt::Flag('D') => dont_fork = true,
            Opt::Flag('d') => dont_fork = false,
            Opt::Flag('h') => {
                print!("{}", cli::help_text(build::CONFIG_DIR));
                return 0;
            }
            Opt::WithArg('i', v) => {
                conf.netdata.set(SECTION_WEB, "bind to", &v);
            }
            Opt::WithArg('P', v) => pidfile = Some(v),
            Opt::WithArg('p', v) => {
                conf.netdata.set(SECTION_GLOBAL, "default port", &v);
            }
            Opt::WithArg('s', v) => {
                conf.netdata.set(SECTION_GLOBAL, "host access prefix", &v);
            }
            Opt::WithArg('t', v) => {
                conf.netdata.set(SECTION_GLOBAL, "update every", &v);
            }
            Opt::WithArg('u', v) => {
                conf.netdata.set(SECTION_GLOBAL, "run as user", &v);
            }
            Opt::Flag('v') | Opt::Flag('V') => {
                println!("netdata {}", build::NETDATA_VERSION);
                return 0;
            }
            Opt::WithArg('W', v) => {
                // The -W sub-options are ported with the subsystems they drive.
                eprintln!("Unknown -W parameter '{v}'");
                eprint!("{}", cli::help_text(build::CONFIG_DIR));
                return 1;
            }
            Opt::Invalid(message) => {
                eprint!("{message}");
                eprintln!("Unknown parameter '?'");
                eprint!("{}", cli::help_text(build::CONFIG_DIR));
                return 1;
            }
            other => unreachable!("getopt returned {other:?} for a known option"),
        }
    }

    if !config_loaded {
        conf.netdata_conf_load(None, false, &mut logger);
        conf.cloud_conf_load(false, &mut logger);
    }
    conf.section_directories();
    conf.flush_log(&mut logger);

    let machine_guid = match guid::machine_guid_get(&conf.dirs.varlib) {
        Ok(guid) => guid,
        Err(err) => {
            log(
                LogLevel::Error,
                &format!("Cannot get or save the machine GUID: {err}"),
            );
            return 1;
        }
    };

    // Every thread started from here on inherits these signals blocked; the main thread waits for them.
    let mut handled = SigSet::empty();
    for signal in [
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGTERM,
        Signal::SIGHUP,
        Signal::SIGUSR2,
    ] {
        handled.add(signal);
    }
    if handled.thread_block().is_err() {
        log(LogLevel::Error, "Cannot block the handled signals.");
        return 1;
    }

    conf.section_global_hostname();

    let listeners = listen::setup(&mut conf.netdata, &mut logger);
    conf.flush_log(&mut logger);
    if listeners.is_empty() {
        log(
            LogLevel::Error,
            "Cannot setup listen port(s). Is Netdata already running?",
        );
        return 1;
    }

    if !dont_fork {
        // Daemonizing needs the audited sys crate (decisions D12); until then the daemon stays in the foreground.
        log(
            LogLevel::Info,
            "running in the foreground (daemonizing is not supported yet)",
        );
    }
    if let Some(pidfile) = &pidfile {
        if let Err(err) = std::fs::write(pidfile, format!("{}\n", std::process::id())) {
            log(
                LogLevel::Error,
                &format!("Cannot write pidfile '{pidfile}': {err}"),
            );
        }
    }

    let shared = Arc::new(server::Shared {
        settings: Settings {
            gzip: true,
            respect_do_not_track: false,
        },
        version: build::NETDATA_VERSION,
        gzip_level: 3,
        info: api::Info {
            version: build::NETDATA_VERSION,
            machine_guid,
            hostname: conf.hostname.clone(),
        },
        web_dir: conf.dirs.web.clone(),
    });
    let sockets: Vec<std::net::TcpListener> = listeners.into_iter().map(|l| l.socket).collect();
    let pool = match Pool::spawn(
        WEB_SERVER_THREADS,
        |i| format!("WEB[{}]", i + 1),
        |_| {
            server::WebWorker::new(
                sockets
                    .iter()
                    .map(|s| s.try_clone().expect("dup listener"))
                    .collect(),
                Arc::clone(&shared),
            )
            .expect("web worker")
        },
    ) {
        Ok(pool) => pool,
        Err(err) => {
            log(
                LogLevel::Error,
                &format!("Cannot start the web server threads: {err}"),
            );
            return 1;
        }
    };
    log(LogLevel::Info, "NETDATA STARTUP: completed");

    loop {
        match handled.wait() {
            Ok(Signal::SIGINT | Signal::SIGQUIT | Signal::SIGTERM) => break,
            // Log reopening and health reloads come with logging and health.
            Ok(_) => continue,
            Err(_) => continue,
        }
    }

    log(LogLevel::Info, "shutting down");
    let _ = pool.stop();
    if let Some(pidfile) = &pidfile {
        let _ = std::fs::remove_file(pidfile);
    }
    0
}

fn main() -> ExitCode {
    ExitCode::from(run(std::env::args().collect()) as u8)
}
