//! TLS of the agent over the system's OpenSSL (D10), ported from `src/libnetdata/socket/security.c`: the library's
//! initialization, the web server's context as `netdata_ssl_create_server_ctx()` builds it and the stream sender's as
//! `netdata_ssl_create_client_ctx()` does, a connection's TLS as `NETDATA_SSL` runs it (the server's non-blocking
//! handshake, the client's blocking one with its certificate test, reads, writes and the close), and C's records of
//! OpenSSL's errors. Decisions D96, D97, D99, D107 and D113 in the status repository.

#![forbid(unsafe_code)]

use std::ffi::c_int;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, RawFd};

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log, nd_log_limit, netdata_log_error};
use netdata_agent_sys::SocketSsl;
use nix::errno::Errno;
use openssl::error::{Error, ErrorStack};
use openssl::ssl::{
    ErrorCode, Ssl, SslContextBuilder, SslFiletype, SslMethod, SslMode, SslRef, SslVerifyMode, SslVersion,
};

pub use openssl::ssl::SslContext;

/// `netdata_conf_ssl()`: OpenSSL initialized, its configuration file loaded (`OPENSSL_INIT_LOAD_CONFIG`, a default
/// since 1.1.1).
pub fn init() {
    openssl::init();
}

/// The web server's `[web]` TLS keys.
#[derive(Debug, Clone, Copy)]
pub struct ServerConfig<'a> {
    /// `ssl key`.
    pub key: &'a str,
    /// `ssl certificate`.
    pub certificate: &'a str,
    /// `tls version`.
    pub tls_version: &'a str,
    /// `tls ciphers`: `none` keeps OpenSSL's.
    pub ciphers: &'a str,
    /// `ssl skip certificate verification`.
    pub skip_verification: bool,
}

/// `netdata_ssl_initialize_ctx(NETDATA_SSL_WEB_SERVER_CTX)`: the context, or none (the server then speaks plain
/// HTTP only), with C's records.
pub fn web_server_context(config: &ServerConfig<'_>) -> Option<SslContext> {
    // stat(key) || stat(certificate): the record carries the errno of the first that fails
    let missing = [config.key, config.certificate].into_iter().find_map(|path| std::fs::metadata(path).err());
    if let Some(e) = missing {
        nd_log!(Source::Daemon, Priority::Info, errno = e.raw_os_error().unwrap_or(0);
            "To use encryption it is necessary to set \"ssl certificate\" and \"ssl key\" in [web] !\n");
        return None;
    }
    match server_context(config) {
        Ok(context) => Some(context),
        Err(record) => {
            netdata_log_error!("{record}");
            None
        }
    }
}

/// `netdata_ssl_create_server_ctx()` in C's order: the chain, the protocol versions, the cipher list (a failure is
/// only reported), the key, then the check that fails the context. The error record of a failure, else the context.
fn server_context(config: &ServerConfig<'_>) -> Result<SslContext, String> {
    let mut b = SslContextBuilder::new(SslMethod::tls_server())
        .map_err(|_| "Cannot create a new SSL context, netdata won't encrypt communication".to_string())?;
    // OpenSSL's error queue: the failures C ignores stay queued, and the key check's record prints the oldest
    let mut queue: Vec<Error> = Vec::new();
    let mut keep = |r: Result<(), ErrorStack>| {
        if let Err(stack) = r {
            queue.extend(stack.errors().iter().cloned());
        }
    };
    keep(b.set_certificate_chain_file(config.certificate));
    keep(b.set_min_proto_version(Some(SslVersion::TLS1)));
    keep(b.set_max_proto_version(Some(tls_version(config.tls_version))));
    if config.ciphers != "none" {
        let r = b.set_cipher_list(config.ciphers);
        if r.is_err() {
            netdata_log_error!("SSL error. cannot set the cipher list");
        }
        keep(r);
    }
    keep(b.set_private_key_file(config.key, SslFiletype::PEM));
    if let Err(stack) = b.check_private_key() {
        queue.extend(stack.errors().iter().cloned());
        return Err(format!(
            "SSL cannot check the private key: {}",
            queue.first().map(error_string).unwrap_or_default()
        ));
    }
    // netdata_id_context: the bytes of an `int` 1
    keep(b.set_session_id_context(&1i32.to_ne_bytes()));
    // the info callback feeds only debug records, compiled out of production builds
    b.set_mode(SslMode::ENABLE_PARTIAL_WRITE | SslMode::ACCEPT_MOVING_WRITE_BUFFER);
    if config.skip_verification {
        b.set_verify(SslVerifyMode::NONE);
    }
    Ok(b.build())
}

/// `alpn_proto_list` of `nd-sock.c`: what a stream sender offers.
pub const STREAM_ALPN: &[u8] = b"\x12netdata_stream/2.0\x08http/1.1";

/// The stream sender's `[stream]` TLS keys.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClientConfig<'a> {
    /// `ssl skip certificate verification`.
    pub skip_verification: bool,
    /// `CAfile` and `CApath`, empty when unset.
    pub ca_file: &'a str,
    pub ca_path: &'a str,
}

