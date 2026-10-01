//! The netdata agent daemon. The startup sequence follows `netdata_main()` in `src/daemon/main.c`, step by step, for
//! the subsystems ported so far (`agent/plan.md` in the status repository tracks the rest).

#![forbid(unsafe_code)]

mod access_log;
mod acl;
mod api;
mod archived;
mod auth;
mod backfill;
mod bearer;
mod build;
mod buildinfo;
mod capas;
mod cli;
mod cloud;
mod cloud_proxy;
mod command_server;
mod commands;
mod conf;
mod contexts_v2;
mod ctxload;
mod daemon;
mod data;
mod dbengine;
mod dbengine_stats;
mod exit_reason;
mod guid;
mod health;
mod heartbeat;
mod host_labels;
mod listen;
mod maintenance;
mod meta_store;
mod metasync;
mod plugins_d;
mod profile;
mod pulse;
mod router;
mod rrdcontext;
mod server;
mod shutdown;
mod startup;
mod static_file;
mod status_file;
mod stream_info;
mod system;
mod system_info;
mod timezone;
mod v1_charts;
mod v1_contexts;

use netdata_agent_metadata::open::{ContextDb, MetaDb};
use netdata_agent_metadata::read::{EventKind, NodeId};

use netdata_agent_log::{Priority, Source, fatal, nd_log, netdata_log_info};
use std::io::Write;
use std::process::ExitCode;
use std::sync::{Arc, Weak};

