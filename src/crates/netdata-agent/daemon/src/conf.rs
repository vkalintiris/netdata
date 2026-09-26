//! Configuration loading and the section readers, ported from `src/daemon/config/` and the `[directories]` reader
//! in `src/libnetdata/runtime-paths/runtime-paths.c`. Each function mirrors the C function of the same name and is
//! called from the same point of the startup sequence, because `/netdata.conf` lists options in first-read order.

use netdata_agent_log::{Priority, Source, nd_log};
use std::path::Path;

use netdata_agent_inicfg::{
    BOOLEAN_AUTO, Config, SECTION_CLOUD, SECTION_DB, SECTION_DIRECTORIES, SECTION_ENV_VARS,
    SECTION_GLOBAL, SECTION_HEALTH, SECTION_LOGS, SECTION_PLUGINS, SECTION_PULSE, SECTION_REGISTRY,
    SECTION_STATSD, SECTION_WEB,
};
use netdata_agent_text::c::filename_from_path_entry;

use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};
use netdata_agent_text::simple_pattern::{
    Separators as SimpleSeparators, SimplePattern, SimplePatternMode,
};

use netdata_agent_text::sanitize::rrdlabels_sanitize_value;

use netdata_agent_query::grouping::Windows;
use netdata_agent_rrd::mode::{DbMode, align_entries_to_pagesize};

use crate::acl::{AclPattern, WebAcl};
use crate::build;
use crate::system::{self, Resources};

/// `PLUGINSD_MAX_DIRECTORIES`.
const PLUGINSD_MAX_DIRECTORIES: usize = 20;

/// `RRD_STORAGE_TIERS`.
const STORAGE_TIERS: usize = 5;
/// `DEFAULT_CLOUD_BASE_URL`.
const DEFAULT_CLOUD_BASE_URL: &str = "https://app.netdata.cloud";

/// The `netdata_configured_*` directories.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub user_config: String,
    pub stock_config: String,
    pub stock_data: String,
    pub log: String,
    pub web: String,
    pub cache: String,
    pub varlib: String,
    pub cloud: String,
    /// `plugin_directories[]`; the first one is `netdata_configured_primary_plugins_dir`.
    pub plugins: Vec<String>,
}

impl Default for Dirs {
    fn default() -> Self {
        Dirs {
            user_config: build::CONFIG_DIR.to_string(),
            stock_config: build::LIBCONFIG_DIR.to_string(),
            stock_data: build::STOCK_DATA_DIR.to_string(),
            log: build::LOG_DIR.to_string(),
            web: build::WEB_DIR.to_string(),
            cache: build::CACHE_DIR.to_string(),
            varlib: build::VARLIB_DIR.to_string(),
            cloud: format!("{}/cloud.d", build::VARLIB_DIR),
            plugins: vec![build::PLUGINS_DIR.to_string()],
        }
    }
}

/// The daemon's configuration state (`netdata_config`, `cloud_config` and what the section readers derive).
#[derive(Debug, Default)]
pub struct Conf {
    pub netdata: Config,
    pub cloud: Config,
    pub dirs: Dirs,
    pub user: String,
    pub hostname: String,
    pub host_prefix: String,
    /// What `libuv_initialize()` sized, at the end of `netdata_conf_load()`.
    pub threads: Threads,
    // FUNCTION_RUN_ONCE guards.
    loaded: bool,
    compat_done: bool,
    directories_done: bool,
    logs_done: bool,
}

