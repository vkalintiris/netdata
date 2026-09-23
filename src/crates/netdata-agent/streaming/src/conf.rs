//! `stream.conf`, ported from `src/streaming/stream-conf.c`: loading with the legacy renames, the `[stream]` sender
//! settings and the `[db]` replication settings (read in C's order), and the per-connection receiver lookups that
//! try the `[<machine guid>]` section, then the `[<api key>]` section, then the default.

use std::path::Path;

use netdata_agent_inicfg::{Config, LogLevel};
use netdata_agent_text::duration::duration_parse;
use netdata_agent_text::parse::str2ndd;
use netdata_agent_text::simple_pattern::{Separators, SimplePattern, SimplePatternMode};

use crate::caps;

const SECTION_STREAM: &str = "stream";
const SECTION_DB: &str = "db";

/// `CBUFFER_INITIAL_MAX_SIZE`.
const CBUFFER_INITIAL_MAX_SIZE: u64 = 10 * 1024 * 1024;
/// `SENDER_MIN_RECONNECT_DELAY`.
const SENDER_MIN_RECONNECT_DELAY: i64 = 5;
/// `MAX_REPLICATION_THREADS` and `MAX_REPLICATION_PREFETCH`.
const MAX_REPLICATION_THREADS: i64 = 256;
const MAX_REPLICATION_PREFETCH: i64 = 256;
/// `STREAM_COMPRESSION_ALGORITHMS_ORDER`.
pub const COMPRESSION_ALGORITHMS_ORDER: &str = "zstd lz4 brotli gzip";
/// `HEALTH_LOG_RETENTION_DEFAULT`: five days.
const HEALTH_LOG_RETENTION_DEFAULT: i64 = 5 * 86400;
/// `STREAM_RECEIVER_KEEPALIVE_IDLE_MIN_SECONDS` and `_MAX_SECONDS`.
const KEEPALIVE_IDLE_MIN_SECONDS: u64 = 30;
const KEEPALIVE_IDLE_MAX_SECONDS: u64 = 3600;

/// Values `stream_conf_load()` takes from the rest of the daemon.
#[derive(Debug, Clone, Copy)]
pub struct LoadDefaults {
    /// `replication_threads_default()`.
    pub replication_threads: i64,
    /// `libuv_worker_threads`, for `replication_prefetch_default()`.
    pub libuv_worker_threads: i64,
    /// `netdata_ssl_validate_certificate`.
    pub ssl_validate_certificate: bool,
}

/// `struct _stream_send`: this agent as a child.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Send {
    pub enabled: bool,
    pub destination: String,
    pub api_key: String,
    pub send_charts_matching: String,
    pub initial_clock_resync_iterations: i64,
    pub buffer_max_size: u64,
    pub replication_threads: i64,
    pub replication_prefetch: i64,
    pub default_port: i64,
    pub h2o: bool,
    pub timeout_s: i64,
    pub reconnect_delay_s: i64,
    pub ssl_validate_certificate: bool,
    pub ssl_ca_path: Option<String>,
    pub ssl_ca_file: Option<String>,
    pub compression_enabled: bool,
}

impl Default for Send {
    fn default() -> Self {
        Send {
            enabled: false,
            destination: String::new(),
            api_key: String::new(),
            send_charts_matching: String::new(),
            initial_clock_resync_iterations: 60,
            buffer_max_size: CBUFFER_INITIAL_MAX_SIZE,
            replication_threads: 0,
            replication_prefetch: 0,
            default_port: 19999,
            h2o: false,
            timeout_s: 300,
            reconnect_delay_s: 15,
            ssl_validate_certificate: true,
            ssl_ca_path: None,
            ssl_ca_file: None,
            compression_enabled: true,
        }
    }
}

/// `struct _stream_receive`: replication defaults for children.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Replication {
    pub enabled: bool,
    pub period: i64,
    pub step: i64,
}

impl Default for Replication {
    fn default() -> Self {
        Replication {
            enabled: true,
            period: 86400,
            step: 3600,
        }
    }
}

/// `STREAM_RECEIVER_KEEPALIVE_CONFIG`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keepalive {
    pub enabled: bool,
    pub automatic: bool,
    pub idle_s: u32,
}