use netdata_agent_evloop::Pool;
use netdata_agent_inicfg::{SECTION_GLOBAL, SECTION_LOGS, SECTION_WEB};
use netdata_agent_rrd::contexts::DbRotation;
use netdata_agent_rrd::host::{Host, HostInfo, Hosts, StreamSend};
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};
use netdata_agent_rrd::storage::{Backfill, StorageLayout};
use netdata_agent_streaming::conf::{LoadDefaults, StreamConf};
use netdata_agent_streaming::receiver::{self, Receivers};
use netdata_agent_streaming::thread::StreamWorker;
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
    // every spawn server this process creates (`-W buildinfo`'s included) re-executes it, its socket in C's run dir
    netdata_agent_spawn::popen::configure(netdata_agent_spawn::client::Start {
        exe: "/proc/self/exe".into(),
        run_dir: || system::run_dir(true),
    });
    // the log's exports go where every other one does
    netdata_agent_log::set_env_writer(netdata_agent_spawn::env::set);
    netdata_agent_rrd::host::set_netdata_start_time(
        netdata_agent_rrd::collection::now_realtime_timeval().0,
    );
    let mut startup = startup::Startup::new();
    // C's constructor-time invocation id, then `program_name`; until nd_log_initialize() records go to stderr.
    netdata_agent_log::init_invocation_id();
    netdata_agent_log::set_program_name("netdata");
    netdata_agent_log::set_default_log_dir(build::LOG_DIR);
    // curl_global_init(CURL_GLOBAL_ALL), before the options, as C
    curl::init();
    let prog = argv.first().cloned().unwrap_or_else(|| b"netdata".to_vec());
    let mut conf = Conf::default();
    let mut config_loaded = false;
    let mut dont_fork = false;
    let mut close_open_fds = true;
    let mut pidfile: Option<String> = None;
    // What C reads lazily at the first libuv_initialize(), inside the first netdata_conf_load().
    let system = system::Resources::probe();
    let text = |v: &[u8]| String::from_utf8_lossy(v).into_owned();
    let help = cli::help_text(build::CONFIG_DIR);

    let args = &argv[1.min(argv.len())..];
    let mut options = cli::Getopt::new(&prog, args);
    while let Some(opt) = options.next() {
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
            // sql_init_meta_database() with its check, in the cache directory known so far (compile-time, or a
            // configuration's loaded by an earlier -c), before the library is initialized
            Opt::WithArg(b'W', v)
                if matches!(
                    &v[..],
                    b"sqlite-meta-recover" | b"sqlite-compact" | b"sqlite-analyze"
                ) =>
            {
                use netdata_agent_metadata::open::{Check, MetaDb};
                let check = match &v[..] {
                    b"sqlite-meta-recover" => Check::Recover,
                    b"sqlite-compact" => Check::ReclaimSpace,
                    _ => Check::Analyze,
                };
                MetaDb::check(std::path::Path::new(&conf.dirs.cache), check);
                return 0;
            }
            Opt::WithArg(b'W', v) if v == b"simple-pattern" => {
                let Some(words) = options.take_words(2) else {
                    out(&mut std::io::stderr(), cli::SIMPLE_PATTERN_USAGE.as_bytes());
                    return 1;
                };
                return cli::simple_pattern_check(&words[0], &words[1], &mut std::io::stdout());
            }
            Opt::WithArg(b'W', v) if v.starts_with(b"stacksize=") => {
                conf.netdata
                    .set(SECTION_GLOBAL, "pthread stack size", &text(&v[10..]));
            }
            // C also sets its in-memory debug flags, for a debug.log the Rust agent does not write
            Opt::WithArg(b'W', v) if v.starts_with(b"debug_flags=") => {
                conf.netdata
                    .set(SECTION_LOGS, "debug flags", &text(&v[12..]));
            }
            // a default for the key, which a value netdata.conf sets still overrides
            Opt::WithArg(b'W', v) if v == b"set" || v == b"set2" => {
                let set2 = v == b"set2";
                let Some(words) = options.take_words(if set2 { 4 } else { 3 }) else {
                    let usage = if set2 {
                        cli::SET2_USAGE
                    } else {
                        cli::SET_USAGE
                    };
                    out(&mut std::io::stderr(), usage.as_bytes());
                    return 1;
                };
                let w: Vec<String> = words.iter().map(|w| text(w)).collect();
                let (target, rest) = match set2 {
                    true if w[0] == "cloud" => (&mut conf.cloud, &w[1..]),
                    true => (&mut conf.netdata, &w[1..]),
                    false => (&mut conf.netdata, &w[..]),
                };
                target.set_default_raw_value(&rest[0], &rest[1], &rest[2]);
            }
            Opt::WithArg(b'W', v) if v == b"get" || v == b"get2" => {
                let get2 = v == b"get2";
                let Some(words) = options.take_words(if get2 { 4 } else { 3 }) else {
                    let usage = if get2 {
                        cli::GET2_USAGE
                    } else {
                        cli::GET_USAGE
                    };
                    out(&mut std::io::stderr(), usage.as_bytes());
                    return 1;
                };
                if !config_loaded {
                    out(
                        &mut std::io::stderr(),
                        b"warning: no configuration file has been loaded. Use -c CONFIG_FILE, before -W get. Using \
                          default config.\n",
                    );
                    conf.netdata_conf_load(None, false, &system);
                    if get2 {
                        conf.cloud_conf_load(true);
                    }
                }
                // netdata_conf_section_global(): the directories, the hostname, the profile (which loads
                // stream.conf) and [db]
                conf.section_directories();
                conf.section_global_hostname();
                let stream_conf = load_stream_conf(&mut conf, &system);
                profile::detect(
                    &mut conf.netdata,
                    system.system_cpus,
                    system.memory.total,
                    stream_conf.is_parent,
                    stream_conf.send.enabled,
                );
                conf::section_db(&mut conf.netdata, system.page_size, &conf.dirs.cache);
                let w: Vec<String> = words.iter().map(|w| text(w)).collect();
                let (target, rest) = match get2 {
                    true if w[0] == "cloud" => (&mut conf.cloud, &w[1..]),
                    true => (&mut conf.netdata, &w[1..]),
                    false => (&mut conf.netdata, &w[..]),
                };
                let mut value = target
                    .get(&rest[0], &rest[1], Some(&rest[2]))
                    .unwrap_or_default();
                value.push(b'\n');
                out(&mut std::io::stdout(), &value);
                return 0;
            }
            Opt::WithArg(b'W', v) if v == b"buildinfo" || v == b"buildinfojson" => {
                // print_build_info(): the packaging info (the profile loads stream.conf), the system info, the
                // directories; netdata.conf only when -c came first
                conf.section_directories();
                let stream_conf = load_stream_conf(&mut conf, &system);
                let profile = profile::detect(
                    &mut conf.netdata,
                    system.system_cpus,
                    system.memory.total,
                    stream_conf.is_parent,
                    stream_conf.send.enabled,
                );
                let memory = system::system_memory_cached(true);
                let si = system_info::for_build_info(&conf.primary_plugins_dir(), &conf.dirs.user_config);
                let info = buildinfo::BuildInfo::new(&buildinfo::Inputs {
                    dirs: &conf.dirs,
                    home: build::VARLIB_DIR,
                    system: &si,
                    profile: profile.name(),
                    parent: stream_conf.is_parent,
                    child: stream_conf.send.enabled,
                    memory,
                });
                let text = if v == b"buildinfo" {
                    info.text().into_bytes()
                } else {
                    info.json()
                };
                out(&mut std::io::stdout(), &text);
                return 0;
            }
            Opt::WithArg(b'W', v) if v == b"cmakecache" => {
                out(&mut std::io::stdout(), &buildinfo::cmake_cache());
                return 0;
            }
            // an internal option: profilers keep their own descriptors open
            Opt::WithArg(b'W', v) if v == b"keepopenfds" => close_open_fds = false,
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

    // what the launcher left open (lxc-attach does), as C closes it right after the options
    if close_open_fds {
        let _ = netdata_agent_sys::close_inherited_fds();
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

    // before anything else saves one: the last run's status file, which the machine GUID may come from
    status_file::init(&conf.dirs.varlib, &conf.dirs.cache, &conf.dirs.user_config);
    let machine_guid = guid::machine_guid_get(&conf.dirs.varlib, &[0; 16]).txt.clone();
    netdata_agent_log::register_fatal_hook(status_file::register_fatal);
    // fatal_status_file_save(): until startup completes, a fatal() saves the status file and exits
    netdata_agent_log::register_fatal_final_callback(|| status_file::update_status(status_file::DaemonStatus::None));
    // a panic is a fatal error (D87 F6, D90.4)
    std::panic::set_hook(Box::new(panic_hook));
    exit_reason::init();

    startup.step_line("signals");
    // The status-file refresh of this step detects the node profile, which loads stream.conf first; the load
    // detects the profile too (for its replication defaults), so C parses [global] profile twice here.
    let mut stream_conf = load_stream_conf(&mut conf, &system);
    let detected = profile::detect(
        &mut conf.netdata,
        system.system_cpus,
        system.memory.total,
        stream_conf.is_parent,
        stream_conf.send.enabled,
    );
    status_file::set_profile(detected.bits());
    status_file::startup_step(Some("startup(signals)"));

    // signals_block_all_except_deadly(): every thread started from here on inherits the mask. The main thread waits
    // for the signals C handles; any other signal stays pending forever, so it is ignored.
    const DEADLY: [Signal; 8] = [
        Signal::SIGBUS,
        Signal::SIGSEGV,
        Signal::SIGFPE,
        Signal::SIGILL,
        Signal::SIGABRT,
        Signal::SIGSYS,
        Signal::SIGXCPU,
        Signal::SIGXFSZ,
    ];
    let mut blocked = SigSet::all();
    for deadly in DEADLY {
        blocked.remove(deadly);
    }
    if blocked.thread_block().is_err() {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "SIGNALS: cannot apply the default mask for signals"
        );
    }
    // nd_initialize_signals()'s sigaction() calls replace an ignore the launcher left (nohup, a shell's `&`), so the
    // children get the default action for each, as C's (D123); SIGCHLD stays as inherited, as C's (the spawn server
    // reaps the children, D140)
    for signal in [Signal::SIGINT, Signal::SIGQUIT, Signal::SIGTERM, Signal::SIGHUP, Signal::SIGUSR2] {
        if let Err(err) = netdata_agent_sys::default_dispositions(&[signal]) {
            nd_log!(Source::Daemon, Priority::Err, errno = err.raw_os_error().unwrap_or(0);
                "SIGNAL: Failed to change signal handler for: {signal}");
        }
    }
    // nd_initialize_signals(): the deadly signals recorded in the status file (D91)
    if let Err(errno) = netdata_agent_sys::install_deadly(&DEADLY, status_file::deadly_signal) {
        nd_log!(Source::Daemon, Priority::Err, errno = errno as i32;
            "SIGNALS: cannot install the deadly signal handler");
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

    // netdata_conf_section_global(): the hostname, nd_profile_setup() (the profile detected once more, then its
    // malloc settings) and [db]; then registry_init()'s configuration
    conf.section_global_hostname();
    status_file::set_host_prefix(&conf.host_prefix);
    let profile = profile::detect(
        &mut conf.netdata,
        system.system_cpus,
        system.memory.total,
        stream_conf.is_parent,
        stream_conf.send.enabled,
    );
    profile::setup_malloc(&mut conf.netdata, profile, system.system_cpus);
    stream_conf.set_sender_compression_levels(profile.stream_compression_fastest());
    // nd_profile_setup(): every profile starts with 3 tiers, until the dbengine reads [db]
    status_file::set_profile(profile.bits());
    status_file::set_db_tiers(3);
    let db = conf::section_db(&mut conf.netdata, system.page_size, &conf.dirs.cache);
    status_file::set_db_mode(db.mode as u8);
    // registry_init()'s configuration: the registry itself is not ported
    let registry_hostname = conf.section_registry();

    startup.step("run dir");
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

    startup.step("crash reports");
    status_file::check_crash(&mut conf.netdata, startup::analytics_enabled(&conf.dirs.user_config));
    startup.step("temp spawn server");
    // C's "init" server takes id 1 here (D134.3: not ported), so the plugins server keeps C's id
    netdata_agent_spawn::client::skip_server_id();
    startup.step("ssl");
    netdata_agent_tls::init();
    startup.step("environment for plugins");
    // set_environment_for_plugins_and_scripts(): an unusable required directory is C's fatal().
    if let Err((errno, message)) = conf.environment_for_plugins(db.update_every) {
        fatal!(errno = errno; "{message}");
    }

    startup.step("cd to user config dir");
    // cd into the user config dir, so plugins can use relative paths to their config files.
    if let Err(err) = std::env::set_current_dir(&conf.dirs.user_config) {
        fatal!(errno = netdata_agent_log::errno_of(&err); "Cannot cd to '{}'", conf.dirs.user_config);
    }

    startup.step("analytics");
    // get_system_timezone(). No thread has started yet, so setenv() cannot fail for lack of exclusivity; C does not
    // check it either.
    let _ = conf::set_timezone_env(&mut conf.netdata);
    let tz = timezone::system_timezone(&mut conf.netdata, std::path::Path::new("/"), netdata_agent_rrd::clock::now_realtime_s());

    startup.step("pulse");
    // the extended pulse charts are not ported (D80.4): the key is read and written as C does
    let _pulse_extended = conf::pulse_extended(&mut conf.netdata);
    startup.step("replication");
    startup.step("inflight functions");
    startup.step("silencers");
    conf.health_silencers_filename();

    startup.step("static threads");
    // the loop's pulse entries while the process has one thread (setenv); the other entries come with their plugins
    let pulse_enabled = conf::static_threads_pulse(&mut conf.netdata);
    startup.step("web server api");
    let web_security = conf::web_security(&mut conf.netdata, &conf.dirs.user_config);
    // nd_web_api_init(): the time-grouping limits, read before the listen sockets as in C.
    let grouping_windows = conf::grouping_windows(&mut conf.netdata);
    // web_server_threading_selection(): with `[web] mode = none` there is no web server at all.
    let web_enabled = conf::web_server_enabled(&mut conf.netdata);

    startup.step("web server sockets");
    let listeners = if web_enabled {
        listen::setup(&mut conf.netdata)
    } else {
        Vec::new()
    };
    if web_enabled && listeners.is_empty() {
        exit_reason::add(exit_reason::ALREADY_RUNNING);
        status_file::update_status(status_file::DaemonStatus::None);
        // web_server_listen_sockets_setup() clears errno first: the bind failure is on the listener's own record
        fatal!("Cannot setup listen port(s). Is Netdata already running?");
    }

    startup.step("sqlite");
    if netdata_agent_metadata::library::init().is_err() {
        fatal!("Failed to initialize sqlite library");
    }
    startup.step("ML");
    startup.step("resource limits");
    system::set_nofile_limit();

    startup.step("stop temporary spawn server");
    startup.step("become daemon");
    // become_daemon(): after the listeners (privileged ports) and before any thread starts.
    match daemon::become_daemon(
        dont_fork,
        &conf.user,
        pidfile.as_deref(),
        &mut conf.netdata,
        &conf.dirs,
    ) {
        daemon::Outcome::Continue => {
            if let Some(pidfile) = &pidfile {
                shutdown::set_pidfile(pidfile);
            }
        }
        daemon::Outcome::ExitParent => return 0,
    }
    startup.step("plugins spawn server");
    // a failure is retried by the first spawn, unnamed (netdata_main_spawn_server_init()'s result is ignored)
    let _ = netdata_agent_spawn::popen::main_server_init(Some("plugins"), true);
    startup.step("home");
    // After the user switch, while there is still one thread.
    let home = conf.section_home();

    startup.step("dyncfg");
    startup.step("threads after fork");
    // netdata_conf_reset_stack_size()
    conf::threads_set_stack_size(conf.threads.pthread_stack_size);
    startup.step("registry");
    startup.step("system info");
    let (mut system_info, build_system_info) =
        system_info::startup(&conf.primary_plugins_dir(), &conf.dirs.user_config);
    // rrdhost_create() of localhost: metric_correlations_version (D101.5; ml_capable stays 0 without ML)
    system_info.mc_version = 1;
    // set_late_analytics_variables() → analytics_build_info(): C fills BUILD_INFO once, here
    let build_info = buildinfo::BuildInfo::new(&buildinfo::Inputs {
        dirs: &conf.dirs,
        home: &home,
        system: &build_system_info,
        profile: profile.name(),
        parent: stream_conf.is_parent,
        child: stream_conf.send.enabled,
        memory: system::system_memory_cached(true),
    });
    startup.step("RRD structures");
    startup.step("commands liveness support");
    // the last single-threaded point: children get this environment and what `env::set` changes later (D134.4)
    netdata_agent_spawn::env::freeze();
    // libuv's thread pool, which runs the netdatacli commands
    let uv_pool = netdata_agent_evloop::work::WorkPool::new(
        conf.threads.libuv_worker_threads as usize,
        conf.threads.thread_stack_size,
    );
    command_server::init(&uv_pool, conf.threads.thread_stack_size);
    // rrd_init(): the metadata databases (a failure is fatal only when the configured mode is dbengine), the health
    // defaults, then localhost.
    let cache_dir = std::path::PathBuf::from(&conf.dirs.cache);
    let sqlite = conf::sqlite_settings(&mut conf.netdata);
    let meta = MetaDb::open(&cache_dir, &sqlite).map(Arc::new);
    if meta.is_none() {
        if db.mode == DbMode::Dbengine {
            fatal!("Failed to initialize SQLite");
        }
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Skipping SQLITE metadata initialization since memory mode is not dbengine"
        );
    }
    let context_db = ContextDb::open(&cache_dir, &sqlite).map(Arc::new);
    if context_db.is_none() {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Failed to initialize context metadata database"
        );
    }
    // the dbengine, when the configured mode or an enabled stream.conf receiver section stores in it
    // the deep pass's deadline: the engine's rotations arm it before the hosts exist
    let db_rotation = Arc::new(DbRotation::default());
    let dbengine = (db.mode == DbMode::Dbengine || stream_conf.config.stream_conf_needs_dbengine())
        .then(|| {
            dbengine::start(
                &mut conf,
                &db,
                profile == profile::Profile::Parent,
                &uv_pool,
                meta.clone(),
                &db_rotation,
            )
        });
    // metadata_sync_init()
    let metasync = match metasync::MetaSync::start(
        &uv_pool,
        conf.threads.cpus as usize,
        conf.threads.thread_stack_size,
    ) {
        Ok(metasync) => metasync,
        Err(err) => fatal!(
            "{}",
            netdata_agent_evloop::thread_create_failed("METASYNC", &err)
        ),
    };
    let health_defaults = conf.health_load_config_defaults();
    let health_enabled = health_defaults.enabled;
    // nd_profile.storage_tiers and multidb_ctx: every host's tiers
    let (dbengine, grouping, backfill, out_of_memory_protection, multidb_disk_quota_mb) =
        match dbengine {
            Some(started) => (
                Some(started.runtime),
                started.grouping,
                started.backfill,
                started.out_of_memory_protection,
                started.multidb_disk_quota_mb,
            ),
            // without the engine, C's globals keep their initial values
            None => (
                None,
                vec![1],
                Backfill::New,
                0,
                conf::DEFAULT_TIER_DISK_SPACE_MB as i32,
            ),
        };
    let storage = Arc::new(
        StorageLayout::new(
            dbengine
                .as_ref()
                .map(|dbengine| Arc::clone(dbengine.engine())),
        )
        .with_profile(grouping, i64::from(db.update_every))
        .with_db_rotation(db_rotation)
        .with_backfill(backfill),
    );
    let localhost = Host::with_storage(
        &machine_guid,
        true,
        HostInfo {
            hostname: conf.hostname.clone(),
            registry_hostname: registry_hostname.clone(),
            os: "linux".to_string(),
            timezone: tz.current().name.clone(),
            abbrev_timezone: tz.current().abbrev.clone(),
            utc_offset: tz.current().utc_offset,
            program_name: "netdata".to_string(),
            program_version: build::NETDATA_VERSION.to_string(),
            update_every: db.update_every,
            db_mode: db.mode,
            history_entries: align_entries_to_pagesize(
                db.mode,
                db.history_entries,
                system.page_size,
            ),
            // no health without a database
            health_enabled: health_enabled && db.mode != DbMode::None,
            system_info,
            replication_enabled: false,
            replication_period: 0,
            replication_step: 0,
            stream_send: StreamSend::new(
                stream_conf.send.enabled,
                &stream_conf.send.destination,
                &stream_conf.send.api_key,
                &stream_conf.send.send_charts_matching,
            ),
            cache_dir: Some(conf.dirs.cache.clone()),
        },
        &storage,
    );
    // sql_load_node_id() in rrdhost_create(), before the host's record
    let host_id = netdata_agent_text::parse::uuid_parse_flexi(machine_guid.as_bytes());
    if let (Some(meta), Some(host_id)) = (&meta, &host_id) {
        match meta.node_id(host_id) {
            NodeId::Set(id) => localhost.set_node_id(id),
            NodeId::Cleared => localhost.set_node_id([0; 16]),
            NodeId::Absent => {}
        }
    }
    let hosts = Arc::new(Hosts::with_storage(localhost, storage));
    // rrdset_free_obsolete_time_s and the host cleanup times, which the maintenance and its readers share
    hosts.storage().set_cleanup_times(db.cleanup);
    // what the status file refreshes from localhost's creation on
    status_file::set_hosts(&hosts);
    status_file::set_db_tiers(u8::try_from(hosts.storage().storage_tiers()).unwrap_or(u8::MAX));
    status_file::set_oom_protection(out_of_memory_protection);
    // store_host_info_and_metadata() at the end of rrdhost_create(localhost)
    match &meta {
        Some(meta) => meta_store::store_host_info_and_metadata(meta, hosts.localhost()),
        None => meta_store::store_localhost_without_database(hosts.localhost()),
    }
    // rrdhost_load_rrdcontext_data() of localhost and of every host created from now on, on the creating thread. The
    // databases are held weakly, so that they close at their shutdown step, and read through handles of the load's
    // own: C's shared connection does not wait for METASYNC's transactions, as a lock on ours would.
    if let Some(meta) = &meta {
        let (meta, context_db, queue) = (
            Arc::downgrade(meta),
            context_db.as_ref().map(Arc::downgrade),
            metasync.queue(),
        );
        hosts.set_context_loader(move |host| {
            let Some(meta) = meta.upgrade() else {
                return;
            };
            let context_db = context_db.as_ref().and_then(std::sync::Weak::upgrade);
            let cache_dir = meta.cache_dir();
            let meta_thread = netdata_agent_metadata::read::read_only(&MetaDb::path(cache_dir));
            let context_thread =
                netdata_agent_metadata::read::read_only(&ContextDb::path(cache_dir));
            let cleanup = |host_id, context| queue.ctx_host_cleanup(host_id, context);
            ctxload::load_host_contexts(
                host,
                &ctxload::Sources {
                    meta: &meta,
                    context_db: context_db.as_ref(),
                    meta_thread: meta_thread.as_ref(),
                    context_thread: context_thread.as_ref(),
                    cleanup: &cleanup,
                },
            );
        });
        hosts.load_contexts(hosts.localhost());
    }
    // rrd_init(): localhost's pulse state, once its contexts are loaded
    hosts.localhost().pulse_status(0);
    if let (Some(meta), Some(host_id)) = (&meta, &host_id) {
        meta.detect_machine_guid_change(host_id);
    }
    if let Some(meta) = &meta {
        metasync.set_writer(
            Arc::clone(meta),
            context_db.as_ref().map_or_else(Weak::new, Arc::downgrade),
            Arc::clone(&hosts),
            db.datafiles_present,
        );
    }
    // aclk_synchronization_init(): archived hosts take the default mode
    match &meta {
        Some(meta) => archived::load(
            meta,
            &hosts,
            &archived::Defaults {
                db_mode: db.mode,
                page_size: system.page_size,
            },
            Some(&metasync),
        ),
        None => archived::load_without_database(&hosts, Some(&metasync)),
    }
    // each started when a node is first assigned to it
    let stream_threads = netdata_agent_streaming::pins::threads_for(conf.threads.cpus);
    let stream_pins = Arc::new(std::sync::Mutex::new(netdata_agent_streaming::pins::Pins::new(stream_threads)));
    let stream_pool = {
        let pins = Arc::clone(&stream_pins);
        match Pool::spawn_lazy(
            stream_threads,
            conf.threads.thread_stack_size,
            |i| format!("STREAM[{i}]"),
            move |_| StreamWorker::new(Arc::clone(&pins), db.update_every),
        ) {
            Ok(pool) => pool,
            // D37: C carries on without the thread; a pool cannot, so the daemon exits after C's record
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, "{err}");
                return 1;
            }
        }
    };
    // stream_sender_structures_init() of localhost, with the connector its first collection starts
    let connector = {
        let info = hosts.localhost().info();
        netdata_agent_streaming::connector::Connector::new(
            netdata_agent_streaming::sender::Settings::of(&stream_conf.send),
            netdata_agent_streaming::parents::Local {
                host_id: netdata_agent_text::parse::uuid_parse_flexi(hosts.localhost().machine_guid().as_bytes())
                    .unwrap_or_default(),
                user_agent: format!("{}/{}", info.program_name, info.program_version),
                update_every: db.update_every,
            },
            hosts.localhost(),
            stream_pool.handle(),
            Arc::clone(&stream_pins),
            conf.threads.thread_stack_size,
        )
    };
    netdata_agent_streaming::sender::Sender::attach(hosts.localhost(), &connector);
    // stream_conf_is_parent() and stream_conf_is_child(), which PULSE reads
    let (stream_is_parent, stream_is_child) = (stream_conf.is_parent, stream_conf.send.enabled);
    // [db] replication threads, which REPLAY[1] starts
    let replication_threads = stream_conf.send.replication_threads.max(1) as usize;
    // netdata_ssl_validate_certificate_sender, which the web server's thread reads
    let senders_validate = stream_conf.send.ssl_validate_certificate;
    let receivers = Arc::new(Receivers::new(
        stream_conf,
        Arc::clone(&hosts),
        stream_pins,
        receiver::Defaults {
            db_mode: db.mode.name().to_string(),
            history: db.history_entries,
            health_enabled,
            update_every: db.update_every,
            gap_when_lost_iterations_above: db.gap_when_lost_iterations_above,
            page_size: system.page_size,
        },
        stream_pool.handle(),
        Arc::clone(&connector),
    ));
    startup.step("localhost labels");
    let plugins_dir = conf.primary_plugins_dir();
    host_labels::reload(&mut conf.netdata, &mut conf.cloud, &plugins_dir, &hosts);
    startup.step("saved bearer tokens");
    auth::init(&mut conf.netdata);
    bearer::init(
        &conf.dirs.varlib,
        meta_store::host_id(hosts.localhost()).unwrap_or_default(),
    );
    startup.step("claiming info");
    // load_claiming_state(), for an agent that is not claimed
    meta_store::invalidate_node_instances(meta.as_deref(), hosts.localhost());
    if let Some(id) = meta_store::host_id(hosts.localhost()) {
        metasync.queue().store_claim_id(meta.clone(), id);
    }
    startup.step("static threads");
    // Flood protection back on, the agent event medians cached (before this start's event), then
    // netdata_conf_section_web() just before C starts its static threads, the web server among them, which then reads
    // its thread count.
    netdata_agent_log::limits_reset();
    let medians = match &meta {
        Some(meta) => (
            meta.agent_event_median(EventKind::StartTime),
            meta.agent_event_median(EventKind::ShutdownTime),
        ),
        None => {
            meta_store::no_database("get_agent_event_time_median");
            meta_store::no_database("get_agent_event_time_median");
            (0, 0)
        }
    };
    netdata_agent_rrd::host::set_agent_event_medians_us(medians.0, medians.1);
    let web = conf.section_web();
    // The web server thread reads its sizing only when it runs.
    // socket_listen_main_static_threaded(): its thread, WEB[1], starts with the TLS context, before its sizing
    let tls_context = if web_enabled {
        web_tls_context(&mut conf.netdata, &web_security, senders_validate)
    } else {
        None
    };
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
        tls: tls_context,
        acl: web.acl,
        // C keeps these as int seconds; a negative value disables the check as 0 does.
        first_request_timeout_s: web.first_request_timeout_s.max(0) as u64,
        idle_timeout_s: web.disconnect_idle_after_s.max(0) as u64,
        web_dir: conf.dirs.web.clone(),
        hosts: Arc::clone(&hosts),
        grouping_windows,
        release_channel,
        // Every startup read is done: from here on netdata.conf is read and dumped under its lock.
        netdata_conf: std::sync::Mutex::new(std::mem::take(&mut conf.netdata)),
        custom_dashboard_info: Default::default(),
        ready: commands::is_ready,
        exiting: netdata_agent_sys::exit::initiated,
        multidb_disk_quota_mb: multidb_disk_quota_mb as u64,
        page_cache_mb: db.page_cache_mb as u64,
        history_entries: db.history_entries,
        build_info,
        cloud_conf_file: conf.cloud_conf_filename(),
        cloud_conf: std::sync::Mutex::new(std::mem::take(&mut conf.cloud)),
    });
    // what a parent's NODE_ID may change here: the agent is never claimed (D61.3), so the Cloud URL follows the parent
    connector.set_env(netdata_agent_streaming::connector::Env {
        claimed: Box::new(|| false),
        aclk_online: Box::new(|| false),
        set_cloud_url: {
            let shared = Arc::clone(&shared);
            Box::new(move |url| {
                // cloud_config_url_set(): only a different URL is stored
                let mut cloud = shared.cloud_conf();
                if cloud::url(&mut cloud) != url.as_bytes() {
                    cloud.set(netdata_agent_inicfg::SECTION_GLOBAL, "url", url);
                }
            })
        },
        cloud_url: {
            let shared = Arc::clone(&shared);
            Box::new(move || String::from_utf8_lossy(&cloud::url(&mut shared.cloud_conf())).into_owned())
        },
    });
    // HEALTH, before PULSE in C's table, which starts it with health off too (D93.2); C carries on without it
    let health_thread = match health::spawn(
        Arc::clone(hosts.storage()),
        conf.threads.thread_stack_size,
        health_defaults.run_at_least_every_s,
        health_defaults.postpone_s,
    ) {
        Ok(thread) => Some(thread),
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, "{err}");
            None
        }
    };
    // PULSE, a static thread started before the web server's as in C's table; C carries on without it
    let pulse_thread = if pulse_enabled {
        let localhost_update_every = i64::from(db.update_every);
        let update_every = {
            let shared = Arc::clone(&shared);
            move || {
                let mut c = shared
                    .netdata_conf
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                conf::pulse_update_every(&mut c, localhost_update_every)
            }
        };
        let settings = netdata_agent_pulse::Settings {
            gap_when_lost_iterations_above: db.gap_when_lost_iterations_above,
            page_size: system.page_size,
            parents: netdata_agent_pulse::Gates {
                is_parent: profile == profile::Profile::Parent,
                stream_is_parent,
                is_child: stream_is_child,
            },
            out_of_memory_protection,
            system_memory: pulse::system_memory_available,
        };
        match pulse::spawn(
            Arc::clone(&hosts),
            conf.threads.thread_stack_size,
            update_every,
            settings,
            tz,
        ) {
            Ok(thread) => Some(thread),
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, "{err}");
                None
            }
        }
    } else {
        None
    };
    // PLUGINSD, after PULSE in C's table; C carries on without it
    let pluginsd = match plugins_d::spawn(
        Arc::clone(hosts.localhost()),
        Arc::clone(&shared),
        plugins_d::Settings {
            dirs: conf.dirs.plugins.clone(),
            stack_size: conf.threads.thread_stack_size,
            update_every: db.update_every,
            parser: netdata_agent_ingest::Config {
                capabilities: 0,
                update_every: db.update_every,
                page_size: system.page_size,
                now: netdata_agent_rrd::collection::now_realtime_timeval,
                gap_when_lost_iterations_above: db.gap_when_lost_iterations_above,
            },
        },
    ) {
        Ok(thread) => Some(thread),
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, "{err}");
            None
        }
    };
    // One descriptor per listener, shared by every web worker.
    let listeners: Arc<[server::WebListener]> = listeners
        .into_iter()
        .map(server::WebListener::new)
        .collect();
    let pool = if web_enabled {
        match Pool::spawn(
            web_server_threads,
            conf.threads.thread_stack_size,
            |i| format!("WEB[{}]", i + 1),
            |_| {
                server::WebWorker::new(
                    Arc::clone(&listeners),
                    max_sockets,
                    Arc::clone(&shared),
                    Arc::clone(&receivers),
                )
            },
        ) {
            Ok(pool) => Some(pool),
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, "{err}");
                return 1;
            }
        }
    } else {
        None
    };
    // The workers hold the listeners from here on; they close when the last one stops.
    drop(listeners);
    // the deep pass's SQL deletes; the database is held weakly, so that it closes at its shutdown step
    let delete_context = {
        let (context_db, queue) = (
            context_db.as_ref().map_or_else(Weak::new, Arc::downgrade),
            metasync.queue(),
        );
        move |host: &Host, context: &str, version: u64| {
            let cleanup = |host_id, context| queue.ctx_host_cleanup(host_id, context);
            ctxload::delete_context(
                host,
                context_db.upgrade().as_ref(),
                context,
                version,
                &cleanup,
            );
        }
    };
    // the extreme cardinality protection's settings, which the thread reads from netdata.conf as it starts
    let settings = {
        let (shared, hosts) = (Arc::clone(&shared), Arc::clone(&hosts));
        let default_on = hosts.storage().storage_tiers() > 1 && db.mode == DbMode::Dbengine;
        move || {
            let mut c = shared
                .netdata_conf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let (enabled, keep, min) = rrdcontext::extreme_cardinality_settings(&mut c, default_on);
            hosts
                .storage()
                .extreme_cardinality()
                .configure(enabled, keep, min);
        }
    };
    let contexts_worker = match rrdcontext::spawn(
        Arc::clone(&hosts),
        conf.threads.thread_stack_size,
        settings,
        delete_context,
    ) {
        Ok(worker) => worker,
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, "{err}");
            return 1;
        }
    };
    // REPLAY[1], after RRDCONTEXT in C's table; C carries on without it
    let replication = match netdata_agent_streaming::replication::ReplicationThreads::spawn(
        replication_threads,
        conf.threads.thread_stack_size,
        Arc::clone(connector.replication()),
        std::time::Duration::from_secs(db.update_every.max(1) as u64),
        {
            let hosts = Arc::clone(&hosts);
            Arc::new(move || hosts.all())
        },
    ) {
        Ok(threads) => Some(threads),
        Err(err) => {
            nd_log!(Source::Daemon, Priority::Err, "{err}");
            None
        }
    };
    // BACKFILL: a parent's static thread (the profile alone decides, as C's enable_routine)
    let backfill_thread = if profile == profile::Profile::Parent {
        match backfill::Thread::spawn(
            Arc::clone(hosts.storage()),
            conf.threads.cpus as usize,
            conf.threads.thread_stack_size,
        ) {
            Ok(thread) => Some(thread),
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, "{err}");
                None
            }
        }
    } else {
        None
    };
    // The exit's work is registered before the command server accepts every command, so an exit it starts (a
    // netdatacli shutdown-agent) stops what runs, as C's globals do. Main keeps its own references for the rest of the
    // startup and drops them before it waits for signals.
    let metaqueue = metasync.queue();
    // metaqueue_delete_dimension_uuid() of every freed dimension that leaves no data behind
    hosts.storage().set_freed_dimension_hook({
        let metaqueue = metaqueue.clone();
        move |uuid| metaqueue.delete_dimension(uuid)
    });
    let meta_main = meta.clone();
    let engine_main = dbengine
        .as_ref()
        .map(|dbengine| Arc::clone(dbengine.engine()));
    let mut pool = pool;
    let mut stream_pool = Some(stream_pool);
    let mut contexts_worker = Some(contexts_worker);
    let mut pulse_thread = pulse_thread;
    let mut pluginsd = pluginsd;
    let mut health_thread = health_thread;
    let mut backfill_thread = backfill_thread;
    let mut replication = replication;
    let mut meta = meta;
    let mut context_db = context_db;
    let mut metasync = Some(metasync);
    let mut dbengine = dbengine;
    let mut shutdown_started_ut = 0;
    shutdown::set_work(Box::new(move |step, normal| match step {
        // rrdeng_quiesce_all() and a first flush of the dirty pages as the watcher starts, unless the exit is abnormal
        0 => {
            shutdown_started_ut = startup::now_ut();
            if let (Some(dbengine), true) = (&dbengine, normal) {
                dbengine.quiesce();
                dbengine.flush_everything(false, false, true);
            }
        }
        // the dirty pages again once the collectors and streams stopped
        shutdown::STOP_REPLICATION => {
            if let (Some(dbengine), true) = (&dbengine, normal) {
                dbengine.flush_everything(false, false, true);
            }
            // service_wait_exit(SERVICE_REPLICATION, 5 s): they left on their own when the exit started (D110)
            if let Some(threads) = replication.take() {
                threads.join_within(shutdown::REPLICATION_WAIT);
            }
        }
        // the exporters, HEALTH and the web servers under one service wait: each leaves on its own once the exit
        // started (D110), so the wait only waits
        shutdown::STOP_WEB_SERVERS => {
            let deadline = std::time::Instant::now() + shutdown::WEB_SERVERS_WAIT;
            if let Some(thread) = health_thread.take() {
                thread.join_within(shutdown::WEB_SERVERS_WAIT);
            }
            if let Some(pool) = pool.take() {
                let _ = pool.join_within(deadline.saturating_duration_since(std::time::Instant::now()));
            }
        }
        // the collectors (PULSE, PLUGINSD and its plugin threads), the stream threads and the BACKFILL threads under one
        // service wait
        shutdown::STOP_STREAMING => {
            // stream_threads_cancel(): the connector's attempt in progress stops
            connector.cancel();
            let deadline = std::time::Instant::now() + shutdown::STREAMING_WAIT;
            // every collector thread cancelled first; PLUGINSD stopped its plugins when the exit started
            pluginsd = pluginsd.take().and_then(|pluginsd| pluginsd.stop_by(deadline));
            if let Some(thread) = pulse_thread.take() {
                thread.join_within(deadline.saturating_duration_since(std::time::Instant::now()));
            }
            if let Some(pool) = stream_pool.take() {
                let _ = pool.join_within(deadline.saturating_duration_since(std::time::Instant::now()));
            }
            backfill_thread = backfill_thread
                .take()
                .and_then(|thread| thread.stop_by(deadline));
            // service_signal_exit(SERVICE_STREAMING_CONNECTOR): its thread removes the queued hosts and ends
            connector.signal_exit();
        }
        shutdown::CANCEL_MAIN_THREADS => {
            if let Some(thread) = &backfill_thread {
                thread.cancel();
            }
        }
        shutdown::STOP_CONTEXT => {
            if let Some(worker) = contexts_worker.take() {
                worker.join_within(shutdown::CONTEXT_WAIT);
            }
        }
        // service_wait_exit(~0, 20 s): the connector, whose last passes follow the exit's start, and PLUGINSD when a
        // plugin thread was still stopping at the streaming step
        shutdown::STOP_REMAINING_THREADS => {
            let deadline = std::time::Instant::now() + shutdown::REMAINING_WAIT;
            connector.join_within(shutdown::REMAINING_WAIT);
            if let Some(pluginsd) = pluginsd.take() {
                let _ = pluginsd.stop_by(deadline);
            }
        }
        // rrd_finalize_collection_for_all_hosts(), which an abnormal exit skips
        shutdown::STOP_COLLECTION if normal => {
            for host in hosts.all() {
                host.finalize_collection();
            }
        }
        // everything, hot pages too, once the collectors finished
        shutdown::WAIT_DBENGINE_COLLECTORS if normal => {
            if let Some(dbengine) = &dbengine {
                dbengine.flush_everything(true, true, false);
            }
        }
        shutdown::STOP_DBENGINE_TIERS => {
            if let Some(dbengine) = dbengine.take() {
                if normal {
                    dbengine.exit();
                } else {
                    // an abnormal exit never joins the engine's threads, as C
                    std::mem::forget(dbengine);
                }
            }
        }
        shutdown::STOP_METASYNC_THREADS => {
            if let Some(metasync) = metasync.take() {
                if normal {
                    metasync.shutdown();
                } else {
                    // an abnormal exit leaves the thread running until the process ends, as C does
                    std::mem::forget(metasync);
                }
            }
        }
        // the shutdown time goes into the agent event log, unless the exit is abnormal
        shutdown::JOIN_STATIC_THREADS if normal => {
            let took = startup::now_ut().saturating_sub(shutdown_started_ut) as i64;
            match &meta {
                Some(meta) => {
                    meta.add_agent_event(EventKind::ShutdownTime, build::NETDATA_VERSION, took)
                }
                None => meta_store::no_database("add_agent_event"),
            }
        }
        // sqlite_close_databases(): an abnormal exit leaves them open
        shutdown::CLOSE_SQL_DATABASES if normal => {
            if let Some(context_db) = context_db.take().and_then(Arc::into_inner) {
                context_db.close();
            }
            if let Some(meta) = meta.take().and_then(Arc::into_inner) {
                meta.close();
            }
        }
        _ => {}
    }));
    startup.step("commands full API");
    commands::set_context(commands::Ctx {
        shared: Arc::clone(&shared),
        plugins_dir: conf.primary_plugins_dir(),
        meta: meta_main.as_ref().map(Arc::downgrade).unwrap_or_default(),
        metaqueue,
    });
    command_server::init(&uv_pool, conf.threads.thread_stack_size);
    startup.step("agent start timings");
    let elapsed_us = startup.elapsed_us();
    match &meta_main {
        Some(meta) => meta.add_agent_event(
            EventKind::StartTime,
            build::NETDATA_VERSION,
            elapsed_us as i64,
        ),
        None => meta_store::no_database("add_agent_event"),
    }
    startup.completed(elapsed_us, medians.0);
    if let Some(meta) = &meta_main {
        meta.cleanup_agent_event_log();
    }
    commands::set_ready();
    // The ANALYTICS thread is not ported: nothing is sent either way.
    startup.step(if startup::analytics_enabled(&conf.dirs.user_config) {
        "anonymous analytics"
    } else {
        "anonymous analytics (disabled)"
    });
    startup.step("mrg cleanup");
    if let Some(engine) = engine_main {
        engine.mrg.prepopulate_cleanup();
    }
    drop(meta_main);
    startup.step("done");
    // netdata_exit_fatal(): a fatal() from here on runs the exit sequence, as an abnormal exit
    netdata_agent_log::register_fatal_final_callback(shutdown::exit_fatal);
    status_file::startup_step(None);
    status_file::update_status(status_file::DaemonStatus::Running);

    signal_loop(&handled)
}