fn text(v: Option<Vec<u8>>) -> String {
    v.map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

impl Conf {
    /// `netdata_conf_load()`: the given file, or the user file then the stock one. Runs once: a second call (a
    /// second `-c`) returns false, and the C daemon exits.
    pub fn netdata_conf_load(
        &mut self,
        filename: Option<&str>,
        overwrite_used: bool,
        system: &Resources,
    ) -> bool {
        if self.loaded {
            return false;
        }
        self.loaded = true;
        let ret = match filename.filter(|f| !f.is_empty()) {
            Some(filename) => {
                let loaded = self.netdata.load(Path::new(filename), overwrite_used, None);
                if let Err(err) = &loaded {
                    nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_inicfg::load_errno(err);
                        "CONFIG: cannot load config file '{filename}'.");
                }
                loaded.is_ok()
            }
            None => {
                let user =
                    filename_from_path_entry(&self.dirs.user_config, build::CONFIG_FILENAME, None);
                let mut loaded = self.netdata.load(Path::new(&user), overwrite_used, None);
                if let Err(err) = &loaded {
                    nd_log!(Source::Daemon, Priority::Info, errno = netdata_agent_inicfg::load_errno(err);
                        "CONFIG: cannot load user config '{user}'. Will try the stock version.");
                    let stock = filename_from_path_entry(
                        &self.dirs.stock_config,
                        build::CONFIG_FILENAME,
                        None,
                    );
                    loaded = self.netdata.load(Path::new(&stock), overwrite_used, None);
                    if let Err(err) = &loaded {
                        nd_log!(Source::Daemon, Priority::Info, errno = netdata_agent_inicfg::load_errno(err);
                            "CONFIG: cannot load stock config '{stock}'. Running with internal defaults.");
                    }
                }
                loaded.is_ok()
            }
        };
        self.backwards_compatibility();
        self.section_directories();
        self.section_global_run_as_user();
        self.threads = libuv_initialize(&mut self.netdata, system, Path::new("/"));
        ret
    }

    /// `netdata_conf_backwards_compatibility()`: every rename, in C's order, once.
    pub fn backwards_compatibility(&mut self) {
        if self.compat_done {
            return;
        }
        self.compat_done = true;
        let c = &mut self.netdata;
        let moves: &[(&str, &str, &str, &str)] = &[
            (
                SECTION_GLOBAL,
                "http port listen backlog",
                SECTION_WEB,
                "listen backlog",
            ),
            (SECTION_GLOBAL, "bind socket to IP", SECTION_WEB, "bind to"),
            (SECTION_GLOBAL, "bind to", SECTION_WEB, "bind to"),
            (SECTION_GLOBAL, "port", SECTION_WEB, "default port"),
            (SECTION_GLOBAL, "default port", SECTION_WEB, "default port"),
            (
                SECTION_GLOBAL,
                "disconnect idle web clients after seconds",
                SECTION_WEB,
                "disconnect idle clients after seconds",
            ),
            (
                SECTION_GLOBAL,
                "respect web browser do not track policy",
                SECTION_WEB,
                "respect do not track policy",
            ),
            (
                SECTION_GLOBAL,
                "web x-frame-options header",
                SECTION_WEB,
                "x-frame-options response header",
            ),
            (
                SECTION_GLOBAL,
                "enable web responses gzip compression",
                SECTION_WEB,
                "enable gzip compression",
            ),
            (
                SECTION_GLOBAL,
                "web compression strategy",
                SECTION_WEB,
                "gzip compression strategy",
            ),
            (
                SECTION_GLOBAL,
                "web compression level",
                SECTION_WEB,
                "gzip compression level",
            ),
            (
                SECTION_GLOBAL,
                "config directory",
                SECTION_DIRECTORIES,
                "config",
            ),
            (
                SECTION_GLOBAL,
                "stock config directory",
                SECTION_DIRECTORIES,
                "stock config",
            ),
            (
                SECTION_GLOBAL,
                "stock data directory",
                SECTION_DIRECTORIES,
                "stock data",
            ),
            (SECTION_GLOBAL, "log directory", SECTION_DIRECTORIES, "log"),
            (
                SECTION_GLOBAL,
                "web files directory",
                SECTION_DIRECTORIES,
                "web",
            ),
            (
                SECTION_GLOBAL,
                "cache directory",
                SECTION_DIRECTORIES,
                "cache",
            ),
            (SECTION_GLOBAL, "lib directory", SECTION_DIRECTORIES, "lib"),
            (
                SECTION_GLOBAL,
                "home directory",
                SECTION_DIRECTORIES,
                "home",
            ),
            (
                SECTION_GLOBAL,
                "lock directory",
                SECTION_DIRECTORIES,
                "lock",
            ),
            (
                SECTION_GLOBAL,
                "plugins directory",
                SECTION_DIRECTORIES,
                "plugins",
            ),
            (
                SECTION_HEALTH,
                "health configuration directory",
                SECTION_DIRECTORIES,
                "health config",
            ),
            (
                SECTION_HEALTH,
                "stock health configuration directory",
                SECTION_DIRECTORIES,
                "stock health config",
            ),
            (
                SECTION_REGISTRY,
                "registry db directory",
                SECTION_DIRECTORIES,
                "registry",
            ),
            (SECTION_GLOBAL, "debug log", SECTION_LOGS, "debug"),
            (SECTION_GLOBAL, "error log", SECTION_LOGS, "error"),
            (SECTION_GLOBAL, "access log", SECTION_LOGS, "access"),
            (SECTION_GLOBAL, "facility log", SECTION_LOGS, "facility"),
            (
                SECTION_GLOBAL,
                "errors flood protection period",
                SECTION_LOGS,
                "errors flood protection period",
            ),
            (
                SECTION_GLOBAL,
                "errors to trigger flood protection",
                SECTION_LOGS,
                "errors to trigger flood protection",
            ),
            (SECTION_GLOBAL, "debug flags", SECTION_LOGS, "debug flags"),
            (
                SECTION_GLOBAL,
                "TZ environment variable",
                SECTION_ENV_VARS,
                "TZ",
            ),
            (
                SECTION_PLUGINS,
                "PATH environment variable",
                SECTION_ENV_VARS,
                "PATH",
            ),
            (
                SECTION_PLUGINS,
                "PYTHONPATH environment variable",
                SECTION_ENV_VARS,
                "PYTHONPATH",
            ),
            (SECTION_STATSD, "enabled", SECTION_PLUGINS, "statsd"),
            (SECTION_GLOBAL, "memory mode", SECTION_DB, "db"),
            (SECTION_DB, "mode", SECTION_DB, "db"),
            (SECTION_GLOBAL, "history", SECTION_DB, "retention"),
            (SECTION_GLOBAL, "update every", SECTION_DB, "update every"),
            (
                SECTION_GLOBAL,
                "page cache size",
                SECTION_DB,
                "dbengine page cache size",
            ),
            (
                SECTION_DB,
                "dbengine page cache size MB",
                SECTION_DB,
                "dbengine page cache size",
            ),
            (
                SECTION_DB,
                "dbengine extent cache size MB",
                SECTION_DB,
                "dbengine extent cache size",
            ),
            (
                SECTION_DB,
                "page cache size",
                SECTION_DB,
                "dbengine page cache size MB",
            ),
            (
                SECTION_GLOBAL,
                "memory deduplication (ksm)",
                SECTION_DB,
                "memory deduplication (ksm)",
            ),
            (
                SECTION_GLOBAL,
                "dbengine page fetch timeout",
                SECTION_DB,
                "dbengine page fetch timeout secs",
            ),
            (
                SECTION_GLOBAL,
                "dbengine page fetch retries",
                SECTION_DB,
                "dbengine page fetch retries",
            ),
            (
                SECTION_GLOBAL,
                "dbengine extent pages",
                SECTION_DB,
                "dbengine pages per extent",
            ),
            (
                SECTION_GLOBAL,
                "cleanup obsolete charts after seconds",
                SECTION_DB,
                "cleanup obsolete charts after",
            ),
            (
                SECTION_DB,
                "cleanup obsolete charts after secs",
                SECTION_DB,
                "cleanup obsolete charts after",
            ),
            (
                SECTION_GLOBAL,
                "gap when lost iterations above",
                SECTION_DB,
                "gap when lost iterations above",
            ),
            (
                SECTION_GLOBAL,
                "cleanup orphan hosts after seconds",
                SECTION_DB,
                "cleanup orphan hosts after",
            ),
            (
                SECTION_DB,
                "cleanup orphan hosts after secs",
                SECTION_DB,
                "cleanup orphan hosts after",
            ),
            (
                SECTION_DB,
                "cleanup ephemeral hosts after secs",
                SECTION_DB,
                "cleanup ephemeral hosts after",
            ),
            (
                SECTION_DB,
                "seconds to replicate",
                SECTION_DB,
                "replication period",
            ),
            (
                SECTION_DB,
                "seconds per replication step",
                SECTION_DB,
                "replication step",
            ),
            (
                SECTION_GLOBAL,
                "enable zero metrics",
                SECTION_DB,
                "enable zero metrics",
            ),
            (
                SECTION_CLOUD,
                "query thread count",
                SECTION_CLOUD,
                "query threads",
            ),
            (
                SECTION_PLUGINS,
                "netdata monitoring",
                SECTION_PLUGINS,
                "netdata pulse",
            ),
            (
                SECTION_PLUGINS,
                "netdata telemetry",
                SECTION_PLUGINS,
                "netdata pulse",
            ),
            (
                SECTION_PLUGINS,
                "netdata monitoring extended",
                SECTION_PULSE,
                "extended",
            ),
            ("telemetry", "extended telemetry", SECTION_PULSE, "extended"),
            (
                "global statistics",
                "update every",
                SECTION_PULSE,
                "update every",
            ),
            ("telemetry", "update every", SECTION_PULSE, "update every"),
            (
                SECTION_GLOBAL,
                "dbengine disk space",
                SECTION_DB,
                "dbengine tier 0 retention size",
            ),
            (
                SECTION_GLOBAL,
                "dbengine multihost disk space",
                SECTION_DB,
                "dbengine tier 0 retention size",
            ),
            (
                SECTION_DB,
                "dbengine disk space MB",
                SECTION_DB,
                "dbengine tier 0 retention size",
            ),
        ];
        for &(so, no, sn, nn) in moves {
            c.move_option(so, no, sn, nn);
        }
        for tier in 0..STORAGE_TIERS {
            c.move_option(
                SECTION_DB,
                &format!("dbengine tier {tier} retention days"),
                SECTION_DB,
                &format!("dbengine tier {tier} retention time"),
            );
            let old = if tier == 0 {
                "dbengine multihost disk space MB".to_string()
            } else {
                format!("dbengine tier {tier} multihost disk space MB")
            };
            let new = format!("dbengine tier {tier} retention size");
            c.move_option(SECTION_DB, &old, SECTION_DB, &new);
            c.move_option(
                SECTION_DB,
                &format!("dbengine tier {tier} disk space MB"),
                SECTION_DB,
                &new,
            );
        }
        let tail: &[(&str, &str, &str, &str)] = &[
            (SECTION_LOGS, "error", SECTION_LOGS, "daemon"),
            (SECTION_LOGS, "severity level", SECTION_LOGS, "level"),
            (
                SECTION_LOGS,
                "errors to trigger flood protection",
                SECTION_LOGS,
                "logs to trigger flood protection",
            ),
            (
                SECTION_LOGS,
                "errors flood protection period",
                SECTION_LOGS,
                "logs flood protection period",
            ),
            (
                SECTION_HEALTH,
                "is ephemeral",
                SECTION_GLOBAL,
                "is ephemeral node",
            ),
            (
                SECTION_HEALTH,
                "has unstable connection",
                SECTION_GLOBAL,
                "has unstable connection",
            ),
            (
                SECTION_HEALTH,
                "run at least every seconds",
                SECTION_HEALTH,
                "run at least every",
            ),
            (
                SECTION_HEALTH,
                "postpone alarms during hibernation for seconds",
                SECTION_HEALTH,
                "postpone alarms during hibernation for",
            ),
            (
                SECTION_HEALTH,
                "health log history",
                SECTION_HEALTH,
                "health log retention",
            ),
            (
                SECTION_REGISTRY,
                "registry expire idle persons days",
                SECTION_REGISTRY,
                "registry expire idle persons",
            ),
            (
                SECTION_WEB,
                "disconnect idle clients after seconds",
                SECTION_WEB,
                "disconnect idle clients after",
            ),
            (
                SECTION_WEB,
                "accept a streaming request every seconds",
                SECTION_WEB,
                "accept a streaming request every",
            ),
            (
                SECTION_STATSD,
                "set charts as obsolete after secs",
                SECTION_STATSD,
                "set charts as obsolete after",
            ),
            (
                SECTION_STATSD,
                "disconnect idle tcp clients after seconds",
                SECTION_STATSD,
                "disconnect idle tcp clients after",
            ),
            (
                "plugin:idlejitter",
                "loop time in ms",
                "plugin:idlejitter",
                "loop time",
            ),
            (
                "plugin:proc:/sys/class/infiniband",
                "refresh ports state every seconds",
                "plugin:proc:/sys/class/infiniband",
                "refresh ports state every",
            ),
        ];
        for &(so, no, sn, nn) in tail {
            c.move_option(so, no, sn, nn);
        }
    }

    /// `netdata_conf_section_directories()`: the runtime paths, then the plugin directories list.
    pub fn section_directories(&mut self) {
        if self.directories_done {
            return;
        }
        self.directories_done = true;
        let c = &mut self.netdata;
        let d = &mut self.dirs;
        let path = |c: &mut Config, name: &str, default: &str| {
            text(c.get_path(SECTION_DIRECTORIES, name, Some(default)))
        };
        d.user_config = path(c, "config", &d.user_config.clone());
        d.stock_config = path(c, "stock config", &d.stock_config.clone());
        d.stock_data = path(c, "stock data", &d.stock_data.clone());
        d.log = path(c, "log", &d.log.clone());
        d.web = path(c, "web", &d.web.clone());
        d.cache = path(c, "cache", &d.cache.clone());
        d.varlib = path(c, "lib", &d.varlib.clone());
        // get_varlib_subdir_from_config()
        d.cloud = path(c, "cloud.d", &format!("{}/cloud.d", d.varlib));
        // pluginsd_initialize_plugin_directories()
        let default = format!(
            "\"{}\" \"{}/custom-plugins.d\"",
            build::PLUGINS_DIR,
            build::CONFIG_DIR
        );
        let list = c
            .get_path_list(SECTION_DIRECTORIES, "plugins", Some(&default))
            .unwrap_or_default();
        d.plugins = quoted_strings_splitter(&list, PLUGINSD_MAX_DIRECTORIES, Separators::Config)
            .iter()
            .map(|w| String::from_utf8_lossy(w).into_owned())
            .collect();
    }

    /// `netdata_conf_section_logs()`: `[logs]` and the ACLK conversation log in C's order, each applied to the
    /// logger as it is read. Sources default to the journal when stderr is connected to it, except access and debug.
    pub fn section_logs(&mut self) {
        if self.logs_done {
            return;
        }
        self.logs_done = true;
        self.section_directories();
        let c = &mut self.netdata;
        netdata_agent_log::set_facility(&text(c.get(SECTION_LOGS, "facility", Some("daemon"))));
        let period = c.get_duration_seconds(
            SECTION_LOGS,
            "logs flood protection period",
            i64::from(netdata_agent_log::DEFAULT_THROTTLE_PERIOD),
        );
        let logs = c.get_number(
            SECTION_LOGS,
            "logs to trigger flood protection",
            i64::from(netdata_agent_log::DEFAULT_THROTTLE_LOGS),
        );
        // C reads it as `long long` and passes it as `unsigned long`
        netdata_agent_log::set_flood_protection(logs as u64, period);
        let level = std::env::var_os("NETDATA_LOG_LEVEL")
            .map_or("info", |v| Priority::parse(&v.to_string_lossy()).name());
        netdata_agent_log::set_priority_level(&text(c.get(SECTION_LOGS, "level", Some(level))));

        let journal = netdata_agent_log::is_stderr_connected_to_journal();
        let log_dir = self.dirs.log.clone();
        let file = |name: &str| format!("{log_dir}/{name}");
        let or_journal = |name: &str| {
            if journal {
                "journal".to_string()
            } else {
                file(name)
            }
        };
        let sources = [
            (Source::Debug, "debug", file("debug.log")),
            (Source::Daemon, "daemon", or_journal("daemon.log")),
            (Source::Collector, "collector", or_journal("collector.log")),
            (Source::Access, "access", file("access.log")),
            (Source::Health, "health", or_journal("health.log")),
        ];
        for (source, name, default) in sources {
            let setting = text(c.get(SECTION_LOGS, name, Some(&default)));
            netdata_agent_log::set_user_settings(source, &setting);
        }
        if c.get_boolean(SECTION_CLOUD, "conversation log", false) {
            let setting = text(c.get(
                SECTION_CLOUD,
                "conversation log file",
                Some(&file("aclk.log")),
            ));
            netdata_agent_log::set_user_settings(Source::Aclk, &setting);
        }

        // debug_flags_initialize()
        let flags = text(c.get(SECTION_LOGS, "debug flags", Some("0x0000000000000000")));
        export("NETDATA_DEBUG_FLAGS", &flags);
        // the flags gate debug records, which release builds compile out
        let debug_flags = netdata_agent_text::parse::strtoul0(flags.as_bytes()).0;
        if debug_flags != 0 {
            use nix::sys::resource::{RLIM_INFINITY, Resource, setrlimit};
            if let Err(errno) = setrlimit(Resource::RLIMIT_CORE, RLIM_INFINITY, RLIM_INFINITY) {
                nd_log!(Source::Daemon, Priority::Err, errno = errno as i32;
                    "Cannot request unlimited core dumps for debugging... Proceeding anyway...");
            }
            let _ = nix::sys::prctl::set_dumpable(true);
        }

        // aclk_config_get_query_scope(): the ACLK port reads the scope from here
        c.get(SECTION_CLOUD, "scope", Some("full"));
    }

    /// `netdata_conf_section_global_run_as_user()`.
    pub fn section_global_run_as_user(&mut self) {
        let uid = nix::unistd::getuid();
        let default = if uid.is_root() {
            build::NETDATA_USER.to_string()
        } else {
            nix::unistd::User::from_uid(uid)
                .ok()
                .flatten()
                .map(|u| u.name)
                .unwrap_or_default()
        };
        self.user = text(
            self.netdata
                .get(SECTION_GLOBAL, "run as user", Some(&default)),
        );
    }

    /// `cloud_conf_load()`: `cloud.d/cloud.conf` over the defaults.
    pub fn cloud_conf_load(&mut self, silent: bool) {
        self.section_directories();
        let filename = self.cloud_conf_filename();
        load_cloud_conf(&mut self.cloud, &filename, silent);
    }

    pub fn cloud_conf_filename(&self) -> String {
        filename_from_path_entry(&self.dirs.cloud, "cloud.conf", None)
    }
}

