//! Outgoing connections as C makes them (`src/libnetdata/socket/connect-to.c`, `nd-sock.c` and
//! `wait_on_socket_or_cancel_with_timeout()`): a definition `[tcp:|udp:]host[%iface][:service]` or a Unix socket
//! path, resolved and tried address by address on a blocking socket, over TLS for an `:SSL` parent, and C's timed
//! sends and receives. Maps: `knowledge/map-m7-commit3-connector.md` §5, `map-m7-commit7-tls.md` §2, §4.

use std::cell::Cell;
use std::net::SocketAddr;
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use netdata_agent_evloop::conn::Conn;
use netdata_agent_log::{Field, Priority, Source, Value, push};
use netdata_agent_tls::{Link, OpenError, SslContext, TlsStream};
use nix::errno::Errno as OsErrno;
use nix::poll::PollFlags;
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

/// glibc's `EAI_SYSTEM`.
const EAI_SYSTEM: i32 = -11;
/// `NI_NUMERICHOST | NI_NUMERICSERV`.
const NI_NUMERIC: i32 = 1 | 2;

/// `ND_SOCK_ERROR`, with `ND_SOCK_ERROR_2str()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SockError {
    #[default]
    None,
    ConnectionRefused,
    CannotResolveHostname,
    FailedToCreateSocket,
    NoHostInDefinition,
    PollError,
    Timeout,
    SslCantEstablishSslConnection,
    SslInvalidCertificate,
    SslFailedToOpen,
    ThreadCancelled,
    NoDestinationAvailable,
    UnknownError,
}

impl SockError {
    pub fn text(self) -> &'static str {
        match self {
            SockError::None => "no socket error",
            SockError::ConnectionRefused => "connection refused",
            SockError::CannotResolveHostname => "cannot resolve hostname",
            SockError::FailedToCreateSocket => "cannot create socket",
            SockError::NoHostInDefinition => "no host in definition",
            SockError::PollError => "socket poll() error",
            SockError::Timeout => "timeout",
            SockError::SslCantEstablishSslConnection => "cannot establish SSL connection",
            SockError::SslInvalidCertificate => "invalid SSL certification",
            SockError::SslFailedToOpen => "failed to open SSL",
            SockError::ThreadCancelled => "thread cancelled",
            SockError::NoDestinationAvailable => "no destination available",
            SockError::UnknownError => "unknown error",
        }
    }
}

/// C's `errno` on the calling thread: what a failed call leaves, which the next record C writes carries and then
/// clears (a filtered record neither carries nor clears it).
#[derive(Debug, Default)]
pub struct Errno(Cell<i32>);

impl Errno {
    pub fn set(&self, errno: i32) {
        self.0.set(errno);
    }

    pub fn get(&self) -> i32 {
        self.0.get()
    }

    /// The errno of a record at `priority`, cleared when the record is written.
    pub fn for_record(&self, priority: Priority) -> i32 {
        let errno = self.0.get();
        if !netdata_agent_log::filtered(Source::Daemon, priority) {
            self.0.set(0);
        }
        errno
    }
}

/// A daemon record carrying the thread's C `errno`.
macro_rules! log_errno {
    ($thread:expr, $priority:expr, $($arg:tt)+) => {{
        let priority = $priority;
        netdata_agent_log::nd_log!(
            netdata_agent_log::Source::Daemon,
            priority,
            errno = $thread.errno.for_record(priority);
            $($arg)+
        )
    }};
}
pub(crate) use log_errno;

/// What C reads from the calling thread: its cancellation flag (`nd_thread_signaled_to_cancel()`) and `errno`.
pub struct Thread<'a> {
    pub cancel: &'a AtomicBool,
    pub errno: Errno,
}

