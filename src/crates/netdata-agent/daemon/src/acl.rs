//! HTTP ACLs, ported from `HTTP_ACL` (`src/libnetdata/user-auth/http-access.h`), `read_acl()` and the ACL part of
//! `bind_to_this()` (`src/libnetdata/socket/listen-sockets.c`), `connection_allowed()` (`socket.c`) and
//! `web_client_update_acl_matches()` (`src/web/server/web_server.c`).

use netdata_agent_log::{Priority, Source, nd_log};
use std::net::IpAddr;

use netdata_agent_text::simple_pattern::SimplePattern;

/// `HTTP_ACL`.
pub mod bits {
    pub const NOCHECK: u32 = 1 << 0;
    pub const API: u32 = 1 << 1;
    pub const API_UDP: u32 = 1 << 2;
    pub const API_UNIX: u32 = 1 << 3;
    pub const ACLK: u32 = 1 << 5;
    pub const WEBRTC: u32 = 1 << 6;
    pub const METRICS: u32 = 1 << 10;
    pub const FUNCTIONS: u32 = 1 << 11;
    pub const NODES: u32 = 1 << 12;
    pub const ALERTS: u32 = 1 << 13;
    pub const DYNCFG: u32 = 1 << 14;
    pub const REGISTRY: u32 = 1 << 15;
    pub const BADGES: u32 = 1 << 16;
    pub const MANAGEMENT: u32 = 1 << 17;
    pub const STREAMING: u32 = 1 << 18;
    pub const NETDATACONF: u32 = 1 << 19;
    pub const MCP: u32 = 1 << 20;
    pub const SSL_OPTIONAL: u32 = 1 << 28;
    pub const SSL_FORCE: u32 = 1 << 29;
    pub const SSL_DEFAULT: u32 = 1 << 30;
    pub const DASHBOARD: u32 = METRICS | FUNCTIONS | ALERTS | NODES | DYNCFG;
    pub const TRANSPORTS: u32 = API | API_UDP | API_UNIX | ACLK | WEBRTC;
    pub const TRANSPORTS_WITHOUT_CLIENT_IP_VALIDATION: u32 = ACLK | WEBRTC;
    /// What a listener allows without an ACL list.
    pub const ALL_LISTENER_FEATURES: u32 =
        DASHBOARD | REGISTRY | BADGES | MANAGEMENT | NETDATACONF | STREAMING | MCP | SSL_DEFAULT;
}

/// `socket_ssl_acl()` and `read_acl()`: one word of a listener's ACL list; an `^SSL=optional|force` suffix is split
/// off first.
fn read_acl(word: &[u8]) -> u32 {
    let (word, ssl) = match word.iter().position(|&b| b == b'^') {
        Some(at) => (&word[..at], Some(&word[at + 1..])),
        None => (word, None),
    };
    let mut acl = match ssl.and_then(|s| s.strip_prefix(b"SSL=")) {
        Some(b"optional") => bits::SSL_OPTIONAL,
        Some(b"force") => bits::SSL_FORCE,
        _ => 0,
    };
    acl |= match word {
        b"dashboard" => bits::DASHBOARD,
        b"registry" => bits::REGISTRY,
        b"badges" => bits::BADGES,
        b"management" => bits::MANAGEMENT,
        b"streaming" => bits::STREAMING,
        b"netdata.conf" => bits::NETDATACONF,
        b"mcp" => bits::MCP,
        _ => 0,
    };
    acl
}

/// The ACL `bind_to_this()` gives an IP listener: its transport (`tcp:` the API, `udp:` the UDP API, a bare address
/// none); the words after `=` (split on `|`) select features, else every feature; SSL defaults when neither optional
/// nor forced.
pub fn listener_acl(transport: u32, acl_list: Option<&[u8]>) -> u32 {
    let mut acl = transport;
    match acl_list {
        Some(list) => {
            for word in list.split(|&b| b == b'|') {
                acl |= read_acl(word);
            }
        }
        None => acl |= bits::ALL_LISTENER_FEATURES,
    }
    if acl & (bits::SSL_OPTIONAL | bits::SSL_FORCE) == 0 {
        acl |= bits::SSL_DEFAULT;
    }
    acl
}

/// The fixed ACL of a `unix:` listener.
pub const UNIX_LISTENER_ACL: u32 = bits::API_UNIX | bits::ALL_LISTENER_FEATURES;

/// An `allow ... from` pattern and its `allow ... by dns` decision.
pub struct AclPattern {
    pub pattern: SimplePattern,
    pub dns: bool,
}

/// The `[web]` (and `[registry] allow from`) access lists.
pub struct WebAcl {
    pub connections: AclPattern,
    pub dashboard: AclPattern,
    pub mcp: AclPattern,
    pub badges: AclPattern,
    pub registry: AclPattern,
    pub streaming: AclPattern,
    pub netdataconf: AclPattern,
    pub management: AclPattern,
}

