//! The receiver side of the stream handshake, ported from `stream_receiver_accept_connection()`
//! (`src/streaming/stream-receiver-connection.c`): the `STREAM` query parameters, the validation that happens before
//! any host is touched, and the bytes of every answer (`src/streaming/stream-handshake.h`).

use netdata_agent_rrd::system_info::SystemInfo;
use netdata_agent_text::c::strsep_skip;
use netdata_agent_text::parse::{str2i, strtoll0, strtoul0};

use crate::caps;
use crate::conf::StreamConf;

/// `START_STREAMING_ERROR_SAME_LOCALHOST`.
pub const ERROR_SAME_LOCALHOST: &str =
    "Don't hit me baby, you are trying to stream my localhost back";
/// `START_STREAMING_ERROR_LOCAL_VNODE`.
pub const ERROR_LOCAL_VNODE: &str = "Don't hit me baby, you are trying to stream my vnode back";
/// `START_STREAMING_ERROR_ALREADY_STREAMING`.
pub const ERROR_ALREADY_STREAMING: &str = "This GUID is already streaming to this server";
/// `START_STREAMING_ERROR_NOT_PERMITTED`.
pub const ERROR_NOT_PERMITTED: &str =
    "You are not permitted to access this. Check the logs for more info.";
/// `START_STREAMING_ERROR_BUSY_TRY_LATER`.
pub const ERROR_BUSY_TRY_LATER: &str =
    "The server is too busy now to accept this request. Try later.";
/// `START_STREAMING_ERROR_INTERNAL_ERROR`.
pub const ERROR_INTERNAL_ERROR: &str = "The server encountered an internal error. Try later.";
/// `START_STREAMING_ERROR_INITIALIZATION`.
pub const ERROR_INITIALIZATION: &str = "The server is initializing. Try later.";

/// What a child sent in its `STREAM` request line and `User-Agent` header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreamRequest {
    pub key: Option<String>,
    pub hostname: Option<String>,
    pub registry_hostname: Option<String>,
    pub machine_guid: Option<String>,
    /// `rpt->config.update_every`: the last `update_every`, else the caller's default.
    pub update_every: i32,
    /// `rpt->handshake_update_every`: 0 when the child sent none.
    pub handshake_update_every: i32,
    pub os: Option<String>,
    pub timezone: Option<String>,
    pub abbrev_timezone: Option<String>,
    pub utc_offset: i32,
    pub hops: i16,
    pub invalid_hops: bool,
    pub capabilities: u32,
    pub system_info: SystemInfo,
    pub program_name: Option<String>,
    pub program_version: Option<String>,
    /// Parameters nobody uses, in order (C logs each at NOTICE).
    pub unused: Vec<(Option<String>, String, String)>,
}

/// `stream_receiver_parse_hops()`: base 0, the whole string, 1..=32767.
fn parse_hops(value: &str) -> Option<i16> {
    let (parsed, used) = strtoll0(value.as_bytes());
    (used > 0 && used == value.len() && (1..=i64::from(i16::MAX)).contains(&parsed))
        .then_some(parsed as i16)
}

