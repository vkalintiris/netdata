//! The `[web]` listen sockets, ported from `src/libnetdata/socket/listen-sockets.c` (`listen_sockets_setup()`,
//! `bind_to_this()`, `create_listen_socket{4,6,_unix}()`, `listen_sockets_add()`). Brief
//! `knowledge/brief-bind-to.md` in the status repository.

use std::ffi::OsStr;
use std::net::SocketAddr;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;

use netdata_agent_inicfg::{Config, SECTION_WEB};
use netdata_agent_log::{Priority, Source, errno_of, nd_log};
use socket2::{Domain, Protocol, SockAddr, Type};

use crate::acl::{self, bits};

/// An opened listen socket.
pub enum Socket {
    Tcp(std::net::TcpListener),
    /// Opened as C opens it; the web server never reads it (C crashes on the first datagram, D53.1).
    Udp(std::net::UdpSocket),
    Unix(std::os::unix::net::UnixListener),
}

/// One opened listener.
pub struct Listener {
    pub socket: Socket,
    /// The features its definition allows (`fds_acl_flags`).
    pub acl: u32,
    /// `strdup_client_description()`: `tcp:<ip>:<port>` (IPv6 in brackets), `udp:...` or `unix:<path>`.
    pub name: String,
}

/// Default backlog of the web listeners (`LISTEN_SOCKETS.backlog` initial value).
const DEFAULT_BACKLOG: i64 = 4096;
const DEFAULT_PORT: i64 = 19999;
const DEFAULT_BIND_TO: &str = "*";
/// `MAX_LISTEN_FDS`.
const MAX_LISTEN_FDS: usize = 50;
/// `LARGE_SOCK_SIZE` on Linux.
const LARGE_SOCK_SIZE: usize = 32 * 1024 * 1024;
/// `sizeof(sockaddr_un.sun_path) - 1`: what `strncpyz()` keeps of a unix path.
const UNIX_PATH_MAX: usize = 107;
/// glibc's `EAI_SYSTEM`.
const EAI_SYSTEM: i32 = -11;

/// One `bind to` definition.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Definition<'a> {
    /// `unix:<path>`: everything after the prefix, `=` included.
    Unix { path: &'a [u8] },
    Inet {
        dgram: bool,
        /// `None` for the wildcard (empty, `*...`, `any`, `all`).
        host: Option<&'a [u8]>,
        /// `None` for the default port.
        service: Option<&'a [u8]>,
        iface: Option<&'a [u8]>,
        acl: u32,
    },
}