/// `cloud_conf_load()` over the cloud configuration in hand.
pub fn load_cloud_conf(c: &mut Config, filename: &str, silent: bool) {
    if let Err(err) = c.load(Path::new(filename), true, None) {
        if !silent {
            nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_inicfg::load_errno(&err);
                "CLAIM: cannot load cloud config '{filename}'. Running with internal defaults.");
        }
    }
    c.move_option(SECTION_GLOBAL, "cloud base url", SECTION_GLOBAL, "url");
    c.get(SECTION_GLOBAL, "url", Some(DEFAULT_CLOUD_BASE_URL));
    c.get(SECTION_GLOBAL, "proxy", Some("env"));
    c.get(SECTION_GLOBAL, "token", Some(""));
    c.get(SECTION_GLOBAL, "rooms", Some(""));
    c.get_boolean(SECTION_GLOBAL, "insecure", false);
    c.get(SECTION_GLOBAL, "machine_guid", Some(""));
    c.get(SECTION_GLOBAL, "claimed_id", Some(""));
    c.get(SECTION_GLOBAL, "hostname", Some(""));
}

impl Conf {
    /// `nd_runtime_paths_load_hostname_from_inicfg()`: `[global] host access prefix`, then `hostname`.
    pub fn section_global_hostname(&mut self) {
        let prefix = text(
            self.netdata
                .get(SECTION_GLOBAL, "host access prefix", Some("")),
        );
        self.host_prefix = verify_netdata_host_prefix(prefix);
        netdata_agent_log::set_host_prefix(&self.host_prefix);
        let system = os_hostname(&self.host_prefix);
        if system.is_empty() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Cannot get machine hostname."
            );
        }
        self.hostname = text(self.netdata.get(SECTION_GLOBAL, "hostname", Some(&system)));
    }
}

