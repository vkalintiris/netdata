//! The `[web]` listen sockets, ported from `src/libnetdata/socket/listen-sockets.c` (`listen_sockets_setup()`,
//! `bind_to_this()`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use netdata_agent_inicfg::{Config, LogLevel, SECTION_WEB};

/// One opened listener.
pub struct Listener {
    pub socket: std::net::TcpListener,
}

/// Default backlog of the web listeners (`LISTEN_SOCKETS.backlog` initial value).
const DEFAULT_BACKLOG: i64 = 4096;
const DEFAULT_PORT: i64 = 19999;
const DEFAULT_BIND_TO: &str = "*";

fn create(addr: SocketAddr, backlog: i32) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let domain = if addr.is_ipv6() {
        Domain::IPV6
    } else {
        Domain::IPV4
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    if addr.is_ipv6() {
        socket.set_only_v6(true)?;
    }
    socket.bind(&addr.into())?;
    socket.listen(backlog)?;
    socket.set_nonblocking(true)?;
    Ok(socket.into())
}

/// `bind_to_this()` for TCP definitions: `[tcp:]ip|*|any|all|[ipv6][%iface][:port][=acl|acl...]`. Unix sockets,
/// UDP, interface scopes and the per-listener ACL flags come with the ACL work.
fn bind_to_this(
    definition: &str,
    default_port: u16,
    backlog: i32,
    out: &mut Vec<Listener>,
    failed: &mut usize,
    log: &mut impl FnMut(LogLevel, &str),
) {
    let mut spec = definition.strip_prefix("tcp:").unwrap_or(definition);
    // The ACL part is everything after '='.
    if let Some(eq) = spec.find('=') {
        spec = &spec[..eq];
    }
    let (ip, port) = if let Some(rest) = spec.strip_prefix('[') {
        let (ip, after) = rest.split_once(']').unwrap_or((rest, ""));
        (ip, after.strip_prefix(':').unwrap_or(""))
    } else {
        let end = spec.find([':', '%']).unwrap_or(spec.len());
        let after = &spec[end..];
        let port = after.split_once(':').map_or("", |(_, p)| p);
        (&spec[..end], port)
    };
    let port = if port.is_empty() {
        default_port.to_string()
    } else {
        port.to_string()
    };
    let wildcard = ip.is_empty() || ip == "*" || ip == "any" || ip == "all";

    let addrs: Vec<SocketAddr> = if wildcard {
        // getaddrinfo(NULL, port, AI_PASSIVE, AF_UNSPEC) returns the IPv4 wildcard first on glibc.
        match port.parse::<u16>() {
            Ok(p) => vec![
                SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), p),
                SocketAddr::new(IpAddr::V6(Ipv6Addr::UNSPECIFIED), p),
            ],
            Err(_) => Vec::new(),
        }
    } else {
        let resolved = port
            .parse::<u16>()
            .map_err(|e| e.to_string())
            .and_then(|p| {
                (ip, p)
                    .to_socket_addrs()
                    .map(Iterator::collect)
                    .map_err(|e| e.to_string())
            });
        match resolved {
            Ok(addrs) => addrs,
            Err(err) => {
                log(
                    LogLevel::Error,
                    &format!("LISTENER: getaddrinfo('{ip}', '{port}'): {err}\n"),
                );
                return;
            }
        }
    };
    for addr in addrs {
        let rip = addr.ip().to_string();
        match create(addr, backlog) {
            Ok(socket) => out.push(Listener { socket }),
            Err(_) => {
                log(
                    LogLevel::Error,
                    &format!("LISTENER: Cannot bind to ip '{rip}', port {}", addr.port()),
                );
                *failed += 1;
            }
        }
    }
}

/// `listen_sockets_setup()` for the `[web]` section.
pub fn setup(config: &mut Config, log: &mut impl FnMut(LogLevel, &str)) -> Vec<Listener> {
    let backlog = config.get_number(SECTION_WEB, "listen backlog", DEFAULT_BACKLOG) as i32;
    let mut port = config.get_number(SECTION_WEB, "default port", DEFAULT_PORT);
    if !(1..=65535).contains(&port) {
        log(
            LogLevel::Error,
            &format!("LISTENER: Invalid listen port {port} given. Defaulting to {DEFAULT_PORT}."),
        );
        port = config.set_number(SECTION_WEB, "default port", DEFAULT_PORT);
    }
    let bind_to = config
        .get(SECTION_WEB, "bind to", Some(DEFAULT_BIND_TO))
        .unwrap_or_default();
    let bind_to = String::from_utf8_lossy(&bind_to).into_owned();
    let mut listeners = Vec::new();
    let mut failed = 0;
    for definition in bind_to
        .split(|c: char| c.is_ascii_whitespace() || c == ',')
        .filter(|d| !d.is_empty())
    {
        bind_to_this(
            definition,
            port as u16,
            backlog,
            &mut listeners,
            &mut failed,
            log,
        );
    }
    listeners
}