/// The definitions of a `bind to` value: separated by C's `isspace()` and `,`.
fn definitions(value: &[u8]) -> impl Iterator<Item = &[u8]> {
    value
        .split(|&b| matches!(b, b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r' | b','))
        .filter(|d| !d.is_empty())
}

/// `bind_to_this()`'s parse, in place as C does: `[tcp:|udp:]host[%iface][:port][=acl|acl...]` with the host in
/// brackets for IPv6, or `unix:<path>`. After a `]` only `%`, `:` or `=` continue; anything else ends the definition
/// and every feature applies.
fn parse(def: &[u8]) -> Definition<'_> {
    let (b, dgram, transport) = if let Some(rest) = def.strip_prefix(b"tcp:") {
        (rest, false, bits::API)
    } else if let Some(rest) = def.strip_prefix(b"udp:") {
        (rest, true, bits::API_UDP)
    } else if let Some(path) = def.strip_prefix(b"unix:") {
        return Definition::Unix { path };
    } else {
        (def, false, 0)
    };
    let scan = |from: usize, stop: &[u8]| {
        (from..b.len())
            .find(|&i| stop.contains(&b[i]))
            .unwrap_or(b.len())
    };
    let (host, mut at) = if b.first() == Some(&b'[') {
        let end = scan(1, b"]");
        (&b[1..end], (end + 1).min(b.len()))
    } else {
        let end = scan(0, b":%=");
        (&b[..end], end)
    };
    let mut iface = None;
    if b.get(at) == Some(&b'%') {
        let end = scan(at + 1, b":=");
        iface = Some(&b[at + 1..end]);
        at = end;
    }
    let mut service = None;
    if b.get(at) == Some(&b':') {
        let end = scan(at + 1, b"=");
        service = Some(&b[at + 1..end]);
        at = end;
    }
    let list = (b.get(at) == Some(&b'=')).then(|| &b[at + 1..]);
    let wildcard = host.is_empty() || host[0] == b'*' || host == b"any" || host == b"all";
    Definition::Inet {
        dgram,
        host: (!wildcard).then_some(host),
        service: service.filter(|s| !s.is_empty()),
        iface: iface.filter(|i| !i.is_empty()),
        acl: acl::listener_acl(transport, list),
    }
}

/// `strdup_client_description()`: formatted into 100 bytes, so at most 99 are kept.
fn name(protocol: &str, addr: Option<SocketAddr>, path: &[u8]) -> String {
    let mut out = match addr {
        Some(SocketAddr::V4(a)) => format!("{protocol}:{}:{}", a.ip(), a.port()).into_bytes(),
        Some(SocketAddr::V6(a)) => format!("{protocol}:[{}]:{}", a.ip(), a.port()).into_bytes(),
        None => [protocol.as_bytes(), b":", path].concat(),
    };
    out.truncate(99);
    String::from_utf8_lossy(&out).into_owned()
}

/// The unix socket address C binds: the path cut to 107 bytes; an empty path is the abstract socket of 107 NULs
/// (`sizeof(struct sockaddr_un)` as its length).
fn unix_address(path: &[u8]) -> std::io::Result<SockAddr> {
    let bytes = if path.is_empty() {
        vec![0u8; UNIX_PATH_MAX + 1]
    } else {
        path[..path.len().min(UNIX_PATH_MAX)].to_vec()
    };
    SockAddr::unix(OsStr::from_bytes(&bytes))
}

/// `gai_strerror()` of a failed lookup: dns-lookup prefixes it, and for `EAI_SYSTEM` it gives the OS error instead.
pub fn gai_text(err: &dns_lookup::LookupError) -> String {
    if err.error_num() == EAI_SYSTEM {
        return "System error".to_string();
    }
    let text = err.to_string();
    match text.strip_prefix("failed to lookup address information: ") {
        Some(t) => t.to_string(),
        None => text,
    }
}

/// `sock_enlarge_rcv_buf()`: 32 MiB when smaller (the kernel caps it at twice `rmem_max`); errors are ignored.
fn enlarge_rcv_buf(socket: &socket2::Socket) {
    if socket
        .recv_buffer_size()
        .is_ok_and(|size| size < LARGE_SOCK_SIZE)
    {
        let _ = socket.set_recv_buffer_size(LARGE_SOCK_SIZE);
    }
}

/// `listen_sockets_setup()`'s state: the listeners, the `failed` count, and the errno C's next record would carry.
#[derive(Default)]
struct Setup {
    listeners: Vec<Listener>,
    failed: usize,
    /// Left by a call that failed without a record (C's `unlink()` of a unix path that does not exist); C clears
    /// errno after every record, so it reaches only the next one.
    errno: i32,
}

impl Setup {
    /// The errno of the next record: its own, or the one left behind.
    fn errno(&mut self, own: i32) -> i32 {
        let errno = if own != 0 { own } else { self.errno };
        self.errno = 0;
        errno
    }

    fn log(&mut self, priority: Priority, own: i32, message: std::fmt::Arguments<'_>) {
        let errno = self.errno(own);
        nd_log!(Source::Daemon, priority, errno = errno; "LISTENER: {message}");
    }

    /// `create_listen_socket4()` / `create_listen_socket6()`: a failed step that C survives is logged and passed.
    fn create_inet(&mut self, addr: SocketAddr, dgram: bool, backlog: i32) -> Option<Socket> {
        let v6 = addr.is_ipv6();
        let (family, domain) = if v6 {
            ("IPv6", Domain::IPV6)
        } else {
            ("IPv4", Domain::IPV4)
        };
        let (ip, port) = (addr.ip(), addr.port());
        let (socktype, kind, protocol) = if dgram {
            (2, Type::DGRAM, Protocol::UDP)
        } else {
            (1, Type::STREAM, Protocol::TCP)
        };
        let at = format!("on ip '{ip}' port {port}, socktype {socktype}");
        // IPv6's lines have a comma more in two places
        let comma = if v6 { "," } else { "" };
        let socket = match socket2::Socket::new(domain, kind, Some(protocol)) {
            Ok(socket) => socket,
            Err(e) => {
                self.log(
                    Priority::Err,
                    errno_of(&e),
                    format_args!("{family} socket() {at}{comma} failed."),
                );
                return None;
            }
        };
        if let Err(e) = socket.set_reuse_address(true) {
            let verb = if v6 { "set" } else { "enable" };
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("{family} socket {at} failed to {verb} reuse address."),
            );
        }
        // C logs only when the option reads back as set, which a new socket never does
        let _ = socket.set_reuse_port(false);
        if let Err(e) = socket.set_nonblocking(true) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("{family} socket {at}{comma} failed to set non-blocking mode."),
            );
        }
        enlarge_rcv_buf(&socket);
        if v6 && let Err(e) = socket.set_only_v6(true) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("Cannot set IPV6_V6ONLY {at}."),
            );
        }
        if let Err(e) = socket.bind(&addr.into()) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("{family} bind() {at} failed."),
            );
            return None;
        }
        if !dgram {
            if let Err(e) = socket.listen(backlog) {
                self.log(
                    Priority::Err,
                    errno_of(&e),
                    format_args!("{family} listen() {at} failed."),
                );
                return None;
            }
            // TCP_DEFER_ACCEPT of 5 seconds, its failure ignored as in C: a client that connects and sends nothing
            // is never accepted
            use std::os::fd::AsFd;
            let _ = netdata_agent_sys::set_tcp_defer_accept(socket.as_fd(), 5);
        }
        self.log(
            Priority::Debug,
            0,
            format_args!("Listening on {family} ip '{ip}' port {port}, socktype {socktype}"),
        );
        Some(if dgram {
            Socket::Udp(socket.into())
        } else {
            Socket::Tcp(socket.into())
        })
    }

    /// `create_listen_socket_unix()`: whatever sits at the path is unlinked first (D53.6); the socket file is made
    /// 0666 and left behind at exit.
    fn create_unix(&mut self, path: &[u8], backlog: i32) -> Option<Socket> {
        let shown = String::from_utf8_lossy(path);
        let socket = match socket2::Socket::new(Domain::UNIX, Type::STREAM, None) {
            Ok(socket) => socket,
            Err(e) => {
                self.log(
                    Priority::Err,
                    errno_of(&e),
                    format_args!("UNIX socket() on path '{shown}' failed."),
                );
                return None;
            }
        };
        if let Err(e) = socket.set_nonblocking(true) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("UNIX socket on path '{shown}' failed to set non-blocking mode."),
            );
        }
        enlarge_rcv_buf(&socket);
        let fs_path = OsStr::from_bytes(path);
        match std::fs::remove_file(fs_path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => self.errno = errno_of(&e),
            Err(e) => self.log(
                Priority::Err,
                errno_of(&e),
                format_args!(
                    "failed to remove existing (probably obsolete or left-over) file on UNIX socket path '{shown}'."
                ),
            ),
            Ok(()) => {}
        }
        if let Err(e) = unix_address(path).and_then(|addr| socket.bind(&addr)) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("UNIX bind() on path '{shown}' failed."),
            );
            return None;
        }
        if let Err(e) = std::fs::set_permissions(fs_path, std::fs::Permissions::from_mode(0o666)) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("failed to chmod() socket file '{shown}'."),
            );
        }
        if let Err(e) = socket.listen(backlog) {
            self.log(
                Priority::Err,
                errno_of(&e),
                format_args!("UNIX listen() on path '{shown}' failed."),
            );
            return None;
        }
        Some(Socket::Unix(socket.into()))
    }

    /// `listen_sockets_add()`: past the cap the socket is closed with a record, and not counted as failed.
    fn add(
        &mut self,
        socket: Socket,
        protocol: &str,
        addr: Option<SocketAddr>,
        path: &[u8],
        acl: u32,
    ) {
        if self.listeners.len() >= MAX_LISTEN_FDS {
            let (ip, port) = match addr {
                Some(a) => (a.ip().to_string(), a.port()),
                None => (String::from_utf8_lossy(path).into_owned(), 0),
            };
            let socktype = if matches!(socket, Socket::Udp(_)) {
                2
            } else {
                1
            };
            self.log(
                Priority::Err,
                0,
                format_args!(
                    "Too many listening sockets. Failed to add listening {protocol} socket at ip '{ip}' port {port}, \
                     protocol {protocol}, socktype {socktype}"
                ),
            );
            return;
        }
        let name = name(protocol, addr, path);
        self.listeners.push(Listener { socket, acl, name });
    }

    /// `bind_to_this()`.
    fn bind_to_this(&mut self, def: &[u8], default_port: u16, backlog: i32) {
        let (dgram, host, service, iface, acl) = match parse(def) {
            Definition::Unix { path } => {
                match self.create_unix(path, backlog) {
                    Some(socket) => self.add(socket, "unix", None, path, acl::UNIX_LISTENER_ACL),
                    None => {
                        let shown = String::from_utf8_lossy(path).into_owned();
                        self.log(
                            Priority::Err,
                            0,
                            format_args!("Cannot create unix socket '{shown}'"),
                        );
                        self.failed += 1;
                    }
                }
                return;
            }
            Definition::Inet {
                dgram,
                host,
                service,
                iface,
                acl,
            } => (dgram, host, service, iface, acl),
        };
        let mut scope_id = 0;
        if let Some(iface) = iface {
            match nix::net::if_::if_nametoindex(OsStr::from_bytes(iface)) {
                Ok(index) => scope_id = index,
                Err(e) => {
                    let shown = String::from_utf8_lossy(iface).into_owned();
                    self.log(
                        Priority::Err,
                        e as i32,
                        format_args!(
                            "Cannot find a network interface named '{shown}'. Continuing with limiting the network \
                             interface"
                        ),
                    );
                }
            }
        }
        let default = default_port.to_string();
        let service = service.unwrap_or(default.as_bytes());
        // AF_UNSPEC, AI_PASSIVE, and the socket type and protocol of the definition
        let (socktype, protocol) = if dgram { (2, 17) } else { (1, 6) };
        let hints = dns_lookup::AddrInfoHints {
            flags: 1,
            address: 0,
            socktype,
            protocol,
        };
        let lookup = || -> Result<Vec<SocketAddr>, String> {
            let host = host.map(std::str::from_utf8).transpose();
            let host = host.map_err(|_| "Name or service not known".to_string())?;
            let service = std::str::from_utf8(service)
                .map_err(|_| "Servname not supported for ai_socktype")?;
            let results = dns_lookup::getaddrinfo(host, Some(service), Some(hints))
                .map_err(|e| gai_text(&e))?;
            Ok(results.filter_map(Result::ok).map(|a| a.sockaddr).collect())
        };
        let addrs = match lookup() {
            Ok(addrs) => addrs,
            Err(text) => {
                let host = host.map_or("(null)".into(), String::from_utf8_lossy);
                let service = String::from_utf8_lossy(service);
                self.log(
                    Priority::Err,
                    0,
                    format_args!("getaddrinfo('{host}', '{service}'): {text}\n"),
                );
                return;
            }
        };
        let protocol = if dgram { "udp" } else { "tcp" };
        for mut addr in addrs {
            // C binds IPv6 with its own scope id: the interface's, else 0
            if let SocketAddr::V6(a) = &mut addr {
                a.set_scope_id(scope_id);
            }
            match self.create_inet(addr, dgram, backlog) {
                Some(socket) => self.add(socket, protocol, Some(addr), b"", acl),
                None => {
                    self.log(
                        Priority::Err,
                        0,
                        format_args!("Cannot bind to ip '{}', port {}", addr.ip(), addr.port()),
                    );
                    self.failed += 1;
                }
            }
        }
    }
}