/// Defaults of a receiver lookup that come from outside `stream.conf`.
#[derive(Debug, Clone)]
pub struct ReceiverDefaults {
    /// `rrd_memory_mode_name(default_rrd_memory_mode)`.
    pub db_mode: String,
    /// `default_rrd_history_entries`.
    pub history: i64,
    /// `health_plugin_enabled()`.
    pub health_enabled: bool,
    /// The `update_every` of the request (or `[db] update every`).
    pub update_every: i64,
}

/// `struct stream_receiver_config`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiverConfig {
    /// The memory mode name, resolved by the caller (`rrd_memory_mode_id()`).
    pub db_mode: String,
    pub history: i64,
    /// `CONFIG_BOOLEAN_*` as returned by `inicfg_get_boolean_ondemand()`.
    pub health_enabled: i32,
    pub health_delay: i64,
    pub health_history: i64,
    pub update_every: i64,
    pub send_enabled: bool,
    pub send_parents: String,
    pub send_api_key: String,
    pub send_charts_matching: String,
    pub replication: Replication,
    pub compression_enabled: bool,
    pub compression_priorities: Vec<u32>,
    pub keepalive: Keepalive,
}

/// The `stream.conf` state (`stream_config`, `stream_send`, `stream_receive`).
#[derive(Debug, Default)]
pub struct StreamConf {
    pub config: Config,
    pub send: Send,
    pub receive: Replication,
    /// `stream_conf_is_parent()`.
    pub is_parent: bool,
}

