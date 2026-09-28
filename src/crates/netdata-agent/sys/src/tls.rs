//! OpenSSL's socket BIO under a TLS connection (decision D97.2): `SSL_set_fd()` and the calls C's `NETDATA_SSL` makes
//! on it. The openssl crate's own streams read through a BIO of their own that never reports the end of file, so
//! OpenSSL cannot tell a peer that closed from a failed call (`SSL_ERROR_SYSCALL` where C gets `SSL_ERROR_SSL`), and
//! its alerts take another path. Here OpenSSL does its own I/O on the descriptor, as in C.

use std::ffi::c_int;
use std::os::fd::{AsFd, AsRawFd};

use foreign_types::ForeignTypeRef;
use openssl::ssl::{Ssl, SslRef};

// openssl-sys does not declare it; libssl, which openssl-sys links, exports it
unsafe extern "C" {
    fn SSL_set_fd(ssl: *mut openssl_sys::SSL, fd: c_int) -> c_int;
}

/// A TLS connection on `stream`'s descriptor. The stream is owned (`AsFd`) and never handed out mutably, and the
/// connection is freed before the stream closes its descriptor (field order), so OpenSSL never works on a descriptor
/// it does not have. A poller registers the stream through this type's `mio::event::Source`.
#[derive(Debug)]
pub struct SocketSsl<S> {
    ssl: Ssl,
    stream: S,
}

/// `SSL_read()`'s and `SSL_write()`'s lengths are `int`s, as C casts them.
fn len(buf: &[u8]) -> c_int {
    c_int::try_from(buf.len()).unwrap_or(c_int::MAX)
}

impl<S: AsFd> SocketSsl<S> {
    /// `SSL_set_fd()` (OpenSSL 3 also enables kTLS on the socket, as for C); on a failure (OpenSSL could not make the
    /// BIO, the error queue says why) both come back.
    pub fn new(ssl: Ssl, stream: S) -> Result<SocketSsl<S>, (Ssl, S)> {
        // SAFETY: `ssl` is a live connection; the descriptor stays open while it lives (see the struct).
        let set = unsafe { SSL_set_fd(ssl.as_ptr(), stream.as_fd().as_raw_fd()) };
        if set == 1 { Ok(SocketSsl { ssl, stream }) } else { Err((ssl, stream)) }
    }

    pub fn ssl(&self) -> &SslRef {
        &self.ssl
    }

    pub fn get_ref(&self) -> &S {
        &self.stream
    }

    /// `SSL_accept()`.
    pub fn accept(&mut self) -> c_int {
        // SAFETY: a live connection.
        unsafe { openssl_sys::SSL_accept(self.ssl.as_ptr()) }
    }

    /// `SSL_read()` into `buf`.
    pub fn read(&mut self, buf: &mut [u8]) -> c_int {
        // SAFETY: a live connection; OpenSSL writes at most `len(buf)` bytes into `buf`.
        unsafe { openssl_sys::SSL_read(self.ssl.as_ptr(), buf.as_mut_ptr().cast(), len(buf)) }
    }

    /// `SSL_peek()` into `buf`.
    pub fn peek(&mut self, buf: &mut [u8]) -> c_int {
        // SAFETY: as `read()`.
        unsafe { openssl_sys::SSL_peek(self.ssl.as_ptr(), buf.as_mut_ptr().cast(), len(buf)) }
    }

    /// `SSL_write()` of `buf`.
    pub fn write(&mut self, buf: &[u8]) -> c_int {
        // SAFETY: a live connection; OpenSSL reads at most `len(buf)` bytes of `buf`.
        unsafe { openssl_sys::SSL_write(self.ssl.as_ptr(), buf.as_ptr().cast(), len(buf)) }
    }

    /// `SSL_shutdown()`.
    pub fn shutdown(&mut self) -> c_int {
        // SAFETY: a live connection.
        unsafe { openssl_sys::SSL_shutdown(self.ssl.as_ptr()) }
    }

    /// `SSL_get_error()` of the last call's `ret`.
    pub fn error(&self, ret: c_int) -> c_int {
        // SAFETY: a live connection; it only reads the connection and the thread's error queue.
        unsafe { openssl_sys::SSL_get_error(self.ssl.as_ptr(), ret) }
    }
}

/// The stream's registration with a poller, delegated: no caller gets the stream itself mutably.
impl<S: AsFd + mio::event::Source> mio::event::Source for SocketSsl<S> {
    fn register(&mut self, registry: &mio::Registry, token: mio::Token, interests: mio::Interest) -> std::io::Result<()> {
        self.stream.register(registry, token, interests)
    }

    fn reregister(
        &mut self,
        registry: &mio::Registry,
        token: mio::Token,
        interests: mio::Interest,
    ) -> std::io::Result<()> {
        self.stream.reregister(registry, token, interests)
    }

    fn deregister(&mut self, registry: &mio::Registry) -> std::io::Result<()> {
        self.stream.deregister(registry)
    }
}