/// `listen_sockets_setup()` for the `[web]` section. With a failed definition, every opened socket is listed.
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
    let mut s = Setup::default();
    for def in definitions(&bind_to) {
        s.bind_to_this(def, port as u16, backlog);
    }
    if s.failed > 0 {
        let names: Vec<String> = s.listeners.iter().map(|l| l.name.clone()).collect();
        for name in names {
            s.log(
                Priority::Debug,
                0,
                format_args!("Listen socket {name} opened successfully."),
            );
        }
    }
    s.listeners
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An IP definition; an empty field is an absent one (a parse never yields an empty one).
    fn inet(
        host: &'static str,
        service: &'static str,
        iface: &'static str,
        acl: u32,
    ) -> Definition<'static> {
        let some = |f: &'static str| (!f.is_empty()).then_some(f.as_bytes());
        Definition::Inet {
            dgram: false,
            host: some(host),
            service: some(service),
            iface: some(iface),
            acl,
        }
    }

    const ALL: u32 = bits::ALL_LISTENER_FEATURES;

    #[test]
    fn definitions_parse_as_c() {
        let cases: Vec<(&[u8], Definition)> = vec![
            (
                b"unix:/p=dashboard",
                Definition::Unix {
                    path: b"/p=dashboard",
                },
            ),
            (b"unix:", Definition::Unix { path: b"" }),
            (b"[::1]%lo:45677", inet("::1", "45677", "lo", ALL)),
            (b"[::1]%lo", inet("::1", "", "lo", ALL)),
            (b"127.0.0.1%lo:1", inet("127.0.0.1", "1", "lo", ALL)),
            // the '=' inside brackets is part of the host
            (b"[::1=x]", inet("::1=x", "", "", ALL)),
            // junk after ']' ends the definition: its ACL list is ignored
            (b"[::1]x=badges", inet("::1", "", "", ALL)),
            (b"[::1", inet("::1", "", "", ALL)),
            (b"*:x11", inet("", "x11", "", ALL)),
            (b"*anything", inet("", "", "", ALL)),
            (b"any", inet("", "", "", ALL)),
            (b"all:1", inet("", "1", "", ALL)),
            (b":1", inet("", "1", "", ALL)),
            (b"127.0.0.1:70000", inet("127.0.0.1", "70000", "", ALL)),
            (
                b"tcp:h:1=dashboard|badges^SSL=force",
                inet(
                    "h",
                    "1",
                    "",
                    bits::API | bits::DASHBOARD | bits::BADGES | bits::SSL_FORCE,
                ),
            ),
            (b"h:1=", inet("h", "1", "", bits::SSL_DEFAULT)),
            (
                b"h=badges",
                inet("h", "", "", bits::BADGES | bits::SSL_DEFAULT),
            ),
            (b"h%=mcp", inet("h", "", "", bits::MCP | bits::SSL_DEFAULT)),
        ];
        for (def, want) in cases {
            assert_eq!(parse(def), want, "{}", String::from_utf8_lossy(def));
        }
        assert_eq!(
            parse(b"udp:127.0.0.1:x"),
            Definition::Inet {
                dgram: true,
                host: Some(b"127.0.0.1"),
                service: Some(b"x"),
                iface: None,
                acl: bits::API_UDP | ALL,
            }
        );
    }

    #[test]
    fn values_split_on_c_space_and_commas() {
        let split = |v: &[u8]| definitions(v).map(|d| d.to_vec()).collect::<Vec<_>>();
        assert_eq!(split(b"a\x0bb"), [b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(split(b",a,,b ,"), [b"a".to_vec(), b"b".to_vec()]);
        assert_eq!(split(b"\x0c\r\n\t"), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn names_and_unix_addresses_as_c() {
        let v6: SocketAddr = "[::1]:5".parse().unwrap();
        assert_eq!(name("tcp", Some(v6), b""), "tcp:[::1]:5");
        assert_eq!(
            name("udp", Some("1.2.3.4:6".parse().unwrap()), b""),
            "udp:1.2.3.4:6"
        );
        let long = vec![b'a'; 120];
        assert_eq!(name("unix", None, &long).len(), 99);
        assert_eq!(name("unix", None, b"/run/nd.sock"), "unix:/run/nd.sock");
        let addr = unix_address(&long).unwrap();
        assert_eq!(addr.as_pathname().unwrap().as_os_str().len(), UNIX_PATH_MAX);
        // the empty path binds the abstract socket named by 107 NULs, with C's address length
        let abstract_ = unix_address(b"").unwrap();
        assert_eq!(
            abstract_.len() as usize,
            std::mem::size_of::<u16>() + UNIX_PATH_MAX + 1
        );
        assert_eq!(
            abstract_.as_abstract_namespace(),
            Some(&[0u8; UNIX_PATH_MAX][..])
        );
    }

    #[test]
    fn the_cap_and_the_summary() {
        let mut s = Setup::default();
        for _ in 0..53 {
            s.bind_to_this(b"127.0.0.1:0", 19999, 16);
        }
        assert_eq!((s.listeners.len(), s.failed), (MAX_LISTEN_FDS, 0));
        // an unresolvable definition is not counted as failed, a socket that cannot be created is
        s.bind_to_this(b"udp:127.0.0.1:x11", 19999, 16);
        assert_eq!(s.failed, 0);
        let busy = match &s.listeners[0].socket {
            Socket::Tcp(l) => l.local_addr().unwrap().port(),
            _ => unreachable!(),
        };
        let mut t = Setup::default();
        t.bind_to_this(format!("127.0.0.1:{busy}").as_bytes(), 19999, 16);
        assert_eq!((t.listeners.len(), t.failed), (0, 1));
    }

    #[test]
    fn unix_listeners_replace_what_is_at_their_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nd.sock");
        std::fs::write(&path, b"x").unwrap();
        let mut s = Setup::default();
        let def = [b"unix:", path.as_os_str().as_bytes()].concat();
        s.bind_to_this(&def, 19999, 16);
        assert_eq!(s.failed, 0);
        assert!(matches!(s.listeners[0].socket, Socket::Unix(_)));
        assert_eq!(s.listeners[0].acl, acl::UNIX_LISTENER_ACL);
        use std::os::unix::fs::FileTypeExt;
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.file_type().is_socket());
        assert_eq!(meta.permissions().mode() & 0o777, 0o666);
        // the next record carries the unlink's ENOENT of a path that was not there
        let other = dir.path().join("other.sock");
        let def = [b"unix:", other.as_os_str().as_bytes()].concat();
        s.bind_to_this(&def, 19999, 16);
        assert_eq!(s.errno(0), nix::errno::Errno::ENOENT as i32);
        assert_eq!(s.errno(0), 0);
        let missing = dir.path().join("none").join("nd.sock");
        let def = [b"unix:", missing.as_os_str().as_bytes()].concat();
        s.bind_to_this(&def, 19999, 16);
        assert_eq!((s.listeners.len(), s.failed), (2, 1));
    }

    #[test]
    fn gai_texts_lose_the_crate_prefix() {
        let Err(err) = dns_lookup::getaddrinfo(Some("127.0.0.1"), Some("nosuchsvc"), None) else {
            panic!("nosuchsvc resolved");
        };
        assert_eq!(gai_text(&err), "Servname not supported for ai_socktype");
    }
}