impl StreamRequest {
    /// Parses the url-decoded query (everything between `STREAM ` and ` HTTP/`). Pairs are split on runs of `&`,
    /// names at the first run of `=`; a pair without a value is ignored. A repeated identity field (`key`, `os`, ...)
    /// or a `ver` after the capabilities are known goes to the system-info fallback, as in C, and is reported unused.
    pub fn parse(decoded: &[u8], default_update_every: i32, user_agent: Option<&[u8]>) -> Self {
        let mut r = StreamRequest {
            update_every: default_update_every,
            hops: 1,
            capabilities: caps::INVALID,
            ..StreamRequest::default()
        };
        r.system_info.hops = 1;
        let mut rest = Some(decoded);
        while rest.is_some() {
            let mut value = Some(strsep_skip(&mut rest, b"&"));
            if value.is_some_and(<[u8]>::is_empty) {
                continue;
            }
            let name = strsep_skip(&mut value, b"=");
            let Some(value) = value.filter(|v| !v.is_empty()) else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(name).into_owned();
            let value = String::from_utf8_lossy(value).into_owned();
            match name.as_str() {
                "key" if r.key.is_none() => r.key = Some(value),
                "hostname" if r.hostname.is_none() => r.hostname = Some(value),
                "registry_hostname" if r.registry_hostname.is_none() => {
                    r.registry_hostname = Some(value)
                }
                // whole, as C keeps it: only the host created for it keeps 37 characters (D126.7)
                "machine_guid" if r.machine_guid.is_none() => r.machine_guid = Some(value),
                "update_every" => {
                    r.update_every = strtoul0(value.as_bytes()).0 as i32;
                    r.handshake_update_every = r.update_every;
                }
                "os" if r.os.is_none() => r.os = Some(value),
                "timezone" if r.timezone.is_none() => r.timezone = Some(value),
                "abbrev_timezone" if r.abbrev_timezone.is_none() => r.abbrev_timezone = Some(value),
                "utc_offset" => r.utc_offset = strtoll0(value.as_bytes()).0 as i32,
                "hops" => match parse_hops(&value) {
                    Some(hops) => {
                        r.hops = hops;
                        r.system_info.hops = hops;
                    }
                    None => r.invalid_hops = true,
                },
                "ml_capable" => r.system_info.ml_capable = str2i(value.as_bytes()) != 0,
                "ml_enabled" => r.system_info.ml_enabled = str2i(value.as_bytes()) != 0,
                "mc_version" => r.system_info.mc_version = str2i(value.as_bytes()),
                "ver" if r.capabilities & caps::INVALID != 0 => {
                    r.capabilities = caps::from_version(strtoul0(value.as_bytes()).0 as i32);
                }
                _ => {
                    let renamed = match name.as_str() {
                        "NETDATA_SYSTEM_OS_NAME" => "NETDATA_HOST_OS_NAME",
                        "NETDATA_SYSTEM_OS_ID" => "NETDATA_HOST_OS_ID",
                        "NETDATA_SYSTEM_OS_ID_LIKE" => "NETDATA_HOST_OS_ID_LIKE",
                        "NETDATA_SYSTEM_OS_VERSION" => "NETDATA_HOST_OS_VERSION",
                        "NETDATA_SYSTEM_OS_VERSION_ID" => "NETDATA_HOST_OS_VERSION_ID",
                        "NETDATA_SYSTEM_OS_DETECTION" => "NETDATA_HOST_OS_DETECTION",
                        other => {
                            if other == "NETDATA_PROTOCOL_VERSION"
                                && r.capabilities & caps::INVALID != 0
                            {
                                r.capabilities = caps::from_version(1);
                            }
                            other
                        }
                    };
                    if !r.system_info.set_by_name(renamed, &value) {
                        // C logs it while parsing, with the hostname known so far
                        r.unused
                            .push((r.hostname.clone(), renamed.to_string(), value));
                    }
                }
            }
        }
        if r.capabilities & caps::INVALID != 0 {
            r.capabilities = caps::from_version(0);
        }
        if let Some(agent) = user_agent.filter(|a| !a.is_empty()) {
            let agent = String::from_utf8_lossy(agent);
            match agent.split_once('/') {
                Some((name, version)) => {
                    r.program_name = Some(name.to_string());
                    if !version.is_empty() {
                        r.program_version = Some(version.to_string());
                    }
                }
                None => r.program_name = Some(agent.into_owned()),
            }
        }
        r
    }
}

/// Why a connection is refused before any host lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    InvalidHops,
    NoKey,
    NoHostname,
    NoMachineGuid,
    KeyIsMachineGuid,
    KeyNotEnabled,
    KeyNotAllowedFromIp,
    MachineGuidIsKey,
    MachineGuidNotEnabled,
    MachineGuidNotAllowedFromIp,
}

impl Denied {
    /// The reason C logs (`stream_receiver_log_status()`).
    pub fn message(self) -> &'static str {
        match self {
            Denied::InvalidHops => {
                "rejecting streaming connection; request has an invalid hops value"
            }
            Denied::NoKey => "rejecting streaming connection; request without an API key",
            Denied::NoHostname => "rejecting streaming connection; request without a hostname",
            Denied::NoMachineGuid => {
                "rejecting streaming connection; request without a machine UUID"
            }
            Denied::KeyIsMachineGuid => {
                "rejecting streaming connection; API key provided is a machine UUID (did you mix them up?)"
            }
            Denied::KeyNotEnabled => {
                "rejecting streaming connection; API key is not enabled in stream.conf"
            }
            Denied::KeyNotAllowedFromIp => {
                "rejecting streaming connection; API key is not allowed from this IP"
            }
            Denied::MachineGuidIsKey => {
                "rejecting streaming connection; machine UUID is an API key (did you mix them up?)"
            }
            Denied::MachineGuidNotEnabled => {
                "rejecting streaming connection; machine UUID is not enabled in stream.conf"
            }
            Denied::MachineGuidNotAllowedFromIp => {
                "rejecting streaming connection; machine UUID is not allowed from this IP"
            }
        }
    }
}

