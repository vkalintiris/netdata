//! Configuration loading and the section readers, ported from `src/daemon/config/` and the `[directories]` reader
//! in `src/libnetdata/runtime-paths/runtime-paths.c`. Each function mirrors the C function of the same name and is
//! called from the same point of the startup sequence, because `/netdata.conf` lists options in first-read order.

use std::path::Path;

use netdata_agent_inicfg::{
    Config, LogLevel, SECTION_CLOUD, SECTION_DB, SECTION_DIRECTORIES, SECTION_ENV_VARS,
    SECTION_GLOBAL, SECTION_HEALTH, SECTION_LOGS, SECTION_PLUGINS, SECTION_PULSE, SECTION_REGISTRY,
    SECTION_STATSD, SECTION_WEB,
};

use netdata_agent_text::line_splitter::{Separators, quoted_strings_splitter};
use netdata_agent_text::simple_pattern::{
    Separators as SimpleSeparators, SimplePattern, SimplePatternMode,
};

use crate::acl::{AclPattern, WebAcl};
use crate::build;

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
    // FUNCTION_RUN_ONCE guards.
    loaded: bool,
    compat_done: bool,
    directories_done: bool,
}

fn text(v: Option<Vec<u8>>) -> String {
    v.map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

impl Conf {
    /// Emits the log lines the configuration engines queued.
    pub fn flush_log(&mut self, log: &mut impl FnMut(LogLevel, &str)) {
        for line in self
            .netdata
            .take_log()
            .into_iter()
            .chain(self.cloud.take_log())
        {
            log(line.level, &line.message);
        }
    }

    /// `netdata_conf_load()`: the given file, or the user file then the stock one. Runs once: a second call (a
    /// second `-c`) returns false, and the C daemon exits.
    pub fn netdata_conf_load(
        &mut self,
        filename: Option<&str>,
        overwrite_used: bool,
        log: &mut impl FnMut(LogLevel, &str),
    ) -> bool {
        if self.loaded {
            return false;
        }
        self.loaded = true;
        let ret = match filename.filter(|f| !f.is_empty()) {
            Some(filename) => {
                let ret = self.netdata.load(Path::new(filename), overwrite_used, None);
                if !ret {
                    log(
                        LogLevel::Error,
                        &format!("CONFIG: cannot load config file '{filename}'."),
                    );
                }
                ret
            }
            None => {
                let user = format!("{}/{}", self.dirs.user_config, build::CONFIG_FILENAME);
                let mut ret = self.netdata.load(Path::new(&user), overwrite_used, None);
                if !ret {
                    log(
                        LogLevel::Info,
                        &format!(
                            "CONFIG: cannot load user config '{user}'. Will try the stock version."
                        ),
                    );
                    let stock = format!("{}/{}", self.dirs.stock_config, build::CONFIG_FILENAME);
                    ret = self.netdata.load(Path::new(&stock), overwrite_used, None);
                    if !ret {
                        log(
                            LogLevel::Info,
                            &format!(
                                "CONFIG: cannot load stock config '{stock}'. Running with internal defaults."
                            ),
                        );
                    }
                }
                ret
            }
        };
        self.backwards_compatibility();
        self.section_directories();
        self.section_global_run_as_user();
        // libuv_initialize() reads [global] pthread stack size, cpu cores and libuv worker threads here; ported
        // with the thread-sizing work (agent/progress.md).
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
    pub fn cloud_conf_load(&mut self, silent: bool, log: &mut impl FnMut(LogLevel, &str)) {
        self.section_directories();
        let filename = format!("{}/cloud.conf", self.dirs.cloud);
        if !self.cloud.load(Path::new(&filename), true, None) && !silent {
            log(
                LogLevel::Error,
                &format!(
                    "CLAIM: cannot load cloud config '{filename}'. Running with internal defaults."
                ),
            );
        }
        let c = &mut self.cloud;
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

    /// `nd_runtime_paths_load_hostname_from_inicfg()`: `[global] host access prefix`, then `hostname`.
    pub fn section_global_hostname(&mut self) {
        self.host_prefix = text(
            self.netdata
                .get(SECTION_GLOBAL, "host access prefix", Some("")),
        );
        let system = nix::unistd::gethostname()
            .map(|h| h.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.hostname = text(self.netdata.get(SECTION_GLOBAL, "hostname", Some(&system)));
    }
}

/// What `netdata_conf_section_web()` configures.
pub struct WebConf {
    pub respect_do_not_track: bool,
    pub x_frame_options: Option<String>,
    pub acl: WebAcl,
    pub gzip: bool,
    pub gzip_level: u32,
}

/// The zlib strategies `[web] gzip compression strategy` accepts.
const GZIP_STRATEGIES: [&str; 5] = ["default", "filtered", "huffman only", "rle", "fixed"];

/// `netdata_conf_web_query_threads()`: two per CPU on a parent (at most 256 CPUs), at least 6, unless configured.
pub fn web_query_threads(
    c: &mut Config,
    cpus: usize,
    is_parent: bool,
    log: &mut impl FnMut(LogLevel, &str),
) -> usize {
    let cpus = cpus.min(256);
    let threads = (cpus * if is_parent { 2 } else { 1 }).max(6);
    let threads = c.get_number(SECTION_WEB, "web server threads", threads as i64);
    if threads < 1 {
        log(
            LogLevel::Error,
            "[web].web server threads in netdata.conf needs to be at least 1. Overwriting it.",
        );
        c.set_number(SECTION_WEB, "web server threads", 1);
        return 1;
    }
    threads as usize
}

impl Conf {
    /// `netdata_conf_section_web()`.
    pub fn section_web(&mut self, log: &mut impl FnMut(LogLevel, &str)) -> WebConf {
        let c = &mut self.netdata;
        // Read in C's order (they are listed in /netdata.conf), applied later: the idle and first-request timeouts come
        // with the web worker timers, the streaming rate with the receiver's admission pacing.
        let _disconnect_idle_after_s =
            c.get_duration_seconds(SECTION_WEB, "disconnect idle clients after", 60);
        let _first_request_timeout_s =
            c.get_duration_seconds(SECTION_WEB, "timeout for first request", 60);
        let _streaming_rate_s =
            c.get_duration_seconds(SECTION_WEB, "accept a streaming request every", 0);
        let respect_do_not_track = c.get_boolean(SECTION_WEB, "respect do not track policy", false);
        let x_frame_options = Some(text(c.get(
            SECTION_WEB,
            "x-frame-options response header",
            Some(""),
        )))
        .filter(|x| !x.is_empty());
        let mut acl_pattern = |c: &mut Config,
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
            let dns = make_dns_decision(c, section, dns_name, dns_default, &pattern, log);
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
                log(
                    LogLevel::Error,
                    &format!(
                        "Invalid compression strategy '{strategy}'. Valid strategies are 'default', 'filtered', 'huffman only', 'rle' and 'fixed'. Proceeding with 'default'."
                    ),
                );
            }
        }
        let level = c.get_number(SECTION_WEB, "gzip compression level", 3) as i32;
        let gzip_level = if level < 1 {
            log(
                LogLevel::Error,
                &format!(
                    "Invalid compression level {level}. Valid levels are 1 (fastest) to 9 (best ratio). Proceeding with level 1 (fastest compression)."
                ),
            );
            1
        } else if level > 9 {
            log(
                LogLevel::Error,
                &format!(
                    "Invalid compression level {level}. Valid levels are 1 (fastest) to 9 (best ratio). Proceeding with level 9 (best compression)."
                ),
            );
            9
        } else {
            level as u32
        };
        WebConf {
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
    log: &mut impl FnMut(LogLevel, &str),
) -> bool {
    let value = text(c.get(section, name, Some(default)));
    match value.as_str() {
        "yes" => true,
        "no" => false,
        other => {
            if other != "heuristic" {
                log(
                    LogLevel::Error,
                    &format!(
                        "Invalid configuration option '{other}' for '{section}'/'{name}'. Valid options are 'yes', 'no' and 'heuristic'. Proceeding with 'heuristic'"
                    ),
                );
            }
            pattern.is_potential_name()
        }
    }
}