/// `netdata_conf_section_db()` (`src/daemon/config/netdata-conf-db.c`) up to the dbengine options. KSM and the
/// orphan, ephemeral and obsolete cleanups are not ported: their options are read so that they print as in C.
pub fn section_db(c: &mut Config, page_size: i64) -> DbSection {
    // nd_profile.update_every: 1 for every profile, iot included (its "MUST BE 2" note notwithstanding).
    let mut update_every = c.get_duration_seconds(SECTION_DB, "update every", 1) as i32;
    if update_every < UPDATE_EVERY_MIN {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "Data collection frequency in netdata.conf ([db].update every), changed from \
                 {update_every} to {UPDATE_EVERY_MIN}"
        );
        update_every = UPDATE_EVERY_MIN;
        c.set_duration_seconds(SECTION_DB, "update every", i64::from(update_every));
    }
    if update_every > UPDATE_EVERY_MAX {
        // C names the minimum in this message too.
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "Data collection frequency in netdata.conf ([db].update every), changed from \
                 {update_every} to {UPDATE_EVERY_MIN}"
        );
        update_every = UPDATE_EVERY_MAX;
        c.set_duration_seconds(SECTION_DB, "update every", i64::from(update_every));
    }

    let name = text(c.get(SECTION_DB, "db", Some(DbMode::Dbengine.name())));
    let mode = DbMode::from_name(&name);
    if name != mode.name() {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "Invalid memory mode '{name}' given. Using '{}'",
            mode.name()
        );
        c.set(SECTION_DB, "db", mode.name());
    }

    let mut history_entries = DEFAULT_HISTORY_ENTRIES;
    if mode != DbMode::Dbengine && mode != DbMode::None {
        history_entries = i64::from(c.get_duration_seconds(
            SECTION_DB,
            "retention",
            align_entries_to_pagesize(mode, DEFAULT_HISTORY_ENTRIES, page_size),
        ) as i32);
        let aligned = align_entries_to_pagesize(mode, history_entries, page_size);
        if aligned != history_entries {
            c.set_duration_seconds(SECTION_DB, "retention", aligned);
            history_entries = aligned;
        }
    }

    c.get_boolean_ondemand(SECTION_DB, "memory deduplication (ksm)", BOOLEAN_AUTO);

    let mut orphan = c.get_duration_seconds(SECTION_DB, "cleanup orphan hosts after", 3600);
    if orphan < 10 {
        orphan = 10;
        c.set_duration_seconds(SECTION_DB, "cleanup orphan hosts after", orphan);
    }
    let ephemeral = c.get_duration_seconds(SECTION_DB, "cleanup ephemeral hosts after", 0);
    if ephemeral != 0 && ephemeral < orphan {
        c.set_duration_seconds(SECTION_DB, "cleanup ephemeral hosts after", orphan);
    }
    if c.get_duration_seconds(SECTION_DB, "cleanup obsolete charts after", 3600) < 10 {
        nd_log!(
            Source::Daemon,
            Priority::Info,
            "The \"cleanup obsolete charts after\" option was set to 10 seconds."
        );
        c.set_duration_seconds(SECTION_DB, "cleanup obsolete charts after", 10);
    }

    let mut gap = c.get_number(SECTION_DB, "gap when lost iterations above", 1) as i32;
    if gap < 1 {
        gap = 1;
        c.set_number(SECTION_DB, "gap when lost iterations above", 1);
    }

    DbSection {
        update_every,
        mode,
        history_entries,
        gap_when_lost_iterations_above: i64::from(gap) + 2,
    }
}

impl Conf {
    /// `set_environment_for_plugins_and_scripts()` (`src/daemon/environment.c`): what plugins and scripts inherit.
    /// Every required directory is entered (the last `chdir` wins until the caller moves on), and the writable ones
    /// are created when missing. The error is C's `fatal()` errno and text.
    pub fn environment_for_plugins(&mut self, update_every: i32) -> Result<(), (i32, String)> {
        export("NETDATA_UPDATE_EVERY", &update_every.to_string());
        export("NETDATA_VERSION", build::NETDATA_VERSION);
        export("NETDATA_HOSTNAME", &self.hostname);
        export("NETDATA_HOST_PREFIX", &self.host_prefix);
        let primary_plugins = self.primary_plugins_dir();
        let d = &self.dirs;
        for (env, dir, create) in [
            ("NETDATA_CONFIG_DIR", &d.user_config, None),
            ("NETDATA_USER_CONFIG_DIR", &d.user_config, None),
            ("NETDATA_STOCK_CONFIG_DIR", &d.stock_config, None),
            ("NETDATA_STOCK_DATA_DIR", &d.stock_data, None),
            ("NETDATA_PLUGINS_DIR", &primary_plugins, None),
            ("NETDATA_WEB_DIR", &d.web, None),
            ("NETDATA_CACHE_DIR", &d.cache, Some(0o775)),
            ("NETDATA_LIB_DIR", &d.varlib, Some(0o775)),
            ("NETDATA_LOG_DIR", &d.log, Some(0o775)),
            ("CLAIMING_DIR", &d.cloud, Some(0o770)),
        ] {
            verify_required_directory(env, dir, create)?;
            export(env, dir);
        }
        let user_dirs = d
            .plugins
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        export("NETDATA_USER_PLUGINS_DIRS", &user_dirs);
        // A NULL default: the key is never created, only marked used when the user wrote it.
        let port = self.netdata.get(SECTION_WEB, "default port", None);
        let port = port.map_or_else(|| "19999".to_string(), |p| text(Some(p)));
        export("NETDATA_LISTEN_PORT", &port);
        let env = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
        // snprintfz() into 4096 bytes
        let mut path = format!(
            "{}:/sbin:/usr/sbin:/usr/local/bin:/usr/local/sbin",
            env("PATH").unwrap_or_else(|| "/bin:/usr/bin".to_string())
        );
        while path.len() > 4095 {
            path.pop();
        }
        let path = text(
            self.netdata
                .get_path_list(SECTION_ENV_VARS, "PATH", Some(&path)),
        );
        export("PATH", &path);
        let python = env("PYTHONPATH").unwrap_or_default();
        let python = text(self.netdata.get_path_list(
            SECTION_ENV_VARS,
            "PYTHONPATH",
            Some(&python),
        ));
        export("PYTHONPATH", &python);
        export("PYTHONUNBUFFERED", "1");
        export("LC_ALL", "C");
        Ok(())
    }