/// `netdata_ssl_initialize_ctx(NETDATA_SSL_STREAMING_SENDER_CTX)` then `ssl_security_location_for_context()`: TLS 1.0
/// to 1.3, partial and moving writes, no verification when skipping it, the configured CA locations, then OpenSSL's
/// own, each failure only reported; none when OpenSSL cannot make a context.
pub fn streaming_sender_context(config: &ClientConfig<'_>) -> Option<SslContext> {
    let mut b = SslContextBuilder::new(SslMethod::tls_client()).ok()?;
    let _ = b.set_min_proto_version(Some(SslVersion::TLS1));
    let _ = b.set_max_proto_version(Some(SslVersion::TLS1_3));
    b.set_mode(SslMode::ENABLE_PARTIAL_WRITE | SslMode::ACCEPT_MOVING_WRITE_BUFFER);
    if config.skip_verification {
        b.set_verify(SslVerifyMode::NONE);
    }
    let (file, path) = (Some(config.ca_file).filter(|f| !f.is_empty()), Some(config.ca_path).filter(|p| !p.is_empty()));
    if (file.is_some() || path.is_some())
        && b.load_verify_locations(file.map(std::path::Path::new), path.map(std::path::Path::new)).is_err()
    {
        // the errno OpenSSL's file access left
        nd_log!(Source::Daemon, Priority::Info, errno = Errno::last_raw();
            "Netdata can not verify custom CAfile or CApath for parent's SSL certificate, so it will use the default \
             OpenSSL configuration to validate certificates!");
    }
    if b.set_default_verify_paths().is_err() {
        nd_log!(Source::Daemon, Priority::Info, errno = Errno::last_raw();
            "Can not verify default OpenSSL configuration to validate certificates!");
    }
    Some(b.build())
}

/// Why a client's TLS open failed (`ND_SOCK_ERR_SSL_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    FailedToOpen,
    CantEstablish,
    InvalidCertificate,
}

/// `WANT_READ_WRITE_TIMEOUT_MS`: how long the client's handshake waits on its socket for each step.
const WANT_READ_WRITE_TIMEOUT_MS: u8 = 10;

/// `netdata_ssl_select_tls_version()`: the highest version the server offers; another text is the library's
/// highest (`TLS_MAX_VERSION`, 1.3).
fn tls_version(text: &str) -> SslVersion {
    match text {
        "1" | "1.0" => SslVersion::TLS1,
        "1.1" => SslVersion::TLS1_1,
        "1.2" => SslVersion::TLS1_2,
        _ => SslVersion::TLS1_3,
    }
}

/// `ERR_error_string_n()` of OpenSSL 3: `error:<code>:<library>::<reason>`, the library and the reason by number when
/// OpenSSL has no text for them.
pub fn error_string(e: &Error) -> String {
    let code = e.code();
    let library = e.library().map_or_else(|| format!("lib({})", e.library_code()), str::to_string);
    let reason = e.reason().map_or_else(|| format!("reason({})", e.reason_code()), str::to_string);
    format!("error:{code:08X}:{library}::{reason}")
}

/// A connection's TLS state (`NETDATA_SSL_STATE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The handshake is under way.
    Init,
    Complete,
    Failed,
}

/// A connection over TLS (`NETDATA_SSL`), OpenSSL reading and writing the socket itself (D97.2). Dropping it sends
/// the peer a `close_notify` (`netdata_ssl_close()`), as every C path that frees a connection does.
#[derive(Debug)]
pub struct TlsStream<S: AsFd> {
    ssl: SocketSsl<S>,
    state: State,
    /// What the last operation waits for (`ssl_errno`): `WANT_READ` or `WANT_WRITE`.
    want: Option<ErrorCode>,
}

/// What a step of the handshake did.
#[derive(Debug)]
pub enum Handshake {
    Complete,
    /// Waiting for the socket to be readable (`false`) or writable (`true`).
    Pending { write: bool },
    Failed,
}

impl<S: AsFd> TlsStream<S> {
    /// `netdata_ssl_open()` on the web server's context: the handshake has not started. `None` (with C's record) when
    /// OpenSSL cannot make the connection; the socket is then gone with it.
    pub fn new(context: &SslContext, stream: S) -> Option<TlsStream<S>> {
        Errno::clear();
        let ssl = match Ssl::new(context) {
            Ok(ssl) => ssl,
            Err(stack) => {
                log_error_queue("SSL_new", None, None, 0, stack.errors());
                return None;
            }
        };
        match SocketSsl::new(ssl, stream) {
            Ok(ssl) => {
                // ERR_clear_error()
                let _ = ErrorStack::get();
                Some(TlsStream { ssl, state: State::Init, want: None })
            }
            Err((ssl, _)) => {
                // no BIO, so SSL_get_rfd() finds no descriptor
                log_error_queue("SSL_set_fd", Some(&ssl), None, 0, ErrorStack::get().errors());
                None
            }
        }
    }