/// A panic as `fatal()`: at the panic's file and line, function "panic", its message; the fatal path then records it in
/// the status file and exits.
fn panic_hook(info: &std::panic::PanicHookInfo<'_>) {
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Box<dyn Any>".to_string());
    let (file, line) = info.location().map_or(("", 0), |l| (l.file(), l.line()));
    // the default hook's line, raw on stderr (review R34): it shows even when the daemon log is off
    let thread = std::thread::current();
    let at = info.location().map(|l| l.to_string()).unwrap_or_default();
    let text = format!("\nthread '{}' panicked at {at}:\n{message}\n", thread.name().unwrap_or("<unnamed>"));
    let _ = nix::unistd::write(std::io::stderr(), text.as_bytes());
    netdata_agent_log::fatal_at(file, line, "panic", format_args!("{message}"))
}

/// `threshold_trigger_smaller()`: true once when `free` falls under `threshold`, again only after it rose to
/// `threshold + hysteresis`.
fn threshold_trigger_smaller(last: &mut bool, threshold: f64, hysteresis: f64, free: f64) -> bool {
    let triggered = *last;
    if free < threshold {
        *last = true;
    }
    if free >= threshold + hysteresis {
        *last = false;
    }
    !triggered && *last
}

/// `nd_process_signals()`: the status file saved every 15 minutes and whenever free memory falls under 10, 5 or 1%,
/// C's `poll()` of 13.379 s, then `process_triggered_signals()`. Returns when this thread ran the exit sequence.
fn signal_loop(handled: &SigSet) -> i32 {
    use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
    use nix::sys::signalfd::{SfdFlags, SignalFd};
    use std::os::fd::AsFd;

    const SAVE_EVERY_UT: u64 = 15 * 60 * 1_000_000;
    // C's handler interrupts its poll(), so the SIGNAL records carry EINTR
    const EINTR: i32 = nix::errno::Errno::EINTR as i32;
    let signals = match SignalFd::with_flags(handled, SfdFlags::SFD_NONBLOCK | SfdFlags::SFD_CLOEXEC) {
        Ok(fd) => fd,
        Err(errno) => fatal!(errno = errno as i32; "SIGNAL: cannot receive the signals"),
    };
    let mut last_update_mt = startup::now_ut();
    let mut triggered = [false; 3];
    loop {
        // os_system_memory_available_percent(os_system_memory(false)); C's || stops at the first trigger
        let memory = system::system_memory_cached(false);
        let free = if memory.total > 0 {
            100.0 * memory.available as f64 / memory.total as f64
        } else {
            100.0
        };
        let [t1, t5, t10] = &mut triggered;
        let save_again = threshold_trigger_smaller(t1, 1.0, 1.0, free)
            || threshold_trigger_smaller(t5, 5.0, 1.0, free)
            || threshold_trigger_smaller(t10, 10.0, 1.0, free);
        // after a threshold's save C's `last += 15 min` passes now, and the difference wraps: from then on every loop
        // saves (DEFECTS), as here
        if startup::now_ut().wrapping_sub(last_update_mt) >= SAVE_EVERY_UT || save_again {
            status_file::update_status(status_file::DaemonStatus::None);
            last_update_mt += SAVE_EVERY_UT;
        }
        let _ = poll(&mut [PollFd::new(signals.as_fd(), PollFlags::POLLIN)], PollTimeout::from(13_379u16));
        while let Ok(Some(info)) = signals.read_signal() {
            let reason = match Signal::try_from(info.ssi_signo as i32) {
                Ok(Signal::SIGINT) => ("SIGINT", exit_reason::SIGINT),
                Ok(Signal::SIGQUIT) => ("SIGQUIT", exit_reason::SIGQUIT),
                Ok(Signal::SIGTERM) => ("SIGTERM", exit_reason::SIGTERM),
                Ok(signal @ (Signal::SIGHUP | Signal::SIGUSR2)) if shutdown::exiting() => {
                    nd_log!(Source::Daemon, Priority::Info, errno = EINTR;
                        "SIGNAL: Received {}. Ignoring it, as we are exiting...", signal.as_str());
                    continue;
                }
                // through the command server's locks and gating: nothing runs when it did not start
                Ok(Signal::SIGHUP) => {
                    netdata_agent_log::limits_unlimited();
                    nd_log!(Source::Daemon, Priority::Info, errno = EINTR;
                        "SIGNAL: Received SIGHUP. Reopening all log files...");
                    netdata_agent_log::limits_reset();
                    commands::execute(commands::REOPEN_LOGS, b"");
                    continue;
                }
                Ok(Signal::SIGUSR2) => {
                    netdata_agent_log::limits_unlimited();
                    nd_log!(Source::Daemon, Priority::Info, errno = EINTR;
                        "SIGNAL: Received SIGUSR2. Reloading HEALTH configuration...");
                    netdata_agent_log::limits_reset();
                    commands::execute(commands::RELOAD_HEALTH, b"");
                    continue;
                }
                // SIGPIPE is ignored.
                Ok(_) | Err(_) => continue,
            };
            netdata_agent_log::limits_unlimited();
            nd_log!(Source::Daemon, Priority::Info, errno = EINTR;
                "SIGNAL: Received {}. Cleaning up to exit...", reason.0);
            command_server::exit();
            // a later exit (the command server's, a fatal's) leaves this thread to its signals, as in C
            if shutdown::exit_gracefully(reason.1) {
                return 0;
            }
        }
    }
}

