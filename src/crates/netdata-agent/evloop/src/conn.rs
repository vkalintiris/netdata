//! The stream sockets the web workers and stream threads serve: TCP, or unix from a `unix:` listener (D53.2),
//! registered with a poller. A blocking exchange (a STREAM handshake's reply) switches the descriptor with
//! `socket2::SockRef`.

use std::io::{self, IoSlice, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};

use mio::event::Source;
use mio::{Interest, Registry, Token};

/// A connection registered with a poller.
#[derive(Debug)]
pub enum Conn {
    Tcp(mio::net::TcpStream),
    Unix(mio::net::UnixStream),
}

/// Calls `$method` on whichever socket `$self` holds.
macro_rules! each {
    ($self:expr, $s:ident => $e:expr) => {
        match $self {
            Self::Tcp($s) => $e,
            Self::Unix($s) => $e,
        }
    };
}

impl Conn {
    pub fn is_unix(&self) -> bool {
        matches!(self, Conn::Unix(_))
    }

    pub fn take_error(&self) -> io::Result<Option<io::Error>> {
        each!(self, s => s.take_error())
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        each!(self, s => s.shutdown(how))
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        each!(self, s => s.read(buf))
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        each!(self, s => s.write(buf))
    }

    fn write_vectored(&mut self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        each!(self, s => s.write_vectored(bufs))
    }

    fn flush(&mut self) -> io::Result<()> {
        each!(self, s => s.flush())
    }
}

impl Source for Conn {
    fn register(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        each!(self, s => s.register(registry, token, interests))
    }

    fn reregister(
        &mut self,
        registry: &Registry,
        token: Token,
        interests: Interest,
    ) -> io::Result<()> {
        each!(self, s => s.reregister(registry, token, interests))
    }

    fn deregister(&mut self, registry: &Registry) -> io::Result<()> {
        each!(self, s => s.deregister(registry))
    }
}

impl AsFd for Conn {
    fn as_fd(&self) -> BorrowedFd<'_> {
        each!(self, s => s.as_fd())
    }
}

impl AsRawFd for Conn {
    fn as_raw_fd(&self) -> RawFd {
        each!(self, s => s.as_raw_fd())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_pair_round_trips() {
        let (a, b) = mio::net::UnixStream::pair().unwrap();
        let (mut a, mut b) = (Conn::Unix(a), Conn::Unix(b));
        assert!(b.is_unix());
        a.write_all(b"ping").unwrap();
        let mut buf = [0u8; 4];
        b.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        b.shutdown(Shutdown::Both).unwrap();
        assert_eq!(a.read(&mut buf).unwrap(), 0);
    }
}