/// The checks of `stream_receiver_accept_connection()` up to the host lookup, in C's order; fills
/// `registry_hostname` from `hostname` as C does. Every refusal answers `ERROR_NOT_PERMITTED` with code 401. The
/// C checks that the key and the machine GUID parse as UUIDs are unreachable and not ported.
pub fn validate(
    r: &mut StreamRequest,
    conf: &mut StreamConf,
    client_ip: &str,
) -> Result<(), Denied> {
    let present = |v: &Option<String>| v.as_deref().is_some_and(|v| !v.is_empty());
    if r.invalid_hops {
        return Err(Denied::InvalidHops);
    }
    if !present(&r.key) {
        return Err(Denied::NoKey);
    }
    if !present(&r.hostname) {
        return Err(Denied::NoHostname);
    }
    if r.registry_hostname.is_none() {
        r.registry_hostname = r.hostname.clone();
    }
    if !present(&r.machine_guid) {
        return Err(Denied::NoMachineGuid);
    }
    let key = r.key.as_deref().unwrap_or_default();
    let guid = r.machine_guid.as_deref().unwrap_or_default();
    if !conf.is_key_type(key, "api") {
        return Err(Denied::KeyIsMachineGuid);
    }
    if !conf.api_key_is_enabled(key, false) {
        return Err(Denied::KeyNotEnabled);
    }
    if !conf.api_key_allows_client(key, client_ip) {
        return Err(Denied::KeyNotAllowedFromIp);
    }
    if !conf.is_key_type(guid, "machine") {
        return Err(Denied::MachineGuidIsKey);
    }
    if !conf.api_key_is_enabled(guid, true) {
        return Err(Denied::MachineGuidNotEnabled);
    }
    if !conf.api_key_allows_client(guid, client_ip) {
        return Err(Denied::MachineGuidNotAllowedFromIp);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "11111111-2222-3333-4444-555555555555";
    const GUID: &str = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";

    #[test]
    fn parameters_follow_c_rules() {
        let q = format!(
            "&&key={KEY}&key=other&hostname=child&machine_guid={GUID}xyz&update_every=010&hops=2&ver=17088\
             &ver=1&==x&empty=&NETDATA_SYSTEM_OS_NAME=Linux&tags=a=b&utc_offset=-0x10"
        );
        let r = StreamRequest::parse(q.as_bytes(), 1, Some(b"query-corpus-pusher/1.0"));
        assert_eq!(r.key.as_deref(), Some(KEY));
        assert_eq!(r.machine_guid.as_deref(), Some(format!("{GUID}xyz").as_str()));
        assert_eq!(
            (r.update_every, r.handshake_update_every, r.hops),
            (8, 8, 2)
        );
        assert_eq!(r.capabilities, 17088);
        assert_eq!(r.utc_offset, -16);
        assert_eq!(r.system_info.host_os_name.as_deref(), Some("Linux"));
        // Repeats fall through to the system-info names; "==x" is the pair ("x", nothing); "tags" keeps everything
        // after its first '='.
        // each with the hostname parsed before it, as C logs them while parsing
        let unused: Vec<_> = r
            .unused
            .iter()
            .map(|(h, n, v)| (h.as_deref(), n.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            unused,
            [
                (None, "key", "other"),
                (Some("child"), "ver", "1"),
                (Some("child"), "tags", "a=b")
            ]
        );
        assert_eq!(
            (r.program_name.as_deref(), r.program_version.as_deref()),
            (Some("query-corpus-pusher"), Some("1.0"))
        );
        let r = StreamRequest::parse(b"NETDATA_PROTOCOL_VERSION=1.1&ver=17088&hops=0", 1, None);
        assert_eq!(r.capabilities, caps::V1);
        assert!(r.invalid_hops);
        assert_eq!(StreamRequest::parse(b"", 1, None).capabilities, caps::V1);
    }

    /// `stream_receiver_accept_connection()`: `key`, `hostname`, `registry_hostname`, `machine_guid`, `os`, `timezone`
    /// and `abbrev_timezone` keep their first value (a repeat falls through to the system-info names, unused);
    /// `update_every`, `utc_offset`, `hops`, `ml_capable`, `ml_enabled` and `mc_version` keep their last.
    #[test]
    fn repeated_parameters_keep_the_first_or_the_last_as_c() {
        let first = ["key", "hostname", "registry_hostname", "machine_guid", "os", "timezone", "abbrev_timezone"];
        let mut q: Vec<String> = first.iter().flat_map(|n| [format!("{n}={n}1"), format!("{n}={n}2")]).collect();
        q.extend(
            ["update_every=3", "update_every=4", "utc_offset=5", "utc_offset=6", "hops=1", "hops=2"]
                .map(String::from),
        );
        q.extend(
            ["ml_capable=1", "ml_capable=0", "ml_enabled=1", "ml_enabled=0", "mc_version=7", "mc_version=8"]
                .map(String::from),
        );
        q.push("ver=17088".into());
        let r = StreamRequest::parse(q.join("&").as_bytes(), 1, None);
        let kept = [
            &r.key,
            &r.hostname,
            &r.registry_hostname,
            &r.machine_guid,
            &r.os,
            &r.timezone,
            &r.abbrev_timezone,
        ]
        .map(|v| v.as_deref().unwrap_or_default().to_string());
        assert_eq!(kept, first.map(|n| format!("{n}1")));
        let unused: Vec<_> = r.unused.iter().map(|(_, n, v)| (n.as_str(), v.as_str())).collect();
        let repeats: Vec<_> = first.iter().map(|n| format!("{n}2")).collect();
        assert_eq!(unused, first.iter().zip(&repeats).map(|(n, v)| (*n, v.as_str())).collect::<Vec<_>>());
        let si = &r.system_info;
        assert_eq!(
            (r.update_every, r.utc_offset, r.hops, si.ml_capable, si.ml_enabled, si.mc_version),
            (4, 6, 2, false, false, 8)
        );
    }

    /// `ver` is `strtoul(…, 0)` (hex and octal, cut to `int32_t`); `ml_capable`, `ml_enabled` and `mc_version` are
    /// `str2i()` (decimal only, a sign, stops at the first non-digit).
    #[test]
    fn ver_is_base_0_and_the_ml_fields_are_str2i() {
        let caps_of = |q: &str| StreamRequest::parse(q.as_bytes(), 1, None).capabilities;
        assert_eq!(
            [caps_of("ver=0x42C0"), caps_of("ver=041300"), caps_of("ver=4294967298"), caps_of("ver=-1")],
            [17088, 17088, caps::V2 | caps::HLABELS, caps::V1]
        );
        let ml = |q: &str| {
            let si = StreamRequest::parse(q.as_bytes(), 1, None).system_info;
            (si.ml_capable, si.ml_enabled, si.mc_version)
        };
        assert_eq!(ml("ml_capable=0x1&ml_enabled=+2&mc_version=010"), (false, true, 10));
        assert_eq!(ml("ml_capable= 3&ml_enabled=-0&mc_version=-7x"), (true, false, -7));
    }

    #[test]
    fn validation_order() {
        let conf = || {
            let mut conf = StreamConf::default();
            conf.config.load_bytes(
                format!("[{KEY}]\n  enabled = yes\n  allow from = 10.*\n").as_bytes(),
                "stream.conf",
                false,
                None,
            );
            conf
        };
        let check = |conf: &mut StreamConf, q: &str, ip: &str| {
            let mut r = StreamRequest::parse(q.as_bytes(), 1, None);
            validate(&mut r, conf, ip)
        };
        let full = format!("key={KEY}&hostname=h&machine_guid={GUID}");
        let mut c = conf();
        assert_eq!(
            check(&mut c, &format!("{full}&hops=x"), "10.0.0.1"),
            Err(Denied::InvalidHops)
        );
        assert_eq!(check(&mut c, "hostname=h", "10.0.0.1"), Err(Denied::NoKey));
        assert_eq!(
            check(&mut c, &format!("key={KEY}"), "10.0.0.1"),
            Err(Denied::NoHostname)
        );
        assert_eq!(
            check(&mut c, &full, "localhost"),
            Err(Denied::KeyNotAllowedFromIp)
        );
        assert_eq!(check(&mut c, &full, "10.0.0.1"), Ok(()));
        // Lookups create the options they read: a request that uses the machine GUID as its key records
        // `type = api` in the GUID's section, and the GUID is refused as a machine GUID from then on.
        let mut c = conf();
        let swapped = format!("key={GUID}&hostname=h&machine_guid={GUID}");
        assert_eq!(
            check(&mut c, &swapped, "10.0.0.1"),
            Err(Denied::KeyNotEnabled)
        );
        assert_eq!(
            check(&mut c, &full, "10.0.0.1"),
            Err(Denied::MachineGuidIsKey)
        );
    }
}