    /// `nd_sock_open_ssl()` on a blocking socket: the connection with the stream's ALPN list (its result ignored),
    /// the descriptor, the server name when there is one (a failure only reported), the handshake retried while the
    /// socket turns ready within 10 ms, then, when `verify`, the peer's certificate tested. A failure drops the
    /// connection as C's `netdata_ssl_close()` does, then the socket.
    pub fn connect(context: &SslContext, stream: S, sni: Option<&str>, verify: bool) -> Result<TlsStream<S>, OpenError> {
        Errno::clear();
        let mut ssl = match Ssl::new(context) {
            Ok(ssl) => ssl,
            Err(stack) => {
                log_error_queue("SSL_new", None, None, 0, stack.errors());
                return Err(OpenError::FailedToOpen);
            }
        };
        let _ = ssl.set_alpn_protos(STREAM_ALPN);
        let socket = match SocketSsl::new(ssl, stream) {
            Ok(socket) => socket,
            Err((ssl, _)) => {
                log_error_queue("SSL_set_fd", Some(&ssl), None, 0, ErrorStack::get().errors());
                return Err(OpenError::FailedToOpen);
            }
        };
        // ERR_clear_error()
        let _ = ErrorStack::get();
        let mut tls = TlsStream { ssl: socket, state: State::Init, want: None };
        if let Some(name) = sni.filter(|n| !n.is_empty())
            && tls.ssl.set_hostname(name).is_err()
        {
            nd_log!(Source::Daemon, Priority::Warning, "Failed to set SNI hostname '{name}' for SSL connection");
        }
        if !tls.handshake() {
            return Err(OpenError::CantEstablish);
        }
        if verify && !tls.certificate_ok() {
            return Err(OpenError::InvalidCertificate);
        }
        Ok(tls)
    }

    /// `netdata_ssl_connect()`: `SSL_connect()` again while it only waits for the socket and the socket turns ready
    /// within 10 ms; the error's record otherwise.
    fn handshake(&mut self) -> bool {
        Errno::clear();
        self.ssl.set_connect_state();
        let mut ret = self.ssl.connect();
        while ret != 1 && self.retry(ret) {
            ret = self.ssl.connect();
        }
        if ret == 1 {
            self.state = State::Complete;
            return true;
        }
        self.log(ErrorCode::from_raw(self.ssl.error(ret)), "SSL_connect");
        self.state = State::Failed;
        false
    }

    /// `want_read_write_should_retry()`.
    fn retry(&self, ret: c_int) -> bool {
        use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
        let flags = match ErrorCode::from_raw(self.ssl.error(ret)) {
            ErrorCode::WANT_READ => PollFlags::POLLIN,
            ErrorCode::WANT_WRITE => PollFlags::POLLOUT,
            _ => return false,
        };
        let mut fds = [PollFd::new(self.get_ref().as_fd(), flags)];
        poll(&mut fds, PollTimeout::from(WANT_READ_WRITE_TIMEOUT_MS)).is_ok_and(|n| n > 0)
    }

    /// `security_test_certificate()`: the peer sent a certificate that verified; the record of one that did not (a
    /// missing one has none).
    fn certificate_ok(&self) -> bool {
        let ssl = self.ssl.ssl();
        if ssl.peer_certificate().is_none() {
            return false;
        }
        let status = ssl.verify_result();
        if status == openssl::x509::X509VerifyResult::OK {
            return true;
        }
        // ERR_error_string_n(ERR_get_error()): the oldest queued error, code 0 when none
        let message = ErrorStack::get()
            .errors()
            .first()
            .map_or_else(|| "error:00000000:lib(0)::reason(0)".to_string(), error_string);
        netdata_log_error!(
            "SSL RFC4158 check:  We have a invalid certificate, the tests result with {} and message {message}",
            status.as_raw()
        );
        false
    }

    /// `netdata_ssl_has_pending()`: decrypted bytes OpenSSL holds for the next read.
    pub fn has_pending(&self) -> bool {
        self.ssl.ssl().pending() > 0
    }

    pub fn get_ref(&self) -> &S {
        self.ssl.get_ref()
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Whether the last operation waits for the socket to be writable (`SSL_ERROR_WANT_WRITE`).
    pub fn wants_write(&self) -> bool {
        self.want == Some(ErrorCode::WANT_WRITE)
    }

    /// `netdata_ssl_accept_nonblocking()`.
    pub fn accept(&mut self) -> Handshake {
        Errno::clear();
        self.want = None;
        if self.state != State::Init {
            return Handshake::Failed;
        }
        // SSL_get_error() reads the thread's error queue, so it is emptied first
        let _ = ErrorStack::get();
        let ret = self.ssl.accept();
        if ret == 1 {
            self.state = State::Complete;
            return Handshake::Complete;
        }
        let code = ErrorCode::from_raw(self.ssl.error(ret));
        if matches!(code, ErrorCode::WANT_READ | ErrorCode::WANT_WRITE) {
            self.want = Some(code);
            return Handshake::Pending { write: code == ErrorCode::WANT_WRITE };
        }
        self.log(code, "SSL_accept");
        self.state = State::Failed;
        Handshake::Failed
    }

    /// `netdata_ssl_read()`: `Ok(0)` at the peer's `close_notify`, `WouldBlock` while a record is incomplete, else an
    /// error after C's record.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io("read", |s| s.read(buf))
    }

    /// `netdata_ssl_peek()`: [`TlsStream::read`] that leaves the data for the next read.
    pub fn peek(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.io("peek", |s| s.peek(buf))
    }