    /// The "home" startup step: `[directories] home`, the running user's home unless the key is set, exported as
    /// `HOME` (root's would be inherited otherwise).
    pub fn section_home(&mut self) {
        let pw_dir = nix::unistd::User::from_uid(nix::unistd::getuid())
            .ok()
            .flatten()
            .map(|u| u.dir.to_string_lossy().into_owned());
        let default = match pw_dir {
            Some(dir) if !self.netdata.exists(SECTION_DIRECTORIES, "home") => dir,
            _ => build::VARLIB_DIR.to_string(),
        };
        let home = text(
            self.netdata
                .get_path(SECTION_DIRECTORIES, "home", Some(&default)),
        );
        export("HOME", &home);
    }

    /// `netdata_configured_primary_plugins_dir`: no plugin directory at all is a NULL, which glibc prints as
    /// `(null)`.
    pub fn primary_plugins_dir(&self) -> String {
        self.dirs
            .plugins
            .first()
            .cloned()
            .unwrap_or_else(|| "(null)".to_string())
    }

    /// `health_set_silencers_filename()`, the "silencers" step, which creates `[health]`. Health is not ported, so
    /// the file is not read yet.
    pub fn health_silencers_filename(&mut self) {
        let default = format!("{}/health.silencers.json", self.dirs.varlib);
        self.netdata
            .get_filename(SECTION_HEALTH, "silencers file", Some(&default));
    }

    /// `health_load_config_defaults()`, in `rrd_init()` before localhost is created: every `[health]` default with
    /// C's corrections and records. Health is not ported; only `enabled` has a reader yet.
    pub fn health_load_config_defaults(&mut self) -> bool {
        let alarm_notify = format!("{}/alarm-notify.sh", self.primary_plugins_dir());
        let c = &mut self.netdata;
        let h = SECTION_HEALTH;
        let enabled = c.get_boolean(h, "enabled", true);
        c.get_boolean(h, "enable stock health configuration", true);
        c.get_boolean(h, "use summary for notifications", true);
        c.get_duration_seconds(h, "default repeat warning", 0);
        c.get_duration_seconds(h, "default repeat critical", 0);
        // unsigned int and uint32_t in C
        let entries = c.get_number(
            h,
            "in memory max health log entries",
            i64::from(HEALTH_LOG_ENTRIES_DEFAULT),
        ) as u32;
        let mut retention = c.get_duration_seconds(
            h,
            "health log retention",
            netdata_agent_streaming::conf::HEALTH_LOG_RETENTION_DEFAULT,
        ) as u32;
        c.get_filename(h, "script to execute on alarm", Some(&alarm_notify));
        c.get(h, "enabled alarms", Some("*"));
        c.get_duration_seconds(h, "run at least every", 10);
        c.get_duration_seconds(h, "postpone alarms during hibernation for", 60);
        c.get_duration_seconds(h, "notification execution timeout", 120);
        let bound = if entries < HEALTH_LOG_ENTRIES_MIN {
            Some(("minimum", HEALTH_LOG_ENTRIES_MIN))
        } else if entries > HEALTH_LOG_ENTRIES_MAX {
            Some(("maximum", HEALTH_LOG_ENTRIES_MAX))
        } else {
            None
        };
        if let Some((which, bound)) = bound {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "Health configuration has invalid max log entries {entries}, using {which} of {bound}"
            );
            c.set_number(h, "in memory max health log entries", i64::from(bound));
        }
        if retention < HEALTH_LOG_MINIMUM_HISTORY {
            nd_log!(
                Source::Daemon,
                Priority::Warning,
                "Health configuration has invalid health log retention {retention}. Using minimum {HEALTH_LOG_MINIMUM_HISTORY}"
            );
            retention = HEALTH_LOG_MINIMUM_HISTORY;
            c.set_duration_seconds(h, "health log retention", i64::from(retention));
        }
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Health log history is set to {retention} seconds ({} days)",
            retention / 86400
        );
        enabled
    }
}

/// `health_internals.h` and `health.h` (the retention default is streaming's).
const HEALTH_LOG_ENTRIES_DEFAULT: u32 = 1000;
const HEALTH_LOG_ENTRIES_MIN: u32 = 10;
const HEALTH_LOG_ENTRIES_MAX: u32 = 100_000;
const HEALTH_LOG_MINIMUM_HISTORY: u32 = 86400;

/// `verify_required_directory()`: enter it, or create it when allowed; otherwise C's `fatal()` errno and text of the
/// part that is wrong. C clears errno only at entry and before each component's `stat()`, so the failed `chdir()` or
/// `mkdir()` of a one-component path is still the errno of the later records.
fn verify_required_directory(
    env: &str,
    dir: &str,
    create: Option<u32>,
) -> Result<(), (i32, String)> {
    use std::os::unix::fs::DirBuilderExt;
    if !dir.starts_with('/') {
        return Err((
            0,
            format!("Invalid directory path (must be an absolute path): '{dir}' ({env})"),
        ));
    }
    let mut errno = match std::env::set_current_dir(dir) {
        Ok(()) => return Ok(()),
        Err(err) => netdata_agent_log::errno_of(&err),
    };
    if let Some(mode) = create {
        match std::fs::DirBuilder::new().mode(mode).create(dir) {
            Ok(()) => return Ok(()),
            Err(err) => errno = netdata_agent_log::errno_of(&err),
        }
    }
    let required = format!("Required directory: '{dir}' ({env})");
    for (at, _) in dir.match_indices('/').skip(1) {
        let component = &dir[..at];
        errno = 0;
        match std::fs::metadata(component) {
            Err(err) => {
                return Err((
                    netdata_agent_log::errno_of(&err),
                    format!("{required} - Missing or inaccessible component: '{component}'"),
                ));
            }
            Ok(m) if !m.is_dir() => {
                return Err((
                    errno,
                    format!("{required} - Component '{component}' exists but is not a directory."),
                ));
            }
            Ok(_) => {}
        }
    }
    match std::fs::metadata(dir) {
        Err(err) => {
            return Err((
                netdata_agent_log::errno_of(&err),
                format!("{required} - Missing or inaccessible: '{dir}'"),
            ));
        }
        Ok(m) if !m.is_dir() => {
            return Err((
                errno,
                format!("{required} - '{dir}' exists but is not a directory."),
            ));
        }
        Ok(_) => {}
    }
    use nix::unistd::{AccessFlags, access};
    if let Err(err) = access(dir, AccessFlags::R_OK | AccessFlags::X_OK) {
        return Err((
            err as i32,
            format!("{required} - Insufficient permissions for: '{dir}'"),
        ));
    }
    Err((errno, format!("{required} - Failed")))
}

/// `web_server_threading_selection()`: `[web] mode`; anything but `none` is the static-threaded server.
pub fn web_server_enabled(c: &mut Config) -> bool {
    text(c.get(SECTION_WEB, "mode", Some("static-threaded"))) != "none"
}

/// `web server max sockets`, as the web server thread reads it: a quarter of the open-files limit by default, split
/// evenly between the workers (each C worker reports its share when accept() runs out of descriptors).
pub fn web_server_max_sockets_per_worker(c: &mut Config, workers: usize) -> usize {
    use nix::sys::resource::{Resource, getrlimit};
    let soft = getrlimit(Resource::RLIMIT_NOFILE).map_or(0, |(soft, _)| soft);
    let max = c.get_number(SECTION_WEB, "web server max sockets", (soft / 4) as i64);
    // a size_t in C: a negative value is a huge share
    (max as u64 as usize) / workers.max(1)
}

