//! The netdata agent daemon. The startup sequence follows `netdata_main()` in `src/daemon/main.c`, step by step, for
//! the subsystems ported so far (`agent/plan.md` in the status repository tracks the rest).

#![forbid(unsafe_code)]

mod access_log;
mod acl;
mod api;
mod archived;
mod build;
mod cli;
mod cloud_proxy;
mod command_server;
mod commands;
mod conf;
mod contexts_v2;
mod daemon;
mod data;
mod dbengine;
mod guid;
mod host_labels;
mod listen;
mod meta_store;
mod metasync;
mod profile;
mod router;
mod rrdcontext;
mod server;
mod shutdown;
mod spawn;
mod startup;
mod static_file;
mod stream_info;
mod system;
mod system_info;
mod timezone;
mod v1_charts;
mod v1_contexts;

use netdata_agent_metadata::open::{ContextDb, MetaDb};
use netdata_agent_metadata::read::{EventKind, NodeId};

use netdata_agent_log::{Priority, Source, fatal, nd_log};
use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use netdata_agent_evloop::Pool;
use netdata_agent_inicfg::{SECTION_GLOBAL, SECTION_LOGS, SECTION_WEB};
use netdata_agent_rrd::host::{Host, HostInfo, Hosts, StreamSend};
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
    netdata_agent_rrd::host::set_netdata_start_time(
        netdata_agent_rrd::collection::now_realtime_timeval().0,
    );
    let mut startup = startup::Startup::new();
    // C's constructor-time invocation id, then `program_name`; until nd_log_initialize() records go to stderr.
    netdata_agent_log::init_invocation_id();
    netdata_agent_log::set_program_name("netdata");
    netdata_agent_log::set_default_log_dir(build::LOG_DIR);
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

    let machine_guid = guid::machine_guid_get(&conf.dirs.varlib);

    startup.step("signals");
    // The status-file refresh of this step line detects the node profile, which loads stream.conf first; the load
    // detects the profile too (for its replication defaults), so C parses [global] profile twice here.
    let stream_conf = load_stream_conf(&mut conf, &system);
    profile::detect(
        &mut conf.netdata,
        system.system_cpus,
        system.memory.total,
        stream_conf.is_parent,
        stream_conf.send.enabled,
    );

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

    // netdata_conf_section_global(): the hostname, nd_profile_setup() (the profile detected once more, then its
    // malloc settings) and [db]. registry_init() follows in C (D42).
    conf.section_global_hostname();
    let profile = profile::detect(
        &mut conf.netdata,
        system.system_cpus,
        system.memory.total,
        stream_conf.is_parent,
        stream_conf.send.enabled,
    );
    profile::setup_malloc(&mut conf.netdata, profile, system.system_cpus);
    let db = conf::section_db(&mut conf.netdata, system.page_size, &conf.dirs.cache);

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
    startup.step("temp spawn server");
    startup.step("ssl");
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
    let tz = timezone::system_timezone(&mut conf.netdata, std::path::Path::new("/"), server::now());

    startup.step("pulse");
    startup.step("replication");
    startup.step("inflight functions");
    startup.step("silencers");
    conf.health_silencers_filename();

    startup.step("static threads");
    startup.step("web server api");
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
    startup.step("home");
    // After the user switch, while there is still one thread.
    conf.section_home();

    startup.step("dyncfg");
    startup.step("threads after fork");
    // netdata_conf_reset_stack_size()
    conf::threads_set_stack_size(conf.threads.pthread_stack_size);
    startup.step("registry");
    startup.step("system info");
    let system_info = system_info::startup(&conf.primary_plugins_dir(), &conf.dirs.user_config);
    startup.step("RRD structures");
    startup.step("commands liveness support");
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
    let context_db = ContextDb::open(&cache_dir, &sqlite);
    if context_db.is_none() {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Failed to initialize context metadata database"
        );
    }
    // the dbengine, when the configured mode or an enabled stream.conf receiver section stores in it
    let mut stream_conf = stream_conf;
    let dbengine = (db.mode == DbMode::Dbengine || stream_conf.config.stream_conf_needs_dbengine())
        .then(|| {
            dbengine::start(
                &mut conf,
                &db,
                profile == profile::Profile::Parent,
                &uv_pool,
                meta.clone(),
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
    let health_enabled = conf.health_load_config_defaults();
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
            ),
            cache_dir: Some(conf.dirs.cache.clone()),
        },
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
    let hosts = Arc::new(Hosts::new(localhost));
    // store_host_info_and_metadata() at the end of rrdhost_create(localhost)
    match &meta {
        Some(meta) => meta_store::store_host_info_and_metadata(meta, hosts.localhost()),
        None => meta_store::store_localhost_without_database(hosts.localhost()),
    }
    if let (Some(meta), Some(host_id)) = (&meta, &host_id) {
        meta.detect_machine_guid_change(host_id);
    }
    if let Some(meta) = &meta {
        metasync.set_writer(Arc::clone(meta), Arc::clone(&hosts), db.datafiles_present);
    }
    // aclk_synchronization_init(): archived hosts take the default mode after C's fallback, as children do
    match &meta {
        Some(meta) => archived::load(
            meta,
            &hosts,
            &archived::Defaults {
                db_mode: if db.mode == DbMode::Dbengine {
                    DbMode::Alloc
                } else {
                    db.mode
                },
                page_size: system.page_size,
                free_ephemeral_time_s: db.free_ephemeral_time_s,
            },
            Some(&metasync),
        ),
        None => archived::load_without_database(&hosts, Some(&metasync)),
    }
    // stream_thread_get_unsafe(): one thread per core but one, 4..=2048, each started when a node is first assigned
    // to it.
    let stream_threads = (conf.threads.cpus - 1).clamp(4, 2048) as usize;
    let stream_load: Arc<std::sync::Mutex<Vec<usize>>> = Arc::default();
    let stream_pool = {
        let load = Arc::clone(&stream_load);
        match Pool::spawn_lazy(
            stream_threads,
            conf.threads.thread_stack_size,
            |i| format!("STREAM[{i}]"),
            move |_| StreamWorker::new(Arc::clone(&load), db.update_every),
        ) {
            Ok(pool) => pool,
            // D37: C carries on without the thread; a pool cannot, so the daemon exits after C's record
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, "{err}");
                return 1;
            }
        }
    };
    let receivers = Arc::new(Receivers::new(
        stream_conf,
        Arc::clone(&hosts),
        stream_load,
        receiver::Defaults {
            // children stay in alloc memory until the dbengine write path (D62.2)
            db_mode: if db.mode == DbMode::Dbengine {
                DbMode::Alloc
            } else {
                db.mode
            }
            .name()
            .to_string(),
            history: db.history_entries,
            health_enabled,
            update_every: db.update_every,
            gap_when_lost_iterations_above: db.gap_when_lost_iterations_above,
            page_size: system.page_size,
        },
        stream_pool.handle(),
    ));
    startup.step("localhost labels");
    let plugins_dir = conf.primary_plugins_dir();
    host_labels::reload(&mut conf.netdata, &mut conf.cloud, &plugins_dir, &hosts);
    startup.step("saved bearer tokens");
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
        web_dir: conf.dirs.web.clone(),
        hosts: Arc::clone(&hosts),
        grouping_windows,
        release_channel,
        // Every startup read is done: from here on netdata.conf is read and dumped under its lock.
        netdata_conf: std::sync::Mutex::new(std::mem::take(&mut conf.netdata)),
        custom_dashboard_info: Default::default(),
    });
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
    let contexts_worker =
        match rrdcontext::Worker::spawn(Arc::clone(&hosts), conf.threads.thread_stack_size) {
            Ok(worker) => worker,
            Err(err) => {
                nd_log!(Source::Daemon, Priority::Err, "{err}");
                return 1;
            }
        };
    startup.step("commands full API");
    commands::set_context(commands::Ctx {
        shared: Arc::clone(&shared),
        cloud_conf_file: conf.cloud_conf_filename(),
        plugins_dir: conf.primary_plugins_dir(),
        cloud: std::sync::Mutex::new(std::mem::take(&mut conf.cloud)),
        meta: meta.as_ref().map(Arc::downgrade).unwrap_or_default(),
        metaqueue: metasync.queue(),
    });
    command_server::init(&uv_pool, conf.threads.thread_stack_size);
    startup.step("agent start timings");
    let elapsed_us = startup.elapsed_us();
    match &meta {
        Some(meta) => meta.add_agent_event(
            EventKind::StartTime,
            build::NETDATA_VERSION,
            elapsed_us as i64,
        ),
        None => meta_store::no_database("add_agent_event"),
    }
    startup.completed(elapsed_us, medians.0);
    if let Some(meta) = &meta {
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
    if let Some(dbengine) = &dbengine {
        dbengine.prepopulate_cleanup();
    }
    startup.step("done");
    let mut pool = pool;
    let mut stream_pool = Some(stream_pool);
    let mut contexts_worker = Some(contexts_worker);
    let mut meta = meta;
    let mut context_db = context_db;
    let mut metasync = Some(metasync);
    let mut dbengine = dbengine;
    let mut shutdown_started_ut = 0;
    shutdown::set_work(Box::new(move |step, normal| match step {
        // rrdeng_quiesce_all() as the watcher starts, unless the exit is abnormal
        0 => {
            shutdown_started_ut = startup::now_ut();
            if let (Some(dbengine), true) = (&dbengine, normal) {
                dbengine.quiesce();
            }
        }
        shutdown::STOP_WEB_SERVERS => {
            if let Some(pool) = pool.take() {
                let _ = pool.stop_within(Some(shutdown::WEB_SERVERS_WAIT));
            }
        }
        shutdown::STOP_STREAMING => {
            if let Some(pool) = stream_pool.take() {
                let _ = pool.stop_within(Some(shutdown::STREAMING_WAIT));
            }
        }
        shutdown::STOP_CONTEXT => {
            if let Some(worker) = contexts_worker.take() {
                worker.stop_within(shutdown::CONTEXT_WAIT);
            }
        }
        // rrd_finalize_collection_for_all_hosts(), which an abnormal exit skips
        shutdown::STOP_COLLECTION if normal => {
            for host in hosts.all() {
                let hostname = host.hostname();
                let _frame = netdata_agent_log::push(vec![(
                    netdata_agent_log::Field::NidlNode,
                    netdata_agent_log::Value::txt(hostname.as_str()),
                )]);
                nd_log!(
                    Source::Daemon,
                    Priority::Debug,
                    "RRD: 'host:{hostname}' stopping data collection..."
                );
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
            if let Some(context_db) = context_db.take() {
                context_db.close();
            }
            if let Some(meta) = meta.take().and_then(Arc::into_inner) {
                meta.close();
            }
        }
        _ => {}
    }));
    // netdata_exit_fatal(): a fatal() from here on runs the exit sequence, as an abnormal exit
    netdata_agent_log::register_fatal_final_callback(shutdown::exit_fatal);

    // process_triggered_signals(). C's handler interrupts its poll(), so the SIGNAL records carry EINTR.
    const EINTR: i32 = nix::errno::Errno::EINTR as i32;
    let reason = loop {
        let (name, reason) = match handled.wait() {
            Ok(Signal::SIGINT) => ("SIGINT", "signal-interrupt"),
            Ok(Signal::SIGQUIT) => ("SIGQUIT", "signal-quit"),
            Ok(Signal::SIGTERM) => ("SIGTERM", "signal-terminate"),
            // an exit started on another thread (a fatal): C's handler ignores the reload signals
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
            "SIGNAL: Received {name}. Cleaning up to exit...");
        command_server::exit();
        break reason;
    };

    shutdown::exit_gracefully(reason);
    0
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

fn main() -> ExitCode {
    use std::os::unix::ffi::OsStringExt;
    ExitCode::from(run(std::env::args_os().map(OsStringExt::into_vec).collect()) as u8)
}