/// A client as the ACL checks see it (`w->user_auth.client_ip`, `w->client_host`).
pub struct Client {
    /// As `accept_socket()` formats it: `localhost` for loopback and unix peers, IPv4-mapped addresses unwrapped.
    pub ip: String,
    /// `None` for a unix peer.
    pub peer: Option<IpAddr>,
    /// The reverse-resolved name, filled by the first check that needs it (`UNKNOWN` when it cannot be validated).
    pub host: String,
    /// The errno C's next record carries (a failed peek's); the first record logged takes it.
    pub errno: i32,
}

/// `getnameinfo(NI_NAMEREQD)` of a peer: glibc names a unix peer by the node name.
fn reverse_name(peer: Option<IpAddr>) -> Result<String, String> {
    match peer {
        Some(ip) => dns_lookup::lookup_addr(&ip).map_err(|e| crate::listen::gai_text(&e)),
        None => nix::unistd::gethostname()
            .map(|name| name.to_string_lossy().into_owned())
            .map_err(|e| e.to_string()),
    }
}

/// `connection_allowed()`: the numeric address matches, or (when DNS is allowed) the validated reverse name does.
pub fn connection_allowed(client: &mut Client, acl: &AclPattern, name: &str) -> bool {
    if acl.pattern.matches(client.ip.as_bytes()) {
        return true;
    }
    if client.host.is_empty() && acl.dns {
        match reverse_name(client.peer) {
            Err(err) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = std::mem::take(&mut client.errno);
                    "Incoming {name} on '{}' does not match a numeric pattern, and host could not be resolved (err={err})",
                    client.ip
                );
                client.host = "UNKNOWN".to_string();
                return false;
            }
            Ok(host) => client.host = host,
        }
        match dns_lookup::lookup_host(&client.host) {
            Err(_) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    errno = std::mem::take(&mut client.errno);
                    "LISTENER: cannot validate hostname '{}' from '{}' by resolving it",
                    client.host,
                    client.ip
                );
                client.host = "UNKNOWN".to_string();
                return false;
            }
            Ok(mut addresses) => {
                if !addresses.any(|a| a.to_string() == client.ip) {
                    nd_log!(
                        Source::Daemon,
                        Priority::Err,
                        errno = std::mem::take(&mut client.errno);
                        "LISTENER: Cannot validate '{}' as ip of '{}', not listed in DNS",
                        client.ip,
                        client.host
                    );
                    client.host = "UNKNOWN".to_string();
                }
            }
        }
    }
    acl.pattern.matches(client.host.as_bytes())
}

impl WebAcl {
    /// `web_client_update_acl_matches()`: the transports, the features whose lists the client matches, limited to
    /// what its listener allows.
    pub fn matches(&self, client: &mut Client, listener_acl: u32) -> u32 {
        let mut acl = bits::TRANSPORTS;
        if listener_acl & bits::TRANSPORTS_WITHOUT_CLIENT_IP_VALIDATION == 0 {
            for (pattern, name, feature) in [
                (&self.dashboard, "dashboard", bits::DASHBOARD),
                (&self.registry, "registry", bits::REGISTRY),
                (&self.badges, "badges", bits::BADGES),
                (&self.management, "management", bits::MANAGEMENT),
                (&self.streaming, "streaming", bits::STREAMING),
                (&self.mcp, "mcp", bits::MCP),
                (&self.netdataconf, "netdata.conf", bits::NETDATACONF),
            ] {
                if connection_allowed(client, pattern, name) {
                    acl |= feature;
                }
            }
        }
        acl & listener_acl
    }
}

/// `http_can_access_*()`: every bit of the feature.
pub fn can(acl: u32, feature: u32) -> bool {
    acl & feature == feature
}

/// `http_can_access_dashboard() || registry || badges || mgmt || netdataconf || (mcp route && mcp)`: what OPTIONS
/// and the GET/POST/PUT/DELETE branch require.
pub fn can_access_web(acl: u32, mcp_route: bool) -> bool {
    can(acl, bits::DASHBOARD)
        || can(acl, bits::REGISTRY)
        || can(acl, bits::BADGES)
        || can(acl, bits::MANAGEMENT)
        || can(acl, bits::NETDATACONF)
        || (mcp_route && can(acl, bits::MCP))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listener_acls_match_c() {
        assert_eq!(listener_acl(0, None), bits::ALL_LISTENER_FEATURES);
        assert_eq!(
            listener_acl(bits::API, None),
            bits::API | bits::ALL_LISTENER_FEATURES
        );
        assert_eq!(
            listener_acl(0, Some(b"dashboard|streaming^SSL=force")),
            bits::DASHBOARD | bits::STREAMING | bits::SSL_FORCE
        );
        assert_eq!(
            listener_acl(0, Some(b"badges")),
            bits::BADGES | bits::SSL_DEFAULT
        );
        // an empty list selects nothing but the SSL default
        assert_eq!(listener_acl(0, Some(b"")), bits::SSL_DEFAULT);
        assert_eq!(
            listener_acl(bits::API_UDP, Some(b"mcp^SSL=optional|x")),
            bits::API_UDP | bits::MCP | bits::SSL_OPTIONAL
        );
    }
}