impl<'a> Thread<'a> {
    pub fn new(cancel: &'a AtomicBool) -> Self {
        Thread { cancel, errno: Errno::default() }
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
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

/// What `parse_connection_definition()` makes of a definition.
#[derive(Debug, PartialEq, Eq)]
enum Definition<'a> {
    Unix(&'a str),
    Inet { host: &'a str, iface: &'a str, service: Option<&'a str>, dgram: bool },
}

/// `parse_connection_definition()`: `None` when there is no host.
fn parse_definition(definition: &str) -> Option<Definition<'_>> {
    let (mut rest, dgram) = if let Some(r) = definition.strip_prefix("tcp:") {
        (r, false)
    } else if let Some(r) = definition.strip_prefix("udp:") {
        (r, true)
    } else if let Some(path) = definition.strip_prefix("unix:") {
        return Some(Definition::Unix(path));
    } else if definition.starts_with('/') {
        return Some(Definition::Unix(definition));
    } else {
        (definition, false)
    };
    let host;
    if let Some(r) = rest.strip_prefix('[') {
        let end = r.find(']').unwrap_or(r.len());
        host = &r[..end];
        rest = r.get(end + 1..).unwrap_or("");
    } else {
        let end = rest.find([':', '%']).unwrap_or(rest.len());
        host = &rest[..end];
        rest = &rest[end..];
    }
    let mut iface = "";
    if let Some(r) = rest.strip_prefix('%') {
        let end = r.find(':').unwrap_or(r.len());
        iface = &r[..end];
        rest = &r[end..];
    }
    let service = rest.strip_prefix(':');
    if host.is_empty() {
        return None;
    }
    Some(Definition::Inet { host, iface, service, dgram })
}

/// `stream_parent_effective_service()` over `connect_to_definition_get_service()`: the definition's port or service
/// text, else the default port.
pub fn effective_service(definition: &str, default_port: u16) -> String {
    match parse_definition(definition) {
        Some(Definition::Inet { service: Some(s), .. }) if !s.is_empty() => s.to_string(),
        _ => default_port.to_string(),
    }
}

/// `wait_on_socket_or_cancel_with_timeout()` ([`netdata_agent_sys::wait_fd`]) for this thread: its cancellation, and
/// the errno C leaves in its errno.
fn wait_on_socket(fd: std::os::fd::BorrowedFd<'_>, timeout_ms: i64, events: PollFlags, th: &Thread<'_>) -> i32 {
    let (r, errno) = netdata_agent_sys::wait_fd(fd, timeout_ms, events, &|| th.cancelled());
    th.errno.set(errno);
    r
}

/// `connect_to_unix()`.
fn connect_to_unix(path: &str, timeout_s: i64, th: &Thread<'_>) -> Result<Socket, SockError> {
    // socket2 opens every socket with SOCK_CLOEXEC, as DEFAULT_SOCKET_FLAGS
    let socket = match Socket::new(Domain::UNIX, Type::STREAM, None) {
        Ok(socket) => socket,
        Err(e) => {
            th.errno.set(netdata_agent_log::errno_of(&e));
            log_errno!(th, Priority::Err, "Failed to create UNIX socket() for '{path}'");
            return Err(SockError::ConnectionRefused);
        }
    };
    if let Err(e) = socket.set_write_timeout(write_timeout(timeout_s)) {
        th.errno.set(netdata_agent_log::errno_of(&e));
        log_errno!(th, Priority::Err, "Failed to set timeout on UNIX socket '{path}'");
    }
    // strncpyz() into sun_path, one byte short of it
    let mut bytes = path.as_bytes();
    bytes = &bytes[..bytes.len().min(107)];
    let addr = SockAddr::unix(std::ffi::OsStr::new(std::str::from_utf8(bytes).unwrap_or(path)));
    let connected = addr.and_then(|addr| socket.connect(&addr));
    if let Err(e) = connected {
        th.errno.set(netdata_agent_log::errno_of(&e));
        log_errno!(th, Priority::Err, "Cannot connect to UNIX socket on path '{path}'.");
        return Err(SockError::ConnectionRefused);
    }
    log_errno!(th, Priority::Debug, "Connected to UNIX socket on path '{path}'.");
    Ok(socket)
}

/// `SO_SNDTIMEO` of `{timeout_s, 0}`: zero is no timeout.
fn write_timeout(timeout_s: i64) -> Option<Duration> {
    (timeout_s > 0).then(|| Duration::from_secs(timeout_s as u64))
}

/// `timeval_to_poll_timeout_ms()` of `{timeout_s, 0}`.
fn poll_timeout_ms(timeout_s: i64) -> i64 {
    if timeout_s <= 0 {
        0
    } else if timeout_s > i64::from(i32::MAX) / 1000 {
        i64::from(i32::MAX)
    } else {
        timeout_s * 1000
    }
}

/// `connect_to_this_ip46()`: every address the host resolves to, while each attempt is refused.
fn connect_to_ip46(
    host: &str,
    service: &str,
    scope_id: u32,
    dgram: bool,
    timeout_s: i64,
    th: &Thread<'_>,
) -> Result<Socket, SockError> {
    let (socktype, protocol) = if dgram { (2, 17) } else { (1, 6) };
    let hints = dns_lookup::AddrInfoHints { flags: 0, address: 0, socktype, protocol };
    let addrs: Vec<SocketAddr> = match dns_lookup::getaddrinfo(Some(host), Some(service), Some(hints)) {
        Ok(results) => results.filter_map(Result::ok).map(|a| a.sockaddr).collect(),
        Err(e) => {
            // errno is the close's 0: glibc's resolver leaves it alone on this failure (R40 m5, checked against C)
            log_errno!(th, Priority::Err, "Cannot resolve host '{host}', port '{service}': {}", gai_text(&e));
            return Err(SockError::CannotResolveHostname);
        }
    };
    let (kind, proto) = if dgram { (Type::DGRAM, Protocol::UDP) } else { (Type::STREAM, Protocol::TCP) };
    for mut addr in addrs {
        if th.cancelled() {
            break;
        }
        if let SocketAddr::V6(a) = &mut addr
            && a.scope_id() == 0
        {
            a.set_scope_id(scope_id);
        }
        let (ip, port) = dns_lookup::getnameinfo(&addr, NI_NUMERIC).unwrap_or_default();
        let _frame = push(vec![(Field::DstIp, Value::txt(ip.as_str())), (Field::DstPort, Value::txt(port.as_str()))]);
        let socket = match Socket::new(Domain::for_address(addr), kind, Some(proto)) {
            Ok(socket) => socket,
            Err(e) => {
                th.errno.set(netdata_agent_log::errno_of(&e));
                log_errno!(th, Priority::Err, "Failed to socket() to '{ip}', port '{port}'");
                return Err(SockError::FailedToCreateSocket);
            }
        };
        if let Err(e) = socket.set_write_timeout(write_timeout(timeout_s)) {
            th.errno.set(netdata_agent_log::errno_of(&e));
            log_errno!(th, Priority::Err, "Failed to set timeout on the socket to ip '{ip}' port '{port}'");
        }
        th.errno.set(0);
        let Err(e) = socket.connect(&addr.into()) else {
            return Ok(socket);
        };
        let errno = netdata_agent_log::errno_of(&e);
        th.errno.set(errno);
        if errno != OsErrno::EALREADY as i32 && errno != OsErrno::EINPROGRESS as i32 {
            log_errno!(th, Priority::Err, "Failed to connect to '{ip}', port '{port}'");
            // -ND_SOCK_ERR_CONNECTION_REFUSED is -1, which the loop takes for "no socket yet"
            continue;
        }
        log_errno!(th, Priority::Debug, "Waiting for connection to ip {ip} port {port} to be established");
        return match wait_on_socket(socket.as_fd(), poll_timeout_ms(timeout_s), PollFlags::POLLOUT, th) {
            0 => {
                log_errno!(th, Priority::Debug, "connect() to ip {ip} port {port} completed successfully");
                Ok(socket)
            }
            -1 => {
                log_errno!(th, Priority::Err, "Thread is cancelled while connecting to '{ip}', port '{port}'.");
                Err(SockError::ThreadCancelled)
            }
            1 => {
                log_errno!(th, Priority::Err, "Timed out while connecting to '{ip}', port '{port}'.");
                Err(SockError::Timeout)
            }
            _ => {
                log_errno!(th, Priority::Err, "Failed to connect to '{ip}', port '{port}'.");
                Err(SockError::PollError)
            }
        };
    }
    Err(SockError::ConnectionRefused)
}

/// `connect_to_this()`: a blocking socket with `SO_SNDTIMEO`, connected within `timeout_s` (twice that when the
/// send timeout expires first and the poll waits again).
fn connect_to_this(definition: &str, default_port: u16, timeout_s: i64, th: &Thread<'_>) -> Result<Socket, SockError> {
    let (host, iface, service, dgram) = match parse_definition(definition) {
        Some(Definition::Unix(path)) => return connect_to_unix(path, timeout_s, th),
        Some(Definition::Inet { host, iface, service, dgram }) => (host, iface, service, dgram),
        None => {
            log_errno!(th, Priority::Err, "Definition '{definition}' does not specify a host.");
            return Err(SockError::NoHostInDefinition);
        }
    };
    let mut scope_id = 0;
    if !iface.is_empty() {
        match nix::net::if_::if_nametoindex(iface) {
            Ok(index) => scope_id = index,
            Err(e) => {
                th.errno.set(e as i32);
                log_errno!(
                    th,
                    Priority::Err,
                    "Cannot find a network interface named '{iface}'. Continuing without limiting the network interface"
                );
            }
        }
    }
    let default_service = default_port.to_string();
    let service = service.filter(|s| !s.is_empty()).unwrap_or(&default_service);
    connect_to_ip46(host, service, scope_id, dgram, timeout_s, th)
}

/// `nd-sock.c`'s server name for a TLS connection: the definition without `tcp:` or `udp:`, the text inside
/// brackets (an interface kept), else the text before the first `:` or `%`; none when empty. A `unix:` definition
/// gives `unix`, as C's.
fn sni_host(definition: &str) -> Option<&str> {
    let host = definition.strip_prefix("tcp:").or_else(|| definition.strip_prefix("udp:")).unwrap_or(definition);
    let host = match host.strip_prefix('[') {
        Some(inner) => inner.split(']').next().unwrap_or(inner),
        None => host.split([':', '%']).next().unwrap_or(host),
    };
    Some(host).filter(|h| !h.is_empty())
}

/// The connected socket as the event loop's connection, kept blocking until the stream thread takes it.
fn to_conn(socket: Socket) -> std::io::Result<Conn> {
    if socket.local_addr()?.as_socket().is_some() {
        Ok(Conn::Tcp(mio::net::TcpStream::from_std(socket.into())))
    } else {
        let fd = std::os::fd::OwnedFd::from(socket);
        Ok(Conn::Unix(mio::net::UnixStream::from_std(std::os::unix::net::UnixStream::from(fd))))
    }
}

/// `ND_SOCK`: a connected socket, plain or over TLS, or none; the last error; the sender's TLS context and whether
/// it verifies the parent's certificate.
#[derive(Debug, Default)]
pub struct NdSock {
    link: Option<Link<Conn>>,
    pub error: SockError,
    pub ctx: Option<SslContext>,
    pub verify: bool,
}

impl NdSock {
    /// `ND_SOCK_INIT(ctx, verify)`.
    pub fn new(ctx: Option<SslContext>, verify: bool) -> NdSock {
        NdSock { ctx, verify, ..NdSock::default() }
    }

    /// The connection's descriptor, -1 without one.
    pub fn fd(&self) -> i32 {
        self.link.as_ref().and_then(Link::socket).map_or(-1, std::os::fd::AsRawFd::as_raw_fd)
    }

    /// The connection, for the stream thread.
    pub fn take_link(&mut self) -> Option<Link<Conn>> {
        self.link.take()
    }

    /// `nd_sock_close()`: `netdata_ssl_close()` clears `errno`, then a TLS connection's `close_notify` leaves what
    /// the shutdown set.
    pub fn close(&mut self, th: &Thread<'_>) {
        th.errno.set(0);
        if let Some(link) = self.link.take() {
            let tls = link.is_tls();
            drop(link);
            if tls {
                th.errno.set(OsErrno::last_raw());
            }
        }
        self.error = SockError::None;
    }

    /// `nd_sock_connect_to_this()`: the connection, then for an `:SSL` parent its TLS (the server name from the
    /// definition). Without a context an `:SSL` parent fails to open (D107.2: C would send the API key in clear, which
    /// its context's failure crashes before).
    pub fn connect_to_this(
        &mut self,
        definition: &str,
        default_port: u16,
        timeout_s: i64,
        ssl: bool,
        th: &Thread<'_>,
    ) -> bool {
        self.close(th);
        if definition.is_empty() {
            self.error = SockError::NoHostInDefinition;
            return false;
        }
        let socket = match connect_to_this(definition, default_port, timeout_s, th) {
            Ok(socket) => socket,
            Err(e) => {
                self.error = e;
                return false;
            }
        };
        // getsockname() of a connected socket: a failure here is not C's, so it is only an error
        let Ok(conn) = to_conn(socket) else {
            self.error = SockError::UnknownError;
            return false;
        };
        if !ssl {
            self.link = Some(Link::Plain(conn));
            return true;
        }
        let Some(ctx) = &self.ctx else {
            self.error = SockError::SslFailedToOpen;
            return false;
        };
        match TlsStream::connect(ctx, conn, sni_host(definition), self.verify) {
            Ok(tls) => {
                self.link = Some(Link::Tls(Box::new(tls)));
                true
            }
            Err(e) => {
                self.error = match e {
                    OpenError::FailedToOpen => SockError::SslFailedToOpen,
                    OpenError::CantEstablish => SockError::SslCantEstablishSslConnection,
                    OpenError::InvalidCertificate => SockError::SslInvalidCertificate,
                };
                // what the failed connection's close left
                th.errno.set(OsErrno::last_raw());
                false
            }
        }
    }

    /// Waits as `nd_sock_*_timeout()` do: the error of a timeout (0) or of a failed wait (-1). A read finds bytes a
    /// TLS connection already holds at once, after the cancellation's check.
    fn wait(&mut self, events: PollFlags, timeout_s: i64, th: &Thread<'_>) -> Result<(), isize> {
        let Some(link) = &self.link else {
            self.error = SockError::PollError;
            return Err(-1);
        };
        let Some(socket) = link.socket() else {
            self.error = SockError::PollError;
            return Err(-1);
        };
        if events == PollFlags::POLLIN && link.has_pending() && !th.cancelled() {
            return Ok(());
        }
        match wait_on_socket(socket.as_fd(), timeout_s.saturating_mul(1000), events, th) {
            0 => Ok(()),
            1 => {
                self.error = SockError::Timeout;
                Err(0)
            }
            -1 => {
                self.error = SockError::ThreadCancelled;
                Err(-1)
            }
            _ => {
                self.error = SockError::PollError;
                Err(-1)
            }
        }
    }

    /// `nd_sock_send_timeout()`: waits up to `timeout_s` (0 waits forever), then one `send()`, whose result it
    /// returns (a partial send included); 0 on a timeout.
    pub fn send_timeout(&mut self, buf: &[u8], timeout_s: i64, th: &Thread<'_>) -> isize {
        if let Err(r) = self.wait(PollFlags::POLLOUT, timeout_s, th) {
            return r;
        }
        match self.link.as_mut().map(|l| l.write(buf)) {
            Some(Ok(n)) => n as isize,
            Some(Err(e)) => {
                th.errno.set(netdata_agent_log::errno_of(&e));
                -1
            }
            None => -1,
        }
    }

    /// `nd_sock_recv_timeout()`: as [`NdSock::send_timeout`], with one `recv()` (0 when the peer closed).
    pub fn recv_timeout(&mut self, buf: &mut [u8], timeout_s: i64, th: &Thread<'_>) -> isize {
        if let Err(r) = self.wait(PollFlags::POLLIN, timeout_s, th) {
            return r;
        }
        match self.link.as_mut().map(|l| l.read(buf)) {
            Some(Ok(n)) => n as isize,
            Some(Err(e)) => {
                th.errno.set(netdata_agent_log::errno_of(&e));
                -1
            }
            None => -1,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    #[test]
    fn definitions_parse_as_c() {
        let inet = |host, iface, service, dgram| Some(Definition::Inet { host, iface, service, dgram });
        assert_eq!(parse_definition("parent"), inet("parent", "", None, false));
        assert_eq!(parse_definition("tcp:10.0.0.1:19999"), inet("10.0.0.1", "", Some("19999"), false));
        assert_eq!(parse_definition("udp:h:x"), inet("h", "", Some("x"), true));
        assert_eq!(parse_definition("[fe80::1%eth0]:80"), inet("fe80::1%eth0", "", Some("80"), false));
        assert_eq!(parse_definition("[::1]"), inet("::1", "", None, false));
        assert_eq!(parse_definition("h%eth0:80"), inet("h", "eth0", Some("80"), false));
        assert_eq!(parse_definition("h:"), inet("h", "", Some(""), false));
        assert_eq!(parse_definition("unix:/run/x"), Some(Definition::Unix("/run/x")));
        assert_eq!(parse_definition("/run/x"), Some(Definition::Unix("/run/x")));
        // an IPv6 address without brackets has no host before its first colon
        assert_eq!(parse_definition("::1"), None);
        assert_eq!(parse_definition("tcp:"), None);
        assert_eq!(parse_definition("[]:80"), None);
    }

    /// `connect_to_this()` past the parse: `udp:` reaches socket() as SOCK_DGRAM/IPPROTO_UDP (a UDP connect needs no
    /// listener, so the peer is the address tried), a missing port is the default one, a service name goes to
    /// getaddrinfo() as C's (compared with the resolver: /etc/services may lack it), and `%iface` after `]` becomes
    /// the link-local address's scope id through if_nametoindex() (a link-local connect fails EINVAL without one).
    #[test]
    fn definitions_reach_the_socket_as_c() {
        let cancel = AtomicBool::new(false);
        let th = Thread::new(&cancel);
        let peer = |s: &Socket| s.peer_addr().unwrap().as_socket().unwrap();
        let s = connect_to_this("udp:127.0.0.1:9", 19999, 1, &th).unwrap();
        assert_eq!(s.r#type().unwrap(), Type::DGRAM);
        assert_eq!(peer(&s), "127.0.0.1:9".parse().unwrap());
        assert_eq!(peer(&connect_to_this("udp:127.0.0.1", 19999, 1, &th).unwrap()), "127.0.0.1:19999".parse().unwrap());
        let hints = dns_lookup::AddrInfoHints { flags: 0, address: 0, socktype: 2, protocol: 17 };
        let tftp = dns_lookup::getaddrinfo(Some("127.0.0.1"), Some("tftp"), Some(hints))
            .ok()
            .and_then(|mut r| r.next())
            .and_then(Result::ok)
            .map(|a| a.sockaddr);
        let (got, records) = netdata_agent_log::capture(|| connect_to_this("udp:127.0.0.1:tftp", 19999, 1, &th));
        match tftp {
            Some(addr) => assert_eq!(peer(&got.unwrap()), addr),
            None => {
                assert_eq!(got.err(), Some(SockError::CannotResolveHostname));
                assert_eq!(records.len(), 1);
            }
        }
        // the scope shows in getnameinfo()'s text; lo routes no fe80::/64, so the connect itself fails
        let texts = |records: Vec<netdata_agent_log::Captured>| -> Vec<_> {
            records.into_iter().map(|r| (r.errno, r.message.unwrap())).collect()
        };
        let (got, records) = netdata_agent_log::capture(|| connect_to_this("udp:[fe80::1]%lo:9", 19999, 1, &th));
        assert_eq!(got.err(), Some(SockError::ConnectionRefused));
        let texts_lo = texts(records);
        assert_eq!(texts_lo.len(), 1);
        assert_eq!(texts_lo[0].1, "Failed to connect to 'fe80::1%lo', port '9'");
        assert_ne!(texts_lo[0].0, 22, "EINVAL: no scope id");
        let (got, records) =
            netdata_agent_log::capture(|| connect_to_this("udp:[fe80::1]%nosuchif0:9", 19999, 1, &th));
        assert_eq!(got.err(), Some(SockError::ConnectionRefused));
        assert_eq!(
            texts(records),
            [
                (19, "Cannot find a network interface named 'nosuchif0'. Continuing without limiting the network interface".to_string()),
                (22, "Failed to connect to 'fe80::1', port '9'".to_string()),
            ]
        );
    }

    #[test]
    fn effective_services_as_c() {
        assert_eq!(effective_service("parent", 19999), "19999");
        assert_eq!(effective_service("parent:20000", 19999), "20000");
        assert_eq!(effective_service("parent:", 19999), "19999");
        assert_eq!(effective_service("[::1]:http", 19999), "http");
        assert_eq!(effective_service("unix:/x", 19999), "19999");
        assert_eq!(effective_service(":80", 19999), "19999");
    }

    #[test]
    fn a_refused_address_logs_and_fails_as_refused() {
        // a port nothing listens on: bind one and drop it
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let th = Thread::new(&cancel);
        let mut sock = NdSock::default();
        let (ok, records) =
            netdata_agent_log::capture(|| sock.connect_to_this(&format!("127.0.0.1:{port}"), 19999, 5, false, &th));
        assert!(!ok);
        assert_eq!(sock.error, SockError::ConnectionRefused);
        let texts: Vec<_> = records.iter().map(|r| (r.priority, r.errno, r.message.clone().unwrap())).collect();
        assert_eq!(
            texts,
            [(Priority::Err, 111, format!("Failed to connect to '127.0.0.1', port '{port}'"))]
        );
    }

    #[test]
    fn sends_and_receives_wait_and_time_out_as_c() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let th = Thread::new(&cancel);
        let mut sock = NdSock::default();
        assert!(sock.connect_to_this(&format!("127.0.0.1:{port}"), 19999, 5, false, &th));
        let (mut peer, _) = listener.accept().unwrap();
        assert_eq!(sock.send_timeout(b"ping", 1, &th), 4);
        let mut buf = [0u8; 4];
        peer.read_exact(&mut buf).unwrap();
        // nothing to read: a timeout, 0 with ETIMEDOUT
        assert_eq!(sock.recv_timeout(&mut buf, 1, &th), 0);
        assert_eq!((sock.error, th.errno.get()), (SockError::Timeout, 110));
        peer.write_all(b"pong").unwrap();
        assert_eq!(sock.recv_timeout(&mut buf, 1, &th), 4);
        drop(peer);
        assert_eq!(sock.recv_timeout(&mut buf, 1, &th), 0);
        // a cancelled thread stops waiting
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(sock.recv_timeout(&mut buf, 0, &th), -1);
        assert_eq!(sock.error, SockError::ThreadCancelled);
    }

    /// An `:SSL` parent without a TLS context fails to open after the connection (D107.2).
    #[test]
    fn an_ssl_parent_without_a_context_fails_to_open() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let cancel = AtomicBool::new(false);
        let th = Thread::new(&cancel);
        let mut sock = NdSock::default();
        assert!(!sock.connect_to_this(&format!("127.0.0.1:{port}"), 19999, 5, true, &th));
        assert_eq!((sock.error, sock.fd()), (SockError::SslFailedToOpen, -1));
    }

    /// `nd-sock.c`'s server names.
    #[test]
    fn server_names_as_c() {
        let cases = [
            ("h:1", Some("h")),
            ("tcp:h:1", Some("h")),
            ("udp:h", Some("h")),
            ("[::1]:1", Some("::1")),
            ("[fe80::1%eth0]:1", Some("fe80::1%eth0")),
            ("h%eth0:1", Some("h")),
            ("unix:/p", Some("unix")),
            ("/p", Some("/p")),
            ("[]:1", None),
            ("tcp:", None),
            ("[::1", Some("::1")),
        ];
        for (definition, want) in cases {
            assert_eq!(sni_host(definition), want, "{definition}");
        }
    }

    #[test]
    fn gai_texts_lose_the_crate_prefix() {
        let Err(err) = dns_lookup::getaddrinfo(Some("127.0.0.1"), Some("nosuchsvc"), None) else {
            panic!("nosuchsvc resolved");
        };
        assert_eq!(gai_text(&err), "Servname not supported for ai_socktype");
    }
}
