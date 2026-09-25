//! The `[web]` listen sockets, ported from `src/libnetdata/socket/listen-sockets.c` (`listen_sockets_setup()`,
//! `bind_to_this()`).

use netdata_agent_log::{Priority, Source, nd_log};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use netdata_agent_inicfg::{Config, SECTION_WEB};

/// One opened listener.
pub struct Listener {
    pub socket: std::net::TcpListener,
    /// The features its definition allows (`fds_acl_flags`).
    pub acl: u32,
    /// `strdup_client_description()`: `tcp:<ip>:<port>`, the IPv6 address in brackets.
    pub name: String,
}

/// Default backlog of the web listeners (`LISTEN_SOCKETS.backlog` initial value).
const DEFAULT_BACKLOG: i64 = 4096;
const DEFAULT_PORT: i64 = 19999;
const DEFAULT_BIND_TO: &str = "*";

/// `create_listen_socket4()` / `create_listen_socket6()`: each failed step logs C's line with its errno.
fn create(addr: SocketAddr, backlog: i32) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let (family, domain) = if addr.is_ipv6() {
        ("IPv6", Domain::IPV6)
    } else {
        ("IPv4", Domain::IPV4)
    };
    let (ip, port) = (addr.ip(), addr.port());
    // SOCK_STREAM
    let socktype = 1;
    let failed = |err: std::io::Error, what: String| {
        nd_log!(Source::Daemon, Priority::Err, errno = netdata_agent_log::errno_of(&err); "LISTENER: {what}");
        err
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP)).map_err(|e| {
        let comma = if addr.is_ipv6() { "," } else { "" };
        failed(
            e,
            format!(
                "{family} socket() on ip '{ip}' port {port}, socktype {socktype}{comma} failed."
            ),
        )
    })?;
    socket.set_reuse_address(true).map_err(|e| {
        let verb = if addr.is_ipv6() { "set" } else { "enable" };
        failed(
            e,
            format!("{family} socket on ip '{ip}' port {port}, socktype {socktype} failed to {verb} reuse address."),
        )
    })?;
    if addr.is_ipv6() {
        socket.set_only_v6(true).map_err(|e| {
            failed(
                e,
                format!("Cannot set IPV6_V6ONLY on ip '{ip}' port {port}, socktype {socktype}."),
            )
        })?;
    }
    socket.bind(&addr.into()).map_err(|e| {
        failed(
            e,
            format!("{family} bind() on ip '{ip}' port {port}, socktype {socktype} failed."),
        )
    })?;
    socket.listen(backlog).map_err(|e| {
        failed(
            e,
            format!("{family} listen() on ip '{ip}' port {port}, socktype {socktype} failed."),
        )
    })?;
    socket.set_nonblocking(true).map_err(|e| {
        let comma = if addr.is_ipv6() { "," } else { "" };
        failed(
            e,
            format!("{family} socket on ip '{ip}' port {port}, socktype {socktype}{comma} failed to set non-blocking mode."),
        )
    })?;
    // TCP_DEFER_ACCEPT of 5 seconds, its failure ignored as in C: a client that connects and sends nothing is never
    // accepted
    use std::os::fd::AsFd;
    let _ = netdata_agent_sys::set_tcp_defer_accept(socket.as_fd(), 5);
    nd_log!(
        Source::Daemon,
        Priority::Debug,
        "LISTENER: Listening on {family} ip '{ip}' port {port}, socktype {socktype}"
    );
    Ok(socket.into())
}

/// `bind_to_this()` for TCP definitions: `[tcp:]ip|*|any|all|[ipv6][%iface][:port][=acl|acl...]`. Unix sockets,
/// UDP and interface scopes are not ported yet.
fn bind_to_this(
    definition: &str,
    default_port: u16,
    backlog: i32,
    out: &mut Vec<Listener>,
    failed: &mut usize,
) {
    let mut spec = definition.strip_prefix("tcp:").unwrap_or(definition);
    // The ACL part is everything after '='.
    let mut acl_list = None;
    if let Some(eq) = spec.find('=') {
        acl_list = Some(&spec[eq + 1..]);
        spec = &spec[..eq];
    }
    let acl = crate::acl::listener_acl(definition, acl_list);
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
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "LISTENER: getaddrinfo('{ip}', '{port}'): {err}\n"
                );
                return;
            }
        }
    };
    for addr in addrs {
        let rip = addr.ip().to_string();
        match create(addr, backlog) {
            Ok(socket) => out.push(Listener {
                socket,
                acl,
                name: if addr.is_ipv6() {
                    format!("tcp:[{rip}]:{}", addr.port())
                } else {
                    format!("tcp:{rip}:{}", addr.port())
                },
            }),
            Err(_) => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "LISTENER: Cannot bind to ip '{rip}', port {}",
                    addr.port()
                );
                *failed += 1;
            }
        }
    }
}

/// `listen_sockets_setup()` for the `[web]` section.
pub fn setup(config: &mut Config) -> Vec<Listener> {
    let backlog = config.get_number(SECTION_WEB, "listen backlog", DEFAULT_BACKLOG) as i32;
    let mut port = config.get_number(SECTION_WEB, "default port", DEFAULT_PORT);
    if !(1..=65535).contains(&port) {
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "LISTENER: Invalid listen port {port} given. Defaulting to {DEFAULT_PORT}."
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
        );
    }
    listeners
}
