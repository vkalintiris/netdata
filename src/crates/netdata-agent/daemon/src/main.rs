//! The netdata agent daemon. The startup sequence follows `netdata_main()` in `src/daemon/main.c`, step by step, for
//! the subsystems ported so far (`agent/plan.md` in the status repository tracks the rest).

#![forbid(unsafe_code)]

mod acl;
mod api;
mod build;
mod cli;
mod conf;
mod daemon;
mod data;
mod guid;
mod listen;
mod profile;
mod router;
mod rrdcontext;
mod server;
mod static_file;
mod system;
mod timezone;
mod v1_charts;
mod v1_contexts;

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use netdata_agent_evloop::Pool;
use netdata_agent_inicfg::{LogLevel, SECTION_GLOBAL, SECTION_WEB};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
use netdata_agent_streaming::conf::{LoadDefaults, StreamConf};
use netdata_agent_streaming::receiver::{self, Receivers, StreamWorker};
use netdata_agent_web::request::Settings;
use nix::sys::signal::{SigSet, Signal};

use crate::cli::Opt;
use crate::conf::Conf;

/// A daemon log line in the logfmt layout of `nd_log` (time, program, source, level, message).
fn log(level: LogLevel, message: &str) {
    let level = match level {
        LogLevel::Error => "error",
        LogLevel::Warning => "warning",
        LogLevel::Notice => "notice",
        LogLevel::Info => "info",
        // `[logs] level` defaults to info.
        LogLevel::Debug => return,
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

/// Writes to stdout or stderr, ignoring failures (a full or closed stream must not kill the daemon).
fn out(stream: &mut dyn Write, bytes: &[u8]) {
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
}

fn run(argv: Vec<Vec<u8>>) -> i32 {
    let prog = argv.first().cloned().unwrap_or_else(|| b"netdata".to_vec());
    let mut conf = Conf::default();
    let mut config_loaded = false;
    let mut dont_fork = false;
    let mut pidfile: Option<String> = None;
    let mut logger = |level: LogLevel, message: &str| log(level, message);
    // What C reads lazily at the first libuv_initialize(), inside the first netdata_conf_load().
    let system = system::Resources::probe(&mut logger);
    let text = |v: &[u8]| String::from_utf8_lossy(v).into_owned();
    let help = cli::help_text(build::CONFIG_DIR);

    let (opts, _operands) = cli::getopt(&prog, &argv[1.min(argv.len())..]);
    for opt in opts {
        match opt {
            Opt::WithArg(b'c', file) => {
                let file = text(&file);
                if !conf.netdata_conf_load(Some(&file), true, &system, &mut logger) {
                    log(
                        LogLevel::Error,
                        &format!("Cannot load configuration file {file}."),
                    );
                    return 1;
                }
                conf.cloud_conf_load(true, &mut logger);
                config_loaded = true;
            }
            Opt::Flag(b'D') => dont_fork = true,
            Opt::Flag(b'd') => dont_fork = false,
            Opt::Flag(b'h') => {
                out(&mut std::io::stdout(), help.as_bytes());
                return 0;
            }
            Opt::WithArg(b'i', v) => {
                conf.netdata.set(SECTION_WEB, "bind to", &text(&v));
            }
            Opt::WithArg(b'P', v) => pidfile = Some(text(&v)),
            Opt::WithArg(b'p', v) => {
                conf.netdata.set(SECTION_GLOBAL, "default port", &text(&v));
            }
            Opt::WithArg(b's', v) => {
                conf.netdata
                    .set(SECTION_GLOBAL, "host access prefix", &text(&v));
            }
            Opt::WithArg(b't', v) => {
                conf.netdata.set(SECTION_GLOBAL, "update every", &text(&v));
            }
            Opt::WithArg(b'u', v) => {
                conf.netdata.set(SECTION_GLOBAL, "run as user", &text(&v));
            }
            Opt::Flag(b'v') | Opt::Flag(b'V') => {
                out(
                    &mut std::io::stdout(),
                    format!("netdata {}\n", build::NETDATA_VERSION).as_bytes(),
                );
                return 0;
            }
            Opt::WithArg(b'W', v) => {
                // The -W sub-options are ported with the subsystems they drive.
                let mut message = b"Unknown -W parameter '".to_vec();
                message.extend_from_slice(&v);
                message.extend_from_slice(b"'\n");
                message.extend_from_slice(help.as_bytes());
                out(&mut std::io::stderr(), &message);
                return 1;
            }
            Opt::Invalid(mut message) => {
                message.extend_from_slice(b"Unknown parameter '?'\n");
                message.extend_from_slice(help.as_bytes());
                out(&mut std::io::stderr(), &message);
                return 1;
            }
            other => unreachable!("getopt returned {other:?} for a known option"),
        }
    }

    if !config_loaded {
        conf.netdata_conf_load(None, false, &system, &mut logger);
        conf.cloud_conf_load(false, &mut logger);
    }
    conf.section_directories();
    conf.flush_log(&mut logger);

    // C loads stream.conf from the profile detection inside netdata_conf_section_global(), before the machine GUID;
    // the exact place among the other reads is part of the config-order work.
    let mut stream_conf = StreamConf::default();
    stream_conf.load(
        &mut conf.netdata,
        &conf.dirs.user_config,
        &conf.dirs.stock_config,
        LoadDefaults {
            conf_cpus: conf.threads.cpus,
            libuv_worker_threads: conf.threads.libuv_worker_threads,
            ssl_validate_certificate: true,
        },
        |netdata, is_parent, is_child| {
            profile::detect(
                netdata,
                system.system_cpus,
                system.memory.total,
                is_parent,
                is_child,
            ) == profile::Profile::Parent
        },
        &mut logger,
    );
    conf.flush_log(&mut logger);

    let machine_guid = guid::machine_guid_get(&conf.dirs.varlib, &mut logger);

    // signals_block_all_except_deadly(): every thread started from here on inherits the mask. The main thread waits
    // for the signals C handles; any other signal stays pending forever, so it is ignored.
    let mut blocked = SigSet::all();
    for deadly in [
        Signal::SIGBUS,
        Signal::SIGSEGV,
        Signal::SIGFPE,
        Signal::SIGILL,
        Signal::SIGABRT,
        Signal::SIGSYS,
        Signal::SIGXCPU,
        Signal::SIGXFSZ,
    ] {
        blocked.remove(deadly);
    }
    if blocked.thread_block().is_err() {
        log(
            LogLevel::Error,
            "SIGNALS: cannot apply the default mask for signals",
        );
    }
    let mut handled = SigSet::empty();
    for signal in [
        Signal::SIGPIPE,
        Signal::SIGINT,
        Signal::SIGQUIT,
        Signal::SIGTERM,
        Signal::SIGHUP,
        Signal::SIGUSR2,
    ] {
        handled.add(signal);
    }

    // The "run dir" startup step.
    match system::run_dir(true) {
        Some(dir) => log(LogLevel::Info, &format!("Netdata run directory is '{dir}'")),
        None => {
            log(LogLevel::Error, "Cannot get/create a run directory.");
            return 1;
        }
    }

    conf.section_global_hostname(&mut logger);
    // nd_profile_setup(): the profile once more (it re-reads [global] profile), its malloc settings, then [db].
    let profile = profile::detect(
        &mut conf.netdata,
        system.system_cpus,
        system.memory.total,
        stream_conf.is_parent,
        stream_conf.send.enabled,
    );
    profile::setup_malloc(&mut conf.netdata, profile, system.system_cpus);
    conf.flush_log(&mut logger);
    let db = conf::section_db(&mut conf.netdata, system.page_size, &mut logger);

    // get_system_timezone(), after the hostname and before the listeners and become_daemon(), as in C. No thread has
    // started yet, so setenv() is sound.
    if let Err(err) = conf::set_timezone_env(&mut conf.netdata) {
        log(LogLevel::Error, &format!("TIMEZONE: cannot set TZ: {err}"));
    }
    let tz = timezone::system_timezone(&mut conf.netdata, std::path::Path::new("/"), server::now());

    // cd into the user config dir, so plugins can use relative paths to their config files.
    if std::env::set_current_dir(&conf.dirs.user_config).is_err() {
        log(
            LogLevel::Error,
            &format!("Cannot cd to '{}'", conf.dirs.user_config),
        );
        return 1;
    }

    // nd_web_api_init(): the time-grouping limits, read before the listen sockets as in C.
    let grouping_windows = conf::grouping_windows(&mut conf.netdata);
    let listeners = listen::setup(&mut conf.netdata, &mut logger);
    conf.flush_log(&mut logger);
    if listeners.is_empty() {
        log(
            LogLevel::Error,
            "Cannot setup listen port(s). Is Netdata already running?",
        );
        return 1;
    }

    system::set_nofile_limit(&mut logger);

    // become_daemon(): after the listeners (privileged ports) and before any thread starts.
    match daemon::become_daemon(
        dont_fork,
        &conf.user,
        pidfile.as_deref(),
        &mut conf.netdata,
        &conf.dirs,
        &mut logger,
    ) {
        Ok(daemon::Outcome::Continue) => {}
        Ok(daemon::Outcome::ExitParent) => return 0,
        Err(err) => {
            log(LogLevel::Error, &err);
            return 1;
        }
    }
    conf.flush_log(&mut logger);

    let localhost = Host::new(
        &machine_guid,
        true,
        HostInfo {
            hostname: conf.hostname.clone(),
            registry_hostname: conf.hostname.clone(),
            os: "linux".to_string(),
            timezone: tz.name,
            abbrev_timezone: tz.abbrev,
            utc_offset: tz.utc_offset,
            program_name: "netdata".to_string(),
            program_version: build::NETDATA_VERSION.to_string(),
            update_every: db.update_every,
            db_mode: db.mode,
            history_entries: align_entries_to_pagesize(
                db.mode,
                db.history_entries,
                system.page_size,
            ),
            health_enabled: true,
            system_info: Default::default(),
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
        },
    );
    let hosts = Arc::new(Hosts::new(localhost));
    // stream_thread_get_unsafe(): one thread per core but one, 4..=2048. C starts them on first use.
    let stream_threads = (conf.threads.cpus - 1).clamp(4, 2048) as usize;
    let stream_load: Arc<std::sync::Mutex<Vec<usize>>> = Arc::default();
    let stream_pool = {
        let load = Arc::clone(&stream_load);
        match Pool::spawn(
            stream_threads,
            conf.threads.thread_stack_size,
            |i| format!("STREAM[{i}]"),
            move |_| StreamWorker::new(Arc::clone(&load)),
        ) {
            Ok(pool) => pool,
            Err(err) => {
                log(
                    LogLevel::Error,
                    &format!("Cannot start the stream threads: {err}"),
                );
                return 1;
            }
        }
    };
    let is_parent = stream_conf.is_parent;
    let receivers = Arc::new(Receivers::new(
        stream_conf,
        Arc::clone(&hosts),
        stream_load,
        receiver::Defaults {
            // No dbengine yet: C falls back to alloc when dbengine is unavailable.
            db_mode: if db.mode == DbMode::Dbengine {
                DbMode::Alloc
            } else {
                db.mode
            }
            .name()
            .to_string(),
            history: db.history_entries,
            health_enabled: true,
            update_every: db.update_every,
            gap_when_lost_iterations_above: db.gap_when_lost_iterations_above,
            page_size: system.page_size,
        },
        stream_pool.handle(),
        log,
    ));
    // netdata_conf_section_web() runs just before C starts its static threads, the web server among them, which
    // then reads its thread count.
    let web = conf.section_web(&mut logger);
    let web_server_threads = conf::web_query_threads(
        &mut conf.netdata,
        conf.threads.cpus as usize,
        is_parent,
        &mut logger,
    );
    conf.flush_log(&mut logger);
    receivers.set_streaming_rate(web.streaming_rate_s);
    let release_channel =
        v1_charts::release_channel(&conf.dirs.user_config, build::NETDATA_VERSION);
    let shared = Arc::new(server::Shared {
        settings: Settings {
            gzip: web.gzip,
            respect_do_not_track: web.respect_do_not_track,
        },
        version: build::NETDATA_VERSION,
        gzip_level: web.gzip_level,
        x_frame_options: web.x_frame_options,
        acl: web.acl,
        log,
        // C keeps these as int seconds; a negative value disables the check as 0 does.
        first_request_timeout_s: web.first_request_timeout_s.max(0) as u64,
        idle_timeout_s: web.disconnect_idle_after_s.max(0) as u64,
        info: api::Info {
            version: build::NETDATA_VERSION,
            machine_guid,
        },
        web_dir: conf.dirs.web.clone(),
        hosts: Arc::clone(&hosts),
        grouping_windows,
        release_channel,
        // Every startup read is done: from here on netdata.conf is read and dumped under its lock.
        netdata_conf: std::sync::Mutex::new(std::mem::take(&mut conf.netdata)),
        custom_dashboard_info: Default::default(),
    });
    let sockets: Vec<(std::net::TcpListener, u32)> =
        listeners.into_iter().map(|l| (l.socket, l.acl)).collect();
    // Every worker polls every listener through its own duplicate; running out of descriptors here is an error,
    // not a panic.
    let mut worker_sockets = Vec::with_capacity(web_server_threads);
    for _ in 0..web_server_threads {
        match sockets
            .iter()
            .map(|(socket, acl)| socket.try_clone().map(|s| (s, *acl)))
            .collect::<std::io::Result<Vec<_>>>()
        {
            Ok(set) => worker_sockets.push(set),
            Err(err) => {
                log(
                    LogLevel::Error,
                    &format!("Cannot start the web server threads: {err}"),
                );
                return 1;
            }
        }
    }
    let pool = match Pool::spawn(
        web_server_threads,
        conf.threads.thread_stack_size,
        |i| format!("WEB[{}]", i + 1),
        |i| {
            server::WebWorker::new(
                std::mem::take(&mut worker_sockets[i]),
                Arc::clone(&shared),
                Arc::clone(&receivers),
            )
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
    let contexts_worker =
        match rrdcontext::Worker::spawn(Arc::clone(&hosts), conf.threads.thread_stack_size) {
            Ok(worker) => worker,
            Err(err) => {
                log(
                    LogLevel::Error,
                    &format!("Cannot start the RRDCONTEXT thread: {err}"),
                );
                return 1;
            }
        };
    log(LogLevel::Info, "NETDATA STARTUP: completed");

    loop {
        match handled.wait() {
            Ok(Signal::SIGINT | Signal::SIGQUIT | Signal::SIGTERM) => break,
            // SIGPIPE is ignored; log reopening (HUP) and health reloads (USR2) come with logging and health.
            Ok(_) => continue,
            Err(_) => continue,
        }
    }

    log(LogLevel::Info, "shutting down");
    let _ = pool.stop();
    let _ = stream_pool.stop();
    contexts_worker.stop();
    if let Some(pidfile) = &pidfile {
        let _ = std::fs::remove_file(pidfile);
    }
    0
}

fn main() -> ExitCode {
    use std::os::unix::ffi::OsStringExt;
    ExitCode::from(run(std::env::args_os().map(OsStringExt::into_vec).collect()) as u8)
}