fn text(v: Option<Vec<u8>>) -> String {
    v.map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

impl StreamConf {
    /// `stream_conf_load_internal()`: the user file, else the stock one, then the renames.
    fn load_file(&mut self, user_dir: &str, stock_dir: &str, log: &mut impl FnMut(LogLevel, &str)) {
        let user = format!("{user_dir}/stream.conf");
        if !self.config.load(Path::new(&user), false, None) {
            log(
                LogLevel::Info,
                &format!("CONFIG: cannot load user config '{user}'. Will try stock config."),
            );
            let stock = format!("{stock_dir}/stream.conf");
            if !self.config.load(Path::new(&stock), false, None) {
                log(
                    LogLevel::Info,
                    &format!(
                        "CONFIG: cannot load stock config '{stock}'. Running with internal defaults."
                    ),
                );
            }
        }
        let c = &mut self.config;
        c.move_option(SECTION_STREAM, "timeout seconds", SECTION_STREAM, "timeout");
        c.move_option(
            SECTION_STREAM,
            "reconnect delay seconds",
            SECTION_STREAM,
            "reconnect delay",
        );
        for (old, new) in [
            ("default memory mode", "db"),
            ("memory mode", "db"),
            ("db mode", "db"),
            ("default history", "retention"),
            ("history", "retention"),
            ("default proxy enabled", "proxy enabled"),
            ("default proxy destination", "proxy destination"),
            ("default proxy api key", "proxy api key"),
            (
                "default proxy send charts matching",
                "proxy send charts matching",
            ),
            ("default health log history", "health log retention"),
            ("health log history", "health log retention"),
            ("seconds to replicate", "replication period"),
            ("seconds per replication step", "replication step"),
            (
                "default postpone alarms on connect seconds",
                "postpone alerts on connect",
            ),
            (
                "postpone alarms on connect seconds",
                "postpone alerts on connect",
            ),
            ("health enabled by default", "health enabled"),
            ("buffer size bytes", "buffer size"),
        ] {
            c.move_everywhere(old, new);
        }
    }

    /// `stream_conf_load()`: `netdata` is `netdata_config`, which holds the `[db]` replication options.
    pub fn load(
        &mut self,
        netdata: &mut Config,
        user_dir: &str,
        stock_dir: &str,
        defaults: LoadDefaults,
        log: &mut impl FnMut(LogLevel, &str),
    ) {
        self.load_file(user_dir, stock_dir, log);
        let c = &mut self.config;
        let s = &mut self.send;
        s.enabled = c.get_boolean(SECTION_STREAM, "enabled", s.enabled);
        s.destination = text(c.get(SECTION_STREAM, "destination", Some("")));
        s.api_key = text(c.get(SECTION_STREAM, "api key", Some("")));
        s.send_charts_matching = text(c.get(SECTION_STREAM, "send charts matching", Some("*")));
        let r = &mut self.receive;
        r.enabled = netdata.get_boolean(SECTION_DB, "enable replication", r.enabled);
        r.period = netdata.get_duration_seconds(SECTION_DB, "replication period", r.period);
        r.step = netdata.get_duration_seconds(SECTION_DB, "replication step", r.step);
        s.replication_threads = netdata.get_number_range(
            SECTION_DB,
            "replication threads",
            defaults.replication_threads,
            1,
            MAX_REPLICATION_THREADS,
        );
        // replication_prefetch_default(), from the thread count just read.
        let threads = s.replication_threads;
        let target = (defaults.libuv_worker_threads / 2).max(threads * 10);
        let prefetch = ((target + threads - 1) / threads).clamp(1, MAX_REPLICATION_PREFETCH);
        s.replication_prefetch = netdata.get_number_range(
            SECTION_DB,
            "replication prefetch",
            prefetch,
            1,
            MAX_REPLICATION_PREFETCH,
        );
        s.buffer_max_size = c.get_size_bytes(SECTION_STREAM, "buffer size", s.buffer_max_size);
        s.reconnect_delay_s =
            c.get_duration_seconds(SECTION_STREAM, "reconnect delay", s.reconnect_delay_s) as u32
                as i64;
        if s.reconnect_delay_s < SENDER_MIN_RECONNECT_DELAY {
            s.reconnect_delay_s = SENDER_MIN_RECONNECT_DELAY;
        }
        s.compression_enabled =
            c.get_boolean(SECTION_STREAM, "enable compression", s.compression_enabled);
        s.h2o = c.get_boolean(SECTION_STREAM, "parent using h2o", s.h2o);
        s.timeout_s = c.get_duration_seconds(SECTION_STREAM, "timeout", s.timeout_s) as i32 as i64;
        s.buffer_max_size = c.get_number(
            SECTION_STREAM,
            "buffer size bytes",
            s.buffer_max_size as i64,
        ) as u64;
        s.default_port = c.get_number(SECTION_STREAM, "default port", s.default_port) as i32 as i64;
        s.initial_clock_resync_iterations = c.get_number(
            SECTION_STREAM,
            "initial clock resync iterations",
            s.initial_clock_resync_iterations,
        ) as u32 as i64;
        s.ssl_validate_certificate = !c.get_boolean(
            SECTION_STREAM,
            "ssl skip certificate verification",
            !defaults.ssl_validate_certificate,
        );
        if !s.ssl_validate_certificate {
            log(
                LogLevel::Info,
                "SSL: streaming senders will skip SSL certificates verification.",
            );
        }
        // string_strdupz() turns empty strings into NULL.
        let non_empty = |v: Option<Vec<u8>>| Some(text(v)).filter(|v| !v.is_empty());
        s.ssl_ca_path = non_empty(c.get_path(SECTION_STREAM, "CApath", None));
        s.ssl_ca_file = non_empty(c.get_filename(SECTION_STREAM, "CAfile", None));
        if s.enabled && (s.destination.is_empty() || s.api_key.is_empty()) {
            let state = |v: &str| if v.is_empty() { "missing" } else { "present" };
            log(
                LogLevel::Error,
                &format!(
                    "STREAM [send]: cannot enable sending thread - missing required fields (destination: {}, api key: {})",
                    state(&s.destination),
                    state(&s.api_key),
                ),
            );
            s.enabled = false;
        }
        self.is_parent = self.config.stream_conf_has_api_enabled();
    }

    /// `stream_conf_is_key_type()`: a missing `type` counts as the one asked for.
    pub fn is_key_type(&mut self, key: &str, kind: &str) -> bool {
        let found = text(self.config.get(key, "type", Some(kind)));
        let found = if found.is_empty() { "unknown" } else { &found };
        found == kind
    }

    /// `stream_conf_api_key_is_enabled()`.
    pub fn api_key_is_enabled(&mut self, key: &str, enabled: bool) -> bool {
        self.config.get_boolean(key, "enabled", enabled)
    }

    /// `stream_conf_api_key_allows_client()`: `allow from` (default `*`) against the client IP, case-insensitive.
    pub fn api_key_allows_client(&mut self, key: &str, client_ip: &str) -> bool {
        let allow = self
            .config
            .get(key, "allow from", Some("*"))
            .unwrap_or_default();
        let pattern = SimplePattern::new(
            &allow,
            Separators::Whitespace,
            SimplePatternMode::Exact,
            true,
        );
        pattern.is_empty() || pattern.matches(client_ip.as_bytes())
    }

    /// `stream_conf_receiver_config()`. Each value tries `[guid]`, then `[key]`, then the default; C evaluates the
    /// `[key]` lookup first, as the argument of the `[guid]` one.
    pub fn receiver_config(
        &mut self,
        key: &str,
        guid: &str,
        defaults: &ReceiverDefaults,
    ) -> ReceiverConfig {
        let c = &mut self.config;
        let key_db = text(c.get(key, "db", Some(&defaults.db_mode)));
        let db_mode = text(c.get(guid, "db", Some(&key_db)));
        let key_history = c.get_number(key, "retention", defaults.history);
        let history = c.get_number(guid, "retention", key_history).max(5) as i32 as i64;
        let key_health =
            c.get_boolean_ondemand(key, "health enabled", i32::from(defaults.health_enabled));
        let health_enabled = c.get_boolean_ondemand(guid, "health enabled", key_health);
        let key_delay = c.get_duration_seconds(key, "postpone alerts on connect", 60);
        let health_delay = c.get_duration_seconds(guid, "postpone alerts on connect", key_delay);
        let mut update_every =
            c.get_duration_seconds(guid, "update every", defaults.update_every) as i32 as i64;
        if update_every < 0 {
            update_every = 1;
        }
        let key_hh =
            c.get_duration_seconds(key, "health log retention", HEALTH_LOG_RETENTION_DEFAULT);
        let health_history = c.get_duration_seconds(guid, "health log retention", key_hh);
        let send = &self.send;
        let key_send = c.get_boolean(key, "proxy enabled", send.enabled);
        let send_enabled = c.get_boolean(guid, "proxy enabled", key_send);
        let key_parents = text(c.get(key, "proxy destination", Some(&send.destination)));
        let send_parents = text(c.get(guid, "proxy destination", Some(&key_parents)));
        let key_api = text(c.get(key, "proxy api key", Some(&send.api_key)));
        let send_api_key = text(c.get(guid, "proxy api key", Some(&key_api)));
        let key_match = text(c.get(
            key,
            "proxy send charts matching",
            Some(&send.send_charts_matching),
        ));
        let send_charts_matching =
            text(c.get(guid, "proxy send charts matching", Some(&key_match)));
        let r = self.receive;
        let key_repl = c.get_boolean(key, "enable replication", r.enabled);
        let key_period = c.get_duration_seconds(key, "replication period", r.period);
        let key_step = c.get_duration_seconds(key, "replication step", r.step);
        let replication = Replication {
            enabled: c.get_boolean(guid, "enable replication", key_repl),
            period: c.get_duration_seconds(guid, "replication period", key_period),
            step: c.get_duration_seconds(guid, "replication step", key_step),
        };
        let key_comp = c.get_boolean(key, "enable compression", send.compression_enabled);
        let compression_enabled = c.get_boolean(guid, "enable compression", key_comp);
        let compression_priorities = if compression_enabled {
            let key_order = text(c.get(
                key,
                "compression algorithms order",
                Some(COMPRESSION_ALGORITHMS_ORDER),
            ));
            let order = c
                .get(guid, "compression algorithms order", Some(&key_order))
                .unwrap_or_default();
            caps::parse_compression_order(&order, caps::COMPRESSIONS_AVAILABLE)
        } else {
            Vec::new()
        };
        let keepalive = self.receiver_keepalive(key, guid);
        ReceiverConfig {
            db_mode,
            history,
            health_enabled,
            health_delay,
            health_history,
            update_every,
            send_enabled,
            send_parents,
            send_api_key,
            send_charts_matching,
            replication,
            compression_enabled,
            compression_priorities,
            keepalive,
        }
    }

    /// `stream_conf_resolve_receiver_keepalive()`: `tcp keepalive idle` from the first section that has it.
    fn receiver_keepalive(&mut self, key: &str, guid: &str) -> Keepalive {
        let name = "tcp keepalive idle";
        let section = if self.config.exists(guid, name) {
            Some(guid)
        } else if self.config.exists(key, name) {
            Some(key)
        } else {
            None
        };
        let value = match section {
            Some(section) => self
                .config
                .get(section, name, Some("auto"))
                .unwrap_or_default(),
            None => b"auto".to_vec(),
        };
        parse_keepalive(&value)
    }
}

/// `stream_conf_parse_receiver_keepalive()`: `auto` (or anything unparsable, negative, or a zero ending in `ago`)
/// is automatic; an explicit zero (`0`, `off`, `never`, `0s`) disables; otherwise whole seconds rounded up and
/// clamped to 30..3600.
pub fn parse_keepalive(value: &[u8]) -> Keepalive {
    let automatic = Keepalive {
        enabled: true,
        automatic: true,
        idle_s: 0,
    };
    if value.eq_ignore_ascii_case(b"auto") {
        return automatic;
    }
    let trimmed = value.trim_ascii_start();
    let Some(ns) = duration_parse(value, "s", "ns") else {
        return automatic;
    };
    if trimmed.first() == Some(&b'-') || ns < 0 {
        return automatic;
    }
    let explicit_zero = ns == 0 && duration_is_explicit_zero(value);
    if ns == 0 && !explicit_zero && has_ago_suffix(value) {
        return automatic;
    }
    if explicit_zero {
        return Keepalive {
            enabled: false,
            automatic: false,
            idle_s: 0,
        };
    }
    let seconds = if ns > 0 {
        1 + (ns as u64 - 1) / 1_000_000_000
    } else {
        1
    };
    Keepalive {
        enabled: true,
        automatic: false,
        idle_s: seconds.clamp(KEEPALIVE_IDLE_MIN_SECONDS, KEEPALIVE_IDLE_MAX_SECONDS) as u32,
    }
}

/// `stream_conf_duration_is_explicit_zero()`: `off`, `never`, or numbers written without a non-zero digit.
fn duration_is_explicit_zero(value: &[u8]) -> bool {
    let mut s = value.trim_ascii_start();
    if s.eq_ignore_ascii_case(b"off") || s.eq_ignore_ascii_case(b"never") {
        return true;
    }
    let mut parsed_number = false;
    while !s.is_empty() {
        s = s.trim_ascii_start();
        if s.is_empty() || s.eq_ignore_ascii_case(b"ago") {
            break;
        }
        let (_, used) = str2ndd(s);
        if used == 0 {
            return false;
        }
        parsed_number = true;
        if s[..used]
            .iter()
            .take_while(|&&c| c != b'e' && c != b'E')
            .any(|c| (b'1'..=b'9').contains(c))
        {
            return false;
        }
        s = s[used..].trim_ascii_start();
        let alpha = s.iter().take_while(|c| c.is_ascii_alphabetic()).count();
        s = &s[alpha..];
    }
    parsed_number
}

/// `stream_conf_duration_has_ago_suffix()`.
fn has_ago_suffix(value: &[u8]) -> bool {
    let end = value.trim_ascii_end();
    end.len() >= 3 && end[end.len() - 3..].eq_ignore_ascii_case(b"ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_parsing() {
        let auto = Keepalive {
            enabled: true,
            automatic: true,
            idle_s: 0,
        };
        let off = Keepalive {
            enabled: false,
            automatic: false,
            idle_s: 0,
        };
        let idle = |s| Keepalive {
            enabled: true,
            automatic: false,
            idle_s: s,
        };
        let cases: [(&[u8], Keepalive); 9] = [
            (b"auto", auto),
            (b"AUTO", auto),
            (b"garbage", auto),
            (b"-5", auto),
            (b"0", off),
            (b"off", off),
            (b"5", idle(30)),
            (b"2h", idle(3600)),
            (b"90s", idle(90)),
        ];
        for (value, expected) in cases {
            assert_eq!(
                parse_keepalive(value),
                expected,
                "{}",
                String::from_utf8_lossy(value)
            );
        }
    }

    #[test]
    fn receiver_lookups_fall_back_from_guid_to_key() {
        let mut sc = StreamConf::default();
        sc.config.load_bytes(
            b"[11111111-2222-3333-4444-555555555555]\n  enabled = yes\n  db = alloc\n  retention = 3\n\
              [aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee]\n  type = machine\n  db = ram\n",
            "stream.conf",
            false,
            None,
        );
        let key = "11111111-2222-3333-4444-555555555555";
        let guid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        assert!(sc.is_key_type(key, "api"));
        assert!(!sc.is_key_type(guid, "api"));
        assert!(sc.api_key_is_enabled(key, false));
        assert!(sc.api_key_allows_client(key, "localhost"));
        let defaults = ReceiverDefaults {
            db_mode: "dbengine".into(),
            history: 3600,
            health_enabled: true,
            update_every: 1,
        };
        let rc = sc.receiver_config(key, guid, &defaults);
        assert_eq!(
            (rc.db_mode.as_str(), rc.history, rc.update_every),
            ("ram", 5, 1)
        );
        assert_eq!(rc.replication, Replication::default());
        assert!(rc.compression_enabled);
        assert!(rc.keepalive.automatic);
    }
}