/// `MIN_LIBUV_WORKER_THREADS` and `MAX_LIBUV_WORKER_THREADS`.
#[cfg(target_pointer_width = "64")]
const LIBUV_WORKER_THREADS: (i64, i64) = (16, 1024);
#[cfg(not(target_pointer_width = "64"))]
const LIBUV_WORKER_THREADS: (i64, i64) = (8, 128);

/// What `libuv_initialize()` sizes the daemon's threads by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Threads {
    /// The stack of every thread the daemon starts: libuv's size, which C's `nd_thread_create()` gets through
    /// `uv_thread_create()`. `[global] pthread stack size` never reaches a C thread.
    pub thread_stack_size: usize,
    /// `netdata_conf_cpus()`, a `size_t` as in C.
    pub cpus: u64,
    /// `libuv_worker_threads`.
    pub libuv_worker_threads: i64,
    /// `default_stacksize`: `[global] pthread stack size`.
    pub pthread_stack_size: u64,
}

/// `netdata_threads_set_stack_size()`: C sets an attribute no thread create uses (the size never reaches a thread);
/// what remains is its record.
pub fn threads_set_stack_size(stack_size: u64) {
    if stack_size > netdata_agent_sys::PTHREAD_STACK_MIN as u64 {
        nd_log!(
            Source::Daemon,
            Priority::Debug,
            "Set threads stack size to {stack_size} bytes"
        );
    } else {
        nd_log!(
            Source::Daemon,
            Priority::Warning,
            "Invalid pthread stacksize {stack_size}"
        );
    }
}

/// `libuv_initialize()` (`src/daemon/config/netdata-conf-global.c`): the thread stack size, `netdata_conf_cpus()` and
/// the libuv worker count, each exported where C exports it. `root` is `/` outside tests (the cgroup cpusets).
pub fn libuv_initialize(c: &mut Config, system: &Resources, root: &Path) -> Threads {
    // netdata_conf_stack_size(): the libc default, at least 1 MiB (musl gives 128 KiB).
    let libc_stack = netdata_agent_sys::default_thread_stack_size().unwrap_or(8 << 20);
    let stack_size = c.get_size_bytes(
        SECTION_GLOBAL,
        "pthread stack size",
        (libc_stack as u64).max(1 << 20),
    );
    // the value still divides the libuv worker cap below
    threads_set_stack_size(stack_size);
    let thread_stack_size = uv_thread_stack_size(system.page_size.max(1) as u64);

    // netdata_conf_cpus(): the cgroup cpuset, else every CPU.
    let cpuset = |rel: &str| {
        std::fs::read(root.join(rel))
            .map(|text| system::cpuset_cpus(&text))
            .unwrap_or(0)
    };
    let mut cpus = cpuset("sys/fs/cgroup/cpuset.cpus");
    if cpus == 0 {
        cpus = cpuset("sys/fs/cgroup/cpuset/cpuset.cpus");
    }
    if cpus == 0 {
        cpus = system.system_cpus;
    }
    // C keeps the count in a size_t: a negative value wraps and only 0 becomes 1
    let cpus = (c.get_number(SECTION_GLOBAL, "cpu cores", cpus) as u64).max(1);
    export("NETDATA_CONF_CPUS", &cpus.to_string());

    // Six per CPU, as many as a twentieth of the RAM (or a tenth of what is available) can hold stacks for; C
    // computes both in int.
    let (min, max) = LIBUV_WORKER_THREADS;
    let mut threads = (cpus as i32).wrapping_mul(6);
    let mem = system.memory;
    if mem.total > 0 {
        let for_threads = (mem.total / 20).min(mem.available / 10);
        let allowed = (for_threads.div_ceil(stack_size.max(1)) as i32).max(min as i32);
        threads = threads.min(allowed);
    }
    let threads = c.get_number_range(
        SECTION_GLOBAL,
        "libuv worker threads",
        i64::from(threads).clamp(min, max),
        min,
        max,
    );
    export("UV_THREADPOOL_SIZE", &threads.to_string());
    Threads {
        thread_stack_size,
        cpus,
        libuv_worker_threads: threads,
        pthread_stack_size: stack_size,
    }
}

/// `uv__thread_stack_size()` on Linux: `RLIMIT_STACK` rounded down to the page size when it is finite and at least
/// `PTHREAD_STACK_MIN` (8 KiB at the least), else 2 MiB. Measured on libuv 1.50, the library the C build links:
/// 8 MiB and 20 KiB limits give those sizes, 12 KiB and unlimited give 2 MiB.
fn uv_thread_stack_size(page_size: u64) -> usize {
    use nix::sys::resource::{RLIM_INFINITY, Resource, getrlimit};
    const DEFAULT: u64 = 2 << 20;
    let min = (netdata_agent_sys::PTHREAD_STACK_MIN as u64).max(8192);
    let size = match getrlimit(Resource::RLIMIT_STACK) {
        Ok((cur, _)) if cur != RLIM_INFINITY => {
            let cur = cur - cur % page_size;
            if cur >= min { cur } else { DEFAULT }
        }
        _ => DEFAULT,
    };
    size as usize
}

/// `setenv()` for the plugins; the daemon is still single-threaded when it runs.
pub(crate) fn export(key: &str, value: &str) {
    if let Err(err) = netdata_agent_sys::setenv(key, value) {
        nd_log!(Source::Daemon, Priority::Err, "cannot export {key}: {err}");
    }
}

/// `UPDATE_EVERY_MIN` and `UPDATE_EVERY_MAX`.
const UPDATE_EVERY_MIN: i32 = 1;
const UPDATE_EVERY_MAX: i32 = 3600;
/// `RRD_DEFAULT_HISTORY_ENTRIES`.
const DEFAULT_HISTORY_ENTRIES: i64 = 3600;

/// What `[db]` sets for the rest of the daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbSection {
    /// `nd_profile.update_every`.
    pub update_every: i32,
    /// `default_rrd_memory_mode`.
    pub mode: DbMode,
    /// `default_rrd_history_entries`: `[db] retention` for ram and alloc, else the compiled default.
    pub history_entries: i64,
    /// `gap_when_lost_iterations_above`: the option plus the 2 C adds after reading it.
    pub gap_when_lost_iterations_above: i64,
}

/// `verify_netdata_host_prefix(true)` (`src/libnetdata/paths/paths.c`): a directory, without `%`, holding procfs and
/// sysfs mounts; otherwise it is ignored (empty).
fn verify_netdata_host_prefix(prefix: String) -> String {
    use nix::sys::statfs::{PROC_SUPER_MAGIC, SYSFS_MAGIC, statfs};
    if prefix.is_empty() {
        return prefix;
    }
    let check = || -> Result<(), (String, &str)> {
        if prefix.contains('%') {
            return Err((prefix.clone(), "contains '%'"));
        }
        match std::fs::metadata(&prefix) {
            Err(_) => return Err((prefix.clone(), "failed to stat()")),
            Ok(m) if !m.is_dir() => return Err((prefix.clone(), "is not a directory")),
            Ok(_) => {}
        }
        for (dir, magic, not) in [
            ("proc", PROC_SUPER_MAGIC, "type is not procfs"),
            ("sys", SYSFS_MAGIC, "type is not sysfs"),
        ] {
            let path = format!("{prefix}/{dir}");
            match statfs(path.as_str()) {
                Err(_) => return Err((path, "failed to statfs()")),
                Ok(st) if st.filesystem_type() != magic => return Err((path, not)),
                Ok(_) => {}
            }
        }
        Ok(())
    };
    match check() {
        Ok(()) => {
            nd_log!(
                Source::Daemon,
                Priority::Info,
                "Using host prefix directory '{prefix}'"
            );
            prefix
        }
        Err((path, reason)) => {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Ignoring host prefix '{prefix}': path '{path}' {reason}"
            );
            String::new()
        }
    }
}