    /// `netdata_ssl_write()`.
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.io("write", |s| s.write(buf))
    }

    fn io(&mut self, op: &str, f: impl FnOnce(&mut SocketSsl<S>) -> c_int) -> io::Result<usize> {
        Errno::clear();
        self.want = None;
        if self.state != State::Complete {
            incomplete(op, self.state, self.get_ref().as_fd().as_raw_fd());
            return Err(io::Error::from_raw_os_error(Errno::ENOTCONN as i32));
        }
        let ret = f(&mut self.ssl);
        if ret > 0 {
            return Ok(ret as usize);
        }
        let code = ErrorCode::from_raw(self.ssl.error(ret));
        match code {
            // the peer's close_notify ends a read; C's write has no such case and reports it below
            ErrorCode::ZERO_RETURN if op != "write" => Ok(0),
            // C's errno: EWOULDBLOCK
            ErrorCode::WANT_READ | ErrorCode::WANT_WRITE => {
                self.want = Some(code);
                Err(io::Error::from_raw_os_error(Errno::EAGAIN as i32))
            }
            _ => {
                // the socket's errno (SSL_ERROR_SYSCALL), before the record's own calls change it
                let errno = Errno::last_raw();
                let call = match op {
                    "read" => "SSL_read",
                    "peek" => "SSL_peek",
                    _ => "SSL_write",
                };
                self.log(code, call);
                if matches!(code, ErrorCode::SSL | ErrorCode::SYSCALL | ErrorCode::ZERO_RETURN) {
                    self.state = State::Failed;
                }
                Err(if errno != 0 { io::Error::from_raw_os_error(errno) } else { io::Error::other(call) })
            }
        }
    }

    /// `netdata_ssl_close()`: a `close_notify` for the peer, twice when the first only sent it.
    fn shutdown(&mut self) {
        Errno::clear();
        if self.ssl.shutdown() == 0 {
            self.ssl.shutdown();
        }
        // ERR_clear_error()
        let _ = ErrorStack::get();
    }

    fn log(&self, code: ErrorCode, call: &str) {
        let fd = self.get_ref().as_fd().as_raw_fd();
        log_error_queue(call, Some(self.ssl.ssl()), Some(fd), code.as_raw(), ErrorStack::get().errors());
    }
}

impl<S: AsFd> Drop for TlsStream<S> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A connection's socket, plain or over TLS (`NETDATA_SSL` beside the socket): what the web server serves and the
/// stream receiver takes over.
#[derive(Debug)]
pub enum Link<S: Read + Write + AsFd> {
    Plain(S),
    Tls(Box<TlsStream<S>>),
    /// The socket went with a TLS connection OpenSSL could not make; the connection is closed.
    Gone,
}

impl<S: Read + Write + AsFd> Link<S> {
    pub fn socket(&self) -> Option<&S> {
        match self {
            Link::Plain(s) => Some(s),
            Link::Tls(t) => Some(t.get_ref()),
            Link::Gone => None,
        }
    }

    /// `nd_sock_is_ssl()`: the connection has TLS (its handshake started, even when it failed).
    pub fn is_tls(&self) -> bool {
        matches!(self, Link::Tls(_))
    }

    /// Whether the last TLS operation waits for the socket to be writable.
    pub fn wants_write(&self) -> bool {
        matches!(self, Link::Tls(t) if t.wants_write())
    }

    /// `netdata_ssl_has_pending()`: a TLS link holds decrypted bytes for the next read.
    pub fn has_pending(&self) -> bool {
        matches!(self, Link::Tls(t) if t.has_pending())
    }

    /// `recv()` or `netdata_ssl_read()`.
    pub fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Link::Plain(s) => s.read(buf),
            Link::Tls(t) => t.read(buf),
            Link::Gone => Err(io::ErrorKind::NotConnected.into()),
        }
    }

    /// `send()` or `netdata_ssl_write()`.
    pub fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Link::Plain(s) => s.write(buf),
            Link::Tls(t) => t.write(buf),
            Link::Gone => Err(io::ErrorKind::NotConnected.into()),
        }
    }
}

/// A TLS connection's socket registers with a poller through the connection (D99.1).
impl<S: AsFd + mio::event::Source> mio::event::Source for TlsStream<S> {
    fn register(&mut self, registry: &mio::Registry, token: mio::Token, interests: mio::Interest) -> io::Result<()> {
        self.ssl.register(registry, token, interests)
    }

    fn reregister(&mut self, registry: &mio::Registry, token: mio::Token, interests: mio::Interest) -> io::Result<()> {
        self.ssl.reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        self.ssl.deregister(registry)
    }
}

/// A link registers its socket, whatever carries it; a gone link has none.
impl<S: Read + Write + AsFd + mio::event::Source> mio::event::Source for Link<S> {
    fn register(&mut self, registry: &mio::Registry, token: mio::Token, interests: mio::Interest) -> io::Result<()> {
        match self {
            Link::Plain(s) => s.register(registry, token, interests),
            Link::Tls(t) => t.register(registry, token, interests),
            Link::Gone => Err(io::ErrorKind::NotConnected.into()),
        }
    }

    fn reregister(&mut self, registry: &mio::Registry, token: mio::Token, interests: mio::Interest) -> io::Result<()> {
        match self {
            Link::Plain(s) => s.reregister(registry, token, interests),
            Link::Tls(t) => t.reregister(registry, token, interests),
            Link::Gone => Err(io::ErrorKind::NotConnected.into()),
        }
    }

    fn deregister(&mut self, registry: &mio::Registry) -> io::Result<()> {
        match self {
            Link::Plain(s) => s.deregister(registry),
            Link::Tls(t) => t.deregister(registry),
            Link::Gone => Err(io::ErrorKind::NotConnected.into()),
        }
    }
}