/// The start of `socket_listen_main_static_threaded()`, in a thread named as C's (`WEB[1]`) so its records carry that
/// name: `[web] ssl skip certificate verification`, C's notice of the streaming senders' skip (C checks the senders'
/// flag here), then the web server's TLS context.
fn web_tls_context(
    config: &mut netdata_agent_inicfg::Config,
    security: &conf::WebSecurity,
    senders_validate: bool,
) -> Option<netdata_agent_tls::SslContext> {
    std::thread::scope(|scope| {
        let thread = std::thread::Builder::new()
            .name("WEB[1]".into())
            .spawn_scoped(scope, || {
                let skip = config.get_boolean(
                    netdata_agent_inicfg::SECTION_WEB,
                    "ssl skip certificate verification",
                    false,
                );
                if !senders_validate {
                    netdata_log_info!("SSL: web server will skip SSL certificates verification.");
                }
                netdata_agent_tls::web_server_context(&netdata_agent_tls::ServerConfig {
                    key: &security.key,
                    certificate: &security.certificate,
                    tls_version: &security.tls_version,
                    ciphers: &security.ciphers,
                    skip_verification: skip,
                })
            });
        thread.ok().and_then(|t| t.join().ok()).flatten()
    })
}

/// `stream_conf_load()`, which also detects the node profile for its replication defaults.
fn load_stream_conf(conf: &mut Conf, system: &system::Resources) -> StreamConf {
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
    stream_conf
}

/// The allocator that tells the status file when memory runs out (D87 F5, D91.1).
#[global_allocator]
static ALLOC: netdata_agent_sys::Alloc = netdata_agent_sys::Alloc;

fn main() -> ExitCode {
    // this binary is also its spawn server: re-executed with the marker, it serves and exits here (D12, D140)
    netdata_agent_spawn::server::run_if_requested();
    use std::os::unix::ffi::OsStringExt;
    ExitCode::from(run(std::env::args_os().map(OsStringExt::into_vec).collect()) as u8)
}