/// `HOST_NAME_MAX * 4 + 1`: the hostname buffer.
const HOSTNAME_BUFFER: usize = 64 * 4 + 1;

/// `os_hostname()`: `<prefix>/etc/hostname`, else `gethostname()`, else `host<hostid>`; trimmed and sanitized as a
/// label value. The reference build has no iconv, so no conversion to UTF-8 happens.
fn os_hostname(prefix: &str) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    if !prefix.is_empty()
        && let Ok(f) = std::fs::File::open(format!("{prefix}/etc/hostname"))
    {
        let _ = f.take(HOSTNAME_BUFFER as u64 - 1).read_to_end(&mut buf);
        if let Some(nul) = buf.iter().position(|&b| b == 0) {
            buf.truncate(nul);
        }
    }
    if buf.is_empty() {
        buf = match nix::unistd::gethostname() {
            Ok(h) => h.as_encoded_bytes().to_vec(),
            Err(_) => format!("host{}", netdata_agent_sys::gethostid()).into_bytes(),
        };
    }
    let trimmed = buf.trim_ascii();
    text(Some(rrdlabels_sanitize_value(trimmed, HOSTNAME_BUFFER)))
}

/// What `netdata_conf_section_web()` configures.
pub struct WebConf {
    pub disconnect_idle_after_s: i64,
    pub first_request_timeout_s: i64,
    pub streaming_rate_s: i64,
    pub respect_do_not_track: bool,
    pub x_frame_options: Option<String>,
    pub acl: WebAcl,
    pub gzip: bool,
    pub gzip_level: u32,
}

/// The zlib strategies `[web] gzip compression strategy` accepts.
const GZIP_STRATEGIES: [&str; 5] = ["default", "filtered", "huffman only", "rle", "fixed"];

/// `tg_ses_init()` and `tg_des_init()` (at web API init): `[web] ses max tg_des_window` and `des max tg_des_window`;
/// a value up to 1 is written back as the default and ignored.
pub fn grouping_windows(c: &mut Config) -> Windows {
    let mut windows = Windows::default();
    for (key, max) in [
        ("ses max tg_des_window", &mut windows.ses),
        ("des max tg_des_window", &mut windows.des),
    ] {
        let value = c.get_number(SECTION_WEB, key, *max);
        if value <= 1 {
            c.set_number(SECTION_WEB, key, *max);
        } else {
            *max = value;
        }
    }
    windows
}

/// The `TZ` part of `get_system_timezone()` (`src/daemon/analytics.c`): without a `TZ` in the environment,
/// `[environment variables] TZ` (default `:/etc/localtime`, which spares libc a `stat()` per conversion). Local-time
/// renderings (csv dates, datatable dates) read it.
pub fn set_timezone_env(netdata: &mut Config) -> std::io::Result<()> {
    if std::env::var_os("TZ").is_some_and(|tz| !tz.is_empty()) {
        return Ok(());
    }
    let tz = netdata
        .get(SECTION_ENV_VARS, "TZ", Some(":/etc/localtime"))
        .unwrap_or_default();
    netdata_agent_sys::setenv("TZ", &String::from_utf8_lossy(&tz))
}

/// `netdata_conf_web_query_threads()`: two per CPU on a parent (at most 256 CPUs), at least 6, unless configured.
pub fn web_query_threads(c: &mut Config, cpus: usize, is_parent: bool) -> usize {
    let cpus = cpus.min(256);
    let threads = (cpus * if is_parent { 2 } else { 1 }).max(6);
    let threads = c.get_number(SECTION_WEB, "web server threads", threads as i64);
    if threads < 1 {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "[web].web server threads in netdata.conf needs to be at least 1. Overwriting it."
        );
        c.set_number(SECTION_WEB, "web server threads", 1);
        return 1;
    }
    threads as usize
}

impl Conf {
    /// `netdata_conf_section_web()`.
    pub fn section_web(&mut self) -> WebConf {
        let c = &mut self.netdata;
        let disconnect_idle_after_s =
            c.get_duration_seconds(SECTION_WEB, "disconnect idle clients after", 60);
        let first_request_timeout_s =
            c.get_duration_seconds(SECTION_WEB, "timeout for first request", 60);
        let streaming_rate_s =
            c.get_duration_seconds(SECTION_WEB, "accept a streaming request every", 0);
        let respect_do_not_track = c.get_boolean(SECTION_WEB, "respect do not track policy", false);
        let x_frame_options = Some(text(c.get(
            SECTION_WEB,
            "x-frame-options response header",
            Some(""),
        )))
        .filter(|x| !x.is_empty());
        let acl_pattern = |c: &mut Config,
                           section: &str,
                           name: &str,
                           default: &str,
                           dns_name: &str,
                           dns_default: &str| {
            let pattern = SimplePattern::new(
                &c.get(section, name, Some(default)).unwrap_or_default(),
                SimpleSeparators::Whitespace,
                SimplePatternMode::Exact,
                true,
            );
            let dns = make_dns_decision(c, section, dns_name, dns_default, &pattern);
            AclPattern { pattern, dns }
        };
        let connections = acl_pattern(
            c,
            SECTION_WEB,
            "allow connections from",
            "localhost *",
            "allow connections by dns",
            "heuristic",
        );
        let dashboard_default =
            text(c.get(SECTION_WEB, "allow dashboard from", Some("localhost *")));
        let dashboard = acl_pattern(
            c,
            SECTION_WEB,
            "allow dashboard from",
            &dashboard_default,
            "allow dashboard by dns",
            "heuristic",
        );
        let mcp = acl_pattern(
            c,
            SECTION_WEB,
            "allow mcp from",
            &dashboard_default,
            "allow mcp by dns",
            "heuristic",
        );
        let badges = acl_pattern(
            c,
            SECTION_WEB,
            "allow badges from",
            "*",
            "allow badges by dns",
            "heuristic",
        );
        let registry = acl_pattern(
            c,
            SECTION_REGISTRY,
            "allow from",
            "*",
            "allow by dns",
            "heuristic",
        );
        let streaming = acl_pattern(
            c,
            SECTION_WEB,
            "allow streaming from",
            "*",
            "allow streaming by dns",
            "heuristic",
        );
        // Not heuristic: the wildcards could match names, but the intent is IP addresses.
        let netdataconf = acl_pattern(
            c,
            SECTION_WEB,
            "allow netdata.conf from",
            "localhost fd* 10.* 192.168.* 172.16.* 172.17.* 172.18.* 172.19.* 172.20.* 172.21.* 172.22.* 172.23.* \
             172.24.* 172.25.* 172.26.* 172.27.* 172.28.* 172.29.* 172.30.* 172.31.* UNKNOWN",
            "allow netdata.conf by dns",
            "no",
        );
        let management = acl_pattern(
            c,
            SECTION_WEB,
            "allow management from",
            "localhost",
            "allow management by dns",
            "heuristic",
        );
        let gzip = c.get_boolean(SECTION_WEB, "enable gzip compression", true);
        let strategy = text(c.get(SECTION_WEB, "gzip compression strategy", Some("default")));
        // flate2 has no strategy setting: the strategy changes only the compressed bytes, not what clients decode.
        match GZIP_STRATEGIES.iter().find(|name| **name == strategy) {
            Some(_) => {}
            None => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Invalid compression strategy '{strategy}'. Valid strategies are 'default', 'filtered', 'huffman only', 'rle' and 'fixed'. Proceeding with 'default'."
                );
            }
        }
        let level = c.get_number(SECTION_WEB, "gzip compression level", 3) as i32;
        let gzip_level = if level < 1 {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Invalid compression level {level}. Valid levels are 1 (fastest) to 9 (best ratio). Proceeding with level 1 (fastest compression)."
            );
            1
        } else if level > 9 {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "Invalid compression level {level}. Valid levels are 1 (fastest) to 9 (best ratio). Proceeding with level 9 (best compression)."
            );
            9
        } else {
            level as u32
        };
        WebConf {
            disconnect_idle_after_s,
            first_request_timeout_s,
            streaming_rate_s,
            respect_do_not_track,
            x_frame_options,
            acl: WebAcl {
                connections,
                dashboard,
                mcp,
                badges,
                registry,
                streaming,
                netdataconf,
                management,
            },
            gzip,
            gzip_level,
        }
    }
}