thread_local! {
    /// `nd_log_limit_static_thread_var(erl, 1, 0)` of the SSL records: one a second per thread.
    static SSL_ERRORS: ErrorLimit = const { ErrorLimit::new(1, 0) };
    static SSL_STATES: ErrorLimit = const { ErrorLimit::new(1, 0) };
}

/// `socket_peers()` at the time of a record (these TLS records' and the stream sender's): the socket's local and
/// remote addresses, `unknown`:0 for one the kernel no longer has (a peer that reset), `not connected`:0 without a
/// socket. The calls leave their errno, which a record reports.
pub fn socket_peers(fd: Option<RawFd>) -> [(String, u16); 2] {
    use nix::sys::socket::{SockaddrStorage, getpeername, getsockname};
    let Some(fd) = fd else {
        return [("not connected".into(), 0), ("not connected".into(), 0)];
    };
    // C reads any family but IPv4 as IPv6, a unix socket's too (garbage for a `unix:` `:SSL` parent, whose local side
    // is unnamed): here it is "unknown":0 (D107.10)
    let part = |a: nix::Result<SockaddrStorage>| match a.ok() {
        Some(a) if a.as_sockaddr_in().is_some() => a.as_sockaddr_in().map(|v4| (v4.ip().to_string(), v4.port())),
        Some(a) => a.as_sockaddr_in6().map(|v6| (v6.ip().to_string(), v6.port())),
        None => None,
    }
    .unwrap_or_else(|| ("unknown".into(), 0));
    let remote = part(getpeername::<SockaddrStorage>(fd));
    let local = part(getsockname::<SockaddrStorage>(fd));
    [local, remote]
}

/// `is_handshake_complete()`'s record of an operation on a connection that is not established.
fn incomplete(op: &str, state: State, fd: RawFd) {
    let what = match state {
        State::Init => "an incomplete",
        State::Failed => "a failed",
        State::Complete => return,
    };
    let [(local_ip, local_port), (remote_ip, remote_port)] = socket_peers(Some(fd));
    let errno = Errno::last_raw();
    SSL_STATES.with(|limit| {
        nd_log_limit!(
            limit,
            Source::Daemon,
            Priority::Warning,
            errno = errno;
            "SSL: on socket local [[{local_ip}]:{local_port}] <-> remote [[{remote_ip}]:{remote_port}], attempt to {op} \
             on {what} connection"
        );
    });
}

/// `SSL_get_error()`'s codes as `netdata_ssl_log_error_queue()` names them; a packed error is none of them.
fn error_code_name(code: u64) -> &'static str {
    match code {
        1 => "SSL_ERROR_SSL",
        2 => "SSL_ERROR_WANT_READ",
        3 => "SSL_ERROR_WANT_WRITE",
        4 => "SSL_ERROR_WANT_X509_LOOKUP",
        5 => "SSL_ERROR_SYSCALL",
        6 => "SSL_ERROR_ZERO_RETURN",
        7 => "SSL_ERROR_WANT_CONNECT",
        8 => "SSL_ERROR_WANT_ACCEPT",
        9 => "SSL_ERROR_WANT_ASYNC",
        10 => "SSL_ERROR_WANT_ASYNC_JOB",
        11 => "SSL_ERROR_WANT_CLIENT_HELLO_CB",
        12 => "SSL_ERROR_WANT_RETRY_VERIFY",
        _ => "SSL_ERROR_UNKNOWN",
    }
}

/// `ERR_LIB_SSL`.
const LIB_SSL: i32 = 20;

/// `SSL_alert_type_string_long()` of OpenSSL 3.5.7 (`openssl/openssl @ openssl-3.5.7`, `ssl/ssl_stat.c`).
fn alert_type(value: i32) -> &'static str {
    match value >> 8 {
        1 => "warning",
        2 => "fatal",
        _ => "unknown",
    }
}

/// `SSL_alert_desc_string_long()` of OpenSSL 3.5.7, by the alert codes of its `ssl3.h` and `tls1.h`.
fn alert_desc(value: i32) -> &'static str {
    match value & 0xff {
        0 => "close notify",
        10 => "unexpected message",
        20 => "bad record mac",
        30 => "decompression failure",
        40 => "handshake failure",
        41 => "no certificate",
        42 => "bad certificate",
        43 => "unsupported certificate",
        44 => "certificate revoked",
        45 => "certificate expired",
        46 => "certificate unknown",
        47 => "illegal parameter",
        21 => "decryption failed",
        22 => "record overflow",
        48 => "unknown CA",
        49 => "access denied",
        50 => "decode error",
        51 => "decrypt error",
        60 => "export restriction",
        70 => "protocol version",
        71 => "insufficient security",
        80 => "internal error",
        90 => "user canceled",
        100 => "no renegotiation",
        110 => "unsupported extension",
        111 => "certificate unobtainable",
        112 => "unrecognized name",
        113 => "bad certificate status response",
        114 => "bad certificate hash value",
        115 => "unknown PSK identity",
        120 => "no application protocol",
        _ => "unknown",
    }
}

