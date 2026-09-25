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

use netdata_agent_log::{Priority, Source, nd_log};
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use netdata_agent_evloop::Pool;
use netdata_agent_inicfg::{SECTION_GLOBAL, SECTION_WEB};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
use netdata_agent_streaming::conf::{LoadDefaults, StreamConf};
use netdata_agent_streaming::receiver::{self, Receivers, StreamWorker};
use netdata_agent_web::request::Settings;
use nix::sys::signal::{SigSet, Signal};

use crate::cli::Opt;
use crate::conf::Conf;

/// Writes to stdout or stderr, ignoring failures (a full or closed stream must not kill the daemon).
fn out(stream: &mut dyn Write, bytes: &[u8]) {
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
}

fn run(argv: Vec<Vec<u8>>) -> i32 {
    // C's constructor-time invocation id, then `program_name`; until nd_log_initialize() records go to stderr.
    netdata_agent_log::init_invocation_id();
    netdata_agent_log::set_program_name("netdata");
    netdata_agent_log::set_default_log_dir(build::LOG_DIR);
    let prog = argv.first().cloned().unwrap_or_else(|| b"netdata".to_vec());
    let mut conf = Conf::default();
    let mut config_loaded = false;
    let mut dont_fork = false;
    let mut pidfile: Option<String> = None;
    // What C reads lazily at the first libuv_initialize(), inside the first netdata_conf_load().
    let system = system::Resources::probe();
    let text = |v: &[u8]| String::from_utf8_lossy(v).into_owned();
    let help = cli::help_text(build::CONFIG_DIR);

    let (opts, _operands) = cli::getopt(&prog, &argv[1.min(argv.len())..]);
    for opt in opts {
        match opt {
            Opt::WithArg(b'c', file) => {
                let file = text(&file);
                if !conf.netdata_conf_load(Some(&file), true, &system) {
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        "Cannot load configuration file {file}."
                    );
                    return 1;
                }
                conf.cloud_conf_load(true);
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
        conf.netdata_conf_load(None, false, &system);
        conf.cloud_conf_load(false);
    }
    // Logging first, so that everything after it logs where the configuration says; no flood protection while
    // starting up.
    conf.section_directories();
    conf.section_logs();
    netdata_agent_log::limits_unlimited();
    netdata_agent_log::initialize();

    // C loads stream.conf from the profile detection of the "signals" step, after the machine GUID; moving it there
    // (and the run dir after [db]) comes with the startup steps of the logging port, which fix the log line order.
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
    );

    let machine_guid = guid::machine_guid_get(&conf.dirs.varlib);

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
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "SIGNALS: cannot apply the default mask for signals"
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
        Some(dir) => nd_log!(
            Source::Daemon,
            Priority::Info,
            "Netdata run directory is '{dir}'"
        ),
        None => {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot get/create a run directory."
            );
            return 1;
        }
    }

    conf.section_global_hostname();
    // nd_profile_setup(): the profile once more (it re-reads [global] profile), its malloc settings, then [db].
    let profile = profile::detect(
        &mut conf.netdata,
        system.system_cpus,
        system.memory.total,
        stream_conf.is_parent,
        stream_conf.send.enabled,
    );
    profile::setup_malloc(&mut conf.netdata, profile, system.system_cpus);
    let db = conf::section_db(&mut conf.netdata, system.page_size);

    // set_environment_for_plugins_and_scripts(): an unusable required directory is C's fatal().
    if let Err(err) = conf.environment_for_plugins(db.update_every) {
        nd_log!(Source::Daemon, Priority::Err, "{}", err);
        return 1;
    }

    // cd into the user config dir, so plugins can use relative paths to their config files.
    if std::env::set_current_dir(&conf.dirs.user_config).is_err() {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Cannot cd to '{}'",
            conf.dirs.user_config
        );
        return 1;
    }

    // get_system_timezone() at the "analytics" step. No thread has started yet, so setenv() is sound.
    if let Err(err) = conf::set_timezone_env(&mut conf.netdata) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "TIMEZONE: cannot set TZ: {err}"
        );
    }
    let tz = timezone::system_timezone(&mut conf.netdata, std::path::Path::new("/"), server::now());

    // nd_web_api_init(): the time-grouping limits, read before the listen sockets as in C.
    let grouping_windows = conf::grouping_windows(&mut conf.netdata);
    // web_server_threading_selection(): with `[web] mode = none` there is no web server at all.
    let web_enabled = conf::web_server_enabled(&mut conf.netdata);
    let listeners = if web_enabled {
        listen::setup(&mut conf.netdata)
    } else {
        Vec::new()
    };
    if web_enabled && listeners.is_empty() {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Cannot setup listen port(s). Is Netdata already running?"
        );
        return 1;
    }

    system::set_nofile_limit();

    // become_daemon(): after the listeners (privileged ports) and before any thread starts.
    match daemon::become_daemon(
        dont_fork,
        &conf.user,
        pidfile.as_deref(),
        &mut conf.netdata,
        &conf.dirs,
    ) {
        Ok(daemon::Outcome::Continue) => {}
        Ok(daemon::Outcome::ExitParent) => return 0,
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, "{}", err);
            return 1;
        }
    }
    // The "home" step: after the user switch, while there is still one thread.
    conf.section_home();

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
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Cannot start the stream threads: {err}"
                );
                return 1;
            }
        }
    };
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
    ));
    // The "static threads" step: flood protection back on, then netdata_conf_section_web() just before C starts its
    // static threads, the web server among them, which then reads its thread count.
    netdata_agent_log::limits_reset();
    let web = conf.section_web();
    // The web server thread reads its sizing only when it runs.
    let (web_server_threads, max_sockets) = if web_enabled {
        // netdata_conf_is_parent(): the node profile, not whether stream.conf enables an API key
        let threads = conf::web_query_threads(
            &mut conf.netdata,
            conf.threads.cpus as usize,
            profile == profile::Profile::Parent,
        );
        let max_sockets = conf::web_server_max_sockets_per_worker(&mut conf.netdata, threads);
        (threads, max_sockets)
    } else {
        (0, 0)
    };
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
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Cannot start the web server threads: {err}"
                );
                return 1;
            }
        }
    }
    let pool = if web_enabled {
        match Pool::spawn(
            web_server_threads,
            conf.threads.thread_stack_size,
            |i| format!("WEB[{}]", i + 1),
            |i| {
                server::WebWorker::new(
                    std::mem::take(&mut worker_sockets[i]),
                    max_sockets,
                    Arc::clone(&shared),
                    Arc::clone(&receivers),
                )
            },
        ) {
            Ok(pool) => Some(pool),
            Err(err) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Cannot start the web server threads: {err}"
                );
                return 1;
            }
        }
    } else {
        None
    };
    let contexts_worker =
        match rrdcontext::Worker::spawn(Arc::clone(&hosts), conf.threads.thread_stack_size) {
            Ok(worker) => worker,
            Err(err) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Cannot start the RRDCONTEXT thread: {err}"
                );
                return 1;
            }
        };
    nd_log!(Source::Daemon, Priority::Info, "NETDATA STARTUP: completed");

    loop {
        match handled.wait() {
            Ok(Signal::SIGINT | Signal::SIGQUIT | Signal::SIGTERM) => break,
            Ok(Signal::SIGHUP) => {
                // process_triggered_signals(): the reopen runs without flood protection
                netdata_agent_log::limits_unlimited();
                nd_log!(
                    Source::Daemon,
                    Priority::Info,
                    "SIGNAL: Received SIGHUP. Reopening all log files..."
                );
                netdata_agent_log::reopen_log_files(true);
                netdata_agent_log::limits_reset();
            }
            // SIGPIPE is ignored; health reloads (USR2) come with health.
            Ok(_) => continue,
            Err(_) => continue,
        }
    }

    nd_log!(Source::Daemon, Priority::Info, "shutting down");
    if let Some(pool) = pool {
        let _ = pool.stop();
    }
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