/// `make_dns_decision()`: `yes`, `no`, else whether the pattern may match names (with an error for other values).
fn make_dns_decision(
    c: &mut Config,
    section: &str,
    name: &str,
    default: &str,
    pattern: &SimplePattern,
) -> bool {
    let value = text(c.get(section, name, Some(default)));
    match value.as_str() {
        "yes" => true,
        "no" => false,
        other => {
            if other != "heuristic" {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "Invalid configuration option '{other}' for '{section}'/'{name}'. Valid options are 'yes', 'no' and 'heuristic'. Proceeding with 'heuristic'"
                );
            }
            pattern.is_potential_name()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_stacks_follow_libuv_not_the_stack_size_key() {
        use nix::sys::resource::{RLIM_INFINITY, Resource, getrlimit};
        let mut c = Config::default();
        c.set(SECTION_GLOBAL, "pthread stack size", "64KiB");
        let system = Resources {
            system_cpus: 4,
            memory: crate::system::SystemMemory {
                total: 0,
                available: 0,
            },
            page_size: 4096,
        };
        let (threads, _) = netdata_agent_log::capture(|| {
            libuv_initialize(&mut c, &system, Path::new("/nonexistent"))
        });
        let (cur, _) = getrlimit(Resource::RLIMIT_STACK).unwrap();
        let expected = if cur == RLIM_INFINITY || cur < 16384 {
            2 << 20
        } else {
            cur - cur % 4096
        };
        assert_eq!(threads.thread_stack_size as u64, expected);
    }

    #[test]
    fn cpu_cores_are_a_size_t_as_in_c() {
        let system = Resources {
            system_cpus: 4,
            memory: crate::system::SystemMemory {
                total: 0,
                available: 0,
            },
            page_size: 4096,
        };
        let cases = [
            ("-1", u64::MAX, 16),
            ("0", 1, 16),
            ("4294967297", 4294967297, 16),
            ("20", 20, 120),
        ];
        for (value, cpus, libuv) in cases {
            let mut c = Config::default();
            c.set(SECTION_GLOBAL, "cpu cores", value);
            let (threads, _) = netdata_agent_log::capture(|| {
                libuv_initialize(&mut c, &system, Path::new("/nonexistent"))
            });
            assert_eq!(
                (threads.cpus, threads.libuv_worker_threads),
                (cpus, libuv),
                "cpu cores = {value}"
            );
        }
    }

    fn loaded(db_section: &str) -> Config {
        let path = std::env::temp_dir().join(format!(
            "nd-conf-db-{}-{}.conf",
            std::process::id(),
            db_section.len()
        ));
        std::fs::write(&path, format!("[db]\n{db_section}")).unwrap();
        let mut c = Config::default();
        assert!(c.load(&path, false, None).is_ok());
        std::fs::remove_file(&path).unwrap();
        c
    }

    #[test]
    fn required_directories_are_entered_created_or_explained() {
        let root = std::env::temp_dir().join(format!("nd-reqdir-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = std::env::current_dir().unwrap();
        let dir = |rel: &str| root.join(rel).to_string_lossy().into_owned();
        std::fs::write(root.join("file"), "").unwrap();
        let created = dir("cache");
        assert_eq!(
            verify_required_directory("E", &created, Some(0o775)),
            Ok(())
        );
        assert!(root.join("cache").is_dir());
        assert_eq!(
            verify_required_directory("E", "relative", None),
            Err((
                0,
                "Invalid directory path (must be an absolute path): 'relative' (E)".to_string()
            ))
        );
        let missing = dir("none/deeper");
        assert_eq!(
            verify_required_directory("E", &missing, Some(0o775)),
            Err((
                nix::errno::Errno::ENOENT as i32,
                format!(
                    "Required directory: '{missing}' (E) - Missing or inaccessible component: '{}'",
                    dir("none")
                )
            ))
        );
        // the component checks cleared the errno of the failed chdir()
        let file = dir("file");
        assert_eq!(
            verify_required_directory("E", &file, None),
            Err((
                0,
                format!(
                    "Required directory: '{file}' (E) - '{file}' exists but is not a directory."
                )
            ))
        );
        std::env::set_current_dir(cwd).unwrap();
        std::fs::remove_dir_all(&root).unwrap();
    }

    struct DbCase {
        file: &'static str,
        want: DbSection,
        /// `[db]` values after the reads, write-backs included.
        values: &'static [(&'static str, &'static str)],
    }

    #[test]
    fn section_db_reads_and_writes_back_like_c() {
        let dbengine = DbSection {
            update_every: 1,
            mode: DbMode::Dbengine,
            history_entries: 3600,
            gap_when_lost_iterations_above: 3,
        };
        let cases = std::collections::BTreeMap::from([
            (
                "defaults",
                DbCase {
                    file: "",
                    want: dbengine,
                    values: &[
                        ("update every", "1s"),
                        ("db", "dbengine"),
                        ("gap when lost iterations above", "1"),
                    ],
                },
            ),
            (
                "clamped",
                DbCase {
                    file: "update every = 0\ngap when lost iterations above = 0\ncleanup orphan hosts after = 5\n\
                           cleanup ephemeral hosts after = 7\ncleanup obsolete charts after = 2\n",
                    want: dbengine,
                    values: &[
                        ("update every", "1s"),
                        ("gap when lost iterations above", "1"),
                        ("cleanup orphan hosts after", "10s"),
                        ("cleanup ephemeral hosts after", "10s"),
                        ("cleanup obsolete charts after", "10s"),
                    ],
                },
            ),
            (
                "ram rounds retention to pages",
                DbCase {
                    file: "db = ram\nretention = 3601\nupdate every = 2h\n",
                    want: DbSection {
                        update_every: 3600,
                        mode: DbMode::Ram,
                        history_entries: 4096,
                        gap_when_lost_iterations_above: 3,
                    },
                    values: &[
                        ("db", "ram"),
                        ("retention", "1h8m16s"),
                        ("update every", "1h"),
                    ],
                },
            ),
            (
                "invalid mode",
                DbCase {
                    file: "db = nosuch\n",
                    want: DbSection {
                        mode: DbMode::Ram,
                        history_entries: 4096,
                        ..dbengine
                    },
                    values: &[("db", "ram"), ("retention", "1h8m16s")],
                },
            ),
        ]);
        for (name, case) in cases {
            let mut c = loaded(case.file);
            let (got, logs) = netdata_agent_log::capture(|| section_db(&mut c, 4096));
            assert_eq!(got, case.want, "{name}: {logs:?}");
            for (key, value) in case.values {
                let v = c
                    .get(SECTION_DB, key, None)
                    .map(|v| String::from_utf8_lossy(&v).into_owned());
                assert_eq!(v.as_deref(), Some(*value), "{name}: [db] {key}");
            }
        }
    }
}