/// `netdata_ssl_log_error_queue()`: a record for `code` (`SSL_get_error()`'s, when set), then one for each queued
/// error, all under the thread's one-a-second limit. The socket's addresses and the errno are the record's time's.
pub fn log_error_queue(call: &str, ssl: Option<&SslRef>, fd: Option<RawFd>, code: i32, queue: &[Error]) {
    if code == 0 && queue.is_empty() {
        return;
    }
    let [(local_ip, local_port), (remote_ip, remote_port)] = socket_peers(fd);
    let errno = Errno::last_raw();
    let state = ssl.map_or("No SSL connection", SslRef::state_string_long);
    let cipher = ssl.map_or("Unknown", |s| s.current_cipher().map_or("(NONE)", |c| c.name()));
    let alpn = ssl.and_then(SslRef::selected_alpn_protocol).unwrap_or(b"");
    let alpn = String::from_utf8_lossy(alpn);
    // (err, err_code, err_str, reason_code, reason, alert type, alert description)
    let mut lines: Vec<(u64, &str, String, i32, String, &str, &str)> = Vec::new();
    if code != 0 {
        // ERR_error_string_n() of a small code: no library, no reason text
        let code = code as u64;
        let text = format!("error:{code:08X}:lib(0)::reason({code})");
        lines.push((code, error_code_name(code), text, code as i32, "Unknown".into(), "None", "None"));
    }
    for e in queue {
        // c_ulong: 32 bits on 32-bit targets
        #[allow(clippy::useless_conversion)]
        let code = u64::from(e.code());
        let reason_code = e.reason_code();
        let (at, ad) = if e.library_code() == LIB_SSL {
            (alert_type(reason_code), alert_desc(reason_code))
        } else {
            ("None", "None")
        };
        let reason = e.reason().unwrap_or("Unknown").to_string();
        lines.push((code, error_code_name(code), error_string(e), reason_code, reason, at, ad));
    }
    for (err, name, text, reason_code, reason, at, ad) in lines {
        SSL_ERRORS.with(|limit| {
            nd_log_limit!(
                limit,
                Source::Daemon,
                Priority::Err,
                errno = errno;
                "SSL ERROR: {call}() on socket local [[{local_ip}]:{local_port}] <-> remote [[{remote_ip}]:{remote_port}], \
                 State [{state}], Cipher: [{cipher}], ALPN: [{alpn}], Error [{err}, {name}, {text}], Reason [{reason_code}, \
                 {reason}], Alert [{at}, {ad}], Errno [{errno}]"
            );
        });
    }
}

#[cfg(test)]
mod tests {
    use openssl::asn1::Asn1Time;
    use openssl::ec::{EcGroup, EcKey};
    use openssl::hash::MessageDigest;
    use openssl::nid::Nid;
    use openssl::pkey::{PKey, Private};
    use openssl::x509::{X509, X509NameBuilder};

    use super::*;

    fn key() -> PKey<Private> {
        let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
        PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
    }

    fn certificate(key: &PKey<Private>) -> X509 {
        let mut name = X509NameBuilder::new().unwrap();
        name.append_entry_by_text("CN", "localhost").unwrap();
        let name = name.build();
        let mut b = X509::builder().unwrap();
        b.set_version(2).unwrap();
        b.set_subject_name(&name).unwrap();
        b.set_issuer_name(&name).unwrap();
        b.set_pubkey(key).unwrap();
        b.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        b.set_not_after(&Asn1Time::days_from_now(1).unwrap()).unwrap();
        b.sign(key, MessageDigest::sha256()).unwrap();
        b.build()
    }

    fn files(dir: &tempfile::TempDir, key: &PKey<Private>, cert: &X509) -> (String, String) {
        let (k, c) = (dir.path().join("key.pem"), dir.path().join("cert.pem"));
        std::fs::write(&k, key.private_key_to_pem_pkcs8().unwrap()).unwrap();
        std::fs::write(&c, cert.to_pem().unwrap()).unwrap();
        (k.to_str().unwrap().into(), c.to_str().unwrap().into())
    }

    fn config<'a>(key: &'a str, certificate: &'a str, ciphers: &'a str) -> ServerConfig<'a> {
        ServerConfig { key, certificate, tls_version: "1.3", ciphers, skip_verification: false }
    }

    /// A matching pair builds; a key that is not the certificate's fails the check with the oldest queued error, a
    /// bad cipher list's when it failed first.
    #[test]
    fn contexts_as_c() {
        init();
        let dir = tempfile::tempdir().unwrap();
        let k = key();
        let (key_file, cert_file) = files(&dir, &k, &certificate(&k));
        assert!(server_context(&config(&key_file, &cert_file, "none")).is_ok());
        let other = tempfile::tempdir().unwrap();
        let (other_key, _) = files(&other, &key(), &certificate(&k));
        let mismatch = server_context(&config(&other_key, &cert_file, "none")).unwrap_err();
        assert!(mismatch.starts_with("SSL cannot check the private key: error:"), "{mismatch}");
        // the key's load queued the x509 error first
        assert!(mismatch.ends_with(":x509 certificate routines::key values mismatch"), "{mismatch}");
        let bad = server_context(&config(&other_key, &cert_file, "NOPE")).unwrap_err();
        assert!(bad.ends_with(":SSL routines::no cipher match"), "{bad}");
        assert!(server_context(&config(&key_file, &cert_file, "NOPE")).is_ok(), "only reported");
        std::fs::write(&key_file, "not a key").unwrap();
        let garbage = server_context(&config(&key_file, &cert_file, "none")).unwrap_err();
        assert!(garbage.starts_with("SSL cannot check the private key: error:"), "{garbage}");
    }

    /// OpenSSL 3.5.7's alert names, and the codes C names in its records.
    #[test]
    fn alerts_and_codes_as_openssl() {
        assert_eq!((alert_type(0x228), alert_desc(0x228)), ("fatal", "handshake failure"));
        assert_eq!((alert_type(0x10a), alert_desc(0x10a)), ("warning", "unexpected message"));
        assert_eq!((alert_type(267), alert_desc(267)), ("warning", "unknown"));
        assert_eq!((alert_type(198), alert_desc(198)), ("unknown", "unknown"));
        assert_eq!((error_code_name(1), error_code_name(5), error_code_name(0x0a00010b)), ("SSL_ERROR_SSL", "SSL_ERROR_SYSCALL", "SSL_ERROR_UNKNOWN"));
    }

    /// A handshake on a non-blocking socket waits for the client, then data goes both ways and the close sends a
    /// `close_notify` the client reads as the end.
    #[test]
    fn a_connection_as_c() {
        use openssl::ssl::{SslConnector, SslVerifyMode};
        use std::os::unix::net::UnixStream;
        init();
        let dir = tempfile::tempdir().unwrap();
        let k = key();
        let (key_file, cert_file) = files(&dir, &k, &certificate(&k));
        let context = server_context(&config(&key_file, &cert_file, "none")).unwrap();
        let (server, client) = UnixStream::pair().unwrap();
        server.set_nonblocking(true).unwrap();
        let mut t = TlsStream::new(&context, server).unwrap();
        assert!(matches!(t.accept(), Handshake::Pending { write: false }), "no client hello yet");
        let peer = std::thread::spawn(move || {
            let mut b = SslConnector::builder(SslMethod::tls_client()).unwrap();
            b.set_verify(SslVerifyMode::NONE);
            let mut s = b.build().connect("localhost", client).unwrap();
            s.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
            let mut answer = Vec::new();
            s.read_to_end(&mut answer).unwrap();
            answer
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !matches!(t.accept(), Handshake::Complete) {
            assert!(std::time::Instant::now() < deadline, "the handshake completes");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(t.state(), State::Complete);
        let mut buf = [0u8; 64];
        let n = loop {
            match t.read(&mut buf) {
                Ok(n) => break n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => std::thread::sleep(std::time::Duration::from_millis(5)),
                Err(e) => panic!("{e}"),
            }
        };
        assert_eq!(&buf[..n], b"GET / HTTP/1.1\r\n\r\n");
        assert_eq!(t.write(b"answer").unwrap(), 6);
        // the drop sends the close_notify the client reads as the end
        drop(t);
        assert_eq!(peer.join().unwrap(), b"answer");
    }

    /// What a test server saw: the client's server name, its ALPN list, and the bytes it read.
    type Seen = (Option<String>, Vec<u8>, usize);

    /// A server on its own non-blocking side of a pair, recording the client's server name and ALPN list, then
    /// echoing what it reads until the close.
    fn tls_server(
        key: &PKey<Private>,
        cert: &X509,
        stream: std::os::unix::net::UnixStream,
    ) -> std::thread::JoinHandle<Seen> {
        tls_peer(key, cert, stream, true)
    }

    /// `tls_server`, echoing or only reading; also the count of the bytes it read.
    fn tls_peer(
        key: &PKey<Private>,
        cert: &X509,
        stream: std::os::unix::net::UnixStream,
        echo: bool,
    ) -> std::thread::JoinHandle<Seen> {
        use std::sync::{Arc, Mutex};
        let seen: Arc<Mutex<Seen>> = Arc::default();
        let mut b = SslContextBuilder::new(SslMethod::tls_server()).unwrap();
        b.set_private_key(key).unwrap();
        b.set_certificate(cert).unwrap();
        let (names, protos) = (Arc::clone(&seen), Arc::clone(&seen));
        b.set_servername_callback(move |ssl, _| {
            names.lock().unwrap().0 = ssl.servername(openssl::ssl::NameType::HOST_NAME).map(str::to_string);
            Ok(())
        });
        b.set_alpn_select_callback(move |_, client| {
            protos.lock().unwrap().1 = client.to_vec();
            Err(openssl::ssl::AlpnError::NOACK)
        });
        let context = b.build();
        stream.set_nonblocking(true).unwrap();
        std::thread::spawn(move || {
            let mut t = TlsStream::new(&context, stream).unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            loop {
                match t.accept() {
                    Handshake::Complete => break,
                    Handshake::Failed => return seen.lock().unwrap().clone(),
                    Handshake::Pending { .. } if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Handshake::Pending { .. } => panic!("the handshake completes"),
                }
            }
            let mut buf = [0u8; 64];
            loop {
                match t.read(&mut buf) {
                    Ok(0) | Err(_) if std::time::Instant::now() > deadline => break,
                    Ok(0) => break,
                    Ok(n) => {
                        seen.lock().unwrap().2 += n;
                        if echo {
                            let _ = t.write(&buf[..n]);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(std::time::Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
            seen.lock().unwrap().clone()
        })
    }

    /// A stream sender's connection verifies the parent's certificate against its CAfile, sends the parent's name
    /// and C's ALPN list, and carries data both ways.
    #[test]
    fn a_client_connects_as_c() {
        init();
        let dir = tempfile::tempdir().unwrap();
        let k = key();
        let cert = certificate(&k);
        let (_, cert_file) = files(&dir, &k, &cert);
        let context = streaming_sender_context(&ClientConfig { ca_file: &cert_file, ..ClientConfig::default() }).unwrap();
        let (server, client) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer = tls_server(&k, &cert, server);
        let mut t = TlsStream::connect(&context, client, Some("localhost"), true).unwrap();
        assert_eq!(t.state(), State::Complete);
        assert_eq!(t.write(b"ping").unwrap(), 4);
        let mut buf = [0u8; 4];
        assert_eq!(t.read(&mut buf).unwrap(), 4);
        assert_eq!(&buf, b"ping");
        drop(t);
        let (name, alpn, _) = peer.join().unwrap();
        assert_eq!(name.as_deref(), Some("localhost"));
        assert_eq!(alpn, STREAM_ALPN);
    }

    /// Without the parent's CA the certificate fails the test after the handshake, with C's record (a self-signed
    /// leaf: 18); skipping the verification accepts it.
    #[test]
    fn an_untrusted_certificate_fails_unless_skipped() {
        init();
        let k = key();
        let cert = certificate(&k);
        let strict = streaming_sender_context(&ClientConfig::default()).unwrap();
        let (server, client) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer = tls_server(&k, &cert, server);
        let (result, records) = netdata_agent_log::capture(|| TlsStream::connect(&strict, client, None, true).err());
        assert_eq!(result, Some(OpenError::InvalidCertificate));
        let texts: Vec<String> = records.iter().filter_map(|r| r.message.clone()).collect();
        assert_eq!(
            texts,
            ["SSL RFC4158 check:  We have a invalid certificate, the tests result with 18 and message \
              error:00000000:lib(0)::reason(0)"]
        );
        let _ = peer.join();
        let skip = streaming_sender_context(&ClientConfig { skip_verification: true, ..ClientConfig::default() }).unwrap();
        let (server, client) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer = tls_server(&k, &cert, server);
        assert!(TlsStream::connect(&skip, client, None, false).is_ok(), "not verified");
        let _ = peer.join();
    }

    /// A peer that answers the client hello with plain text fails the handshake, with C's SSL_connect record.
    #[test]
    fn a_plaintext_peer_fails_the_handshake() {
        use std::io::Write as _;
        init();
        let context = streaming_sender_context(&ClientConfig::default()).unwrap();
        let (mut server, client) = std::os::unix::net::UnixStream::pair().unwrap();
        server.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").unwrap();
        drop(server);
        let (result, records) = netdata_agent_log::capture(|| TlsStream::connect(&context, client, None, true).err());
        assert_eq!(result, Some(OpenError::CantEstablish));
        let first = records.first().and_then(|r| r.message.clone()).unwrap_or_default();
        assert!(first.starts_with("SSL ERROR: SSL_connect() "), "{first}");
    }

    /// Over a full socket a client write waits (WouldBlock) and its retry from the same first byte with more after it,
    /// as the sender's ring grows its first chunk, goes through: the context's partial and moving writes (review R46
    /// m1).
    #[test]
    fn a_blocked_write_takes_a_grown_retry() {
        init();
        let dir = tempfile::tempdir().unwrap();
        let k = key();
        let cert = certificate(&k);
        let (_, cert_file) = files(&dir, &k, &cert);
        let context = streaming_sender_context(&ClientConfig { ca_file: &cert_file, ..ClientConfig::default() }).unwrap();
        let (server, client) = std::os::unix::net::UnixStream::pair().unwrap();
        let peer = tls_peer(&k, &cert, server, false);
        let mut t = TlsStream::connect(&context, client, Some("localhost"), true).unwrap();
        t.get_ref().set_nonblocking(true).unwrap();
        let data: Vec<u8> = (0..4 << 20).map(|i: u32| i as u8).collect();
        const CHUNK: usize = 64 << 10;
        let (mut sent, mut end, mut blocked) = (0, CHUNK, 0);
        while sent < data.len() {
            match t.write(&data[sent..end.min(data.len())]) {
                Ok(n) => {
                    sent += n;
                    end = sent + CHUNK;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    blocked += 1;
                    end += CHUNK;
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(e) => panic!("write at {sent}: {e}"),
            }
        }
        assert!(blocked > 0, "the socket never filled");
        drop(t);
        assert_eq!(peer.join().unwrap().2, data.len());
    }

    /// A CAfile that does not exist is reported, and the context still builds with OpenSSL's defaults.
    #[test]
    fn a_missing_cafile_is_reported() {
        init();
        let (context, records) = netdata_agent_log::capture(|| {
            streaming_sender_context(&ClientConfig { ca_file: "/nonexistent/ca.pem", ..ClientConfig::default() })
        });
        assert!(context.is_some());
        let texts: Vec<String> = records.iter().filter_map(|r| r.message.clone()).collect();
        assert_eq!(
            texts,
            ["Netdata can not verify custom CAfile or CApath for parent's SSL certificate, so it will use the default \
              OpenSSL configuration to validate certificates!"]
        );
    }

    #[test]
    fn versions_as_c() {
        let v = ["1", "1.0", "1.1", "1.2", "1.3", "bogus"].map(tls_version);
        assert_eq!(
            v,
            [
                SslVersion::TLS1,
                SslVersion::TLS1,
                SslVersion::TLS1_1,
                SslVersion::TLS1_2,
                SslVersion::TLS1_3,
                SslVersion::TLS1_3
            ]
        );
    }
}
