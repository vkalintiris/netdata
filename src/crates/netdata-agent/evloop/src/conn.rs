//! The stream sockets the web workers and stream threads serve: TCP, or unix from a `unix:` listener (D53.2). A
//! [`Conn`] is registered with a poller; a [`Stream`] is the same socket in its std form, for the blocking exchanges
//! of a STREAM handshake.

use std::io::{self, IoSlice, Read, Write};
use std::net::Shutdown;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, RawFd};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use mio::event::Source;
use mio::{Interest, Registry, Token};

/// A connection registered with a poller.
#[derive(Debug)]
pub enum Conn {
    Tcp(mio::net::TcpStream),
    Unix(mio::net::UnixStream),
}

/// A connection outside any poller.
#[derive(Debug)]
pub enum Stream {
    Tcp(std::net::TcpStream),
    Unix(UnixStream),
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
    pub fn from_std(stream: Stream) -> Conn {
        match stream {
            Stream::Tcp(s) => Conn::Tcp(mio::net::TcpStream::from_std(s)),
            Stream::Unix(s) => Conn::Unix(mio::net::UnixStream::from_std(s)),
        }
    }

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

impl From<Conn> for Stream {
    fn from(conn: Conn) -> Stream {
        match conn {
            Conn::Tcp(s) => Stream::Tcp(s.into()),
            Conn::Unix(s) => Stream::Unix(s.into()),
        }
    }
}

impl Stream {
    pub fn try_clone(&self) -> io::Result<Stream> {
        Ok(match self {
            Stream::Tcp(s) => Stream::Tcp(s.try_clone()?),
            Stream::Unix(s) => Stream::Unix(s.try_clone()?),
        })
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        each!(self, s => s.shutdown(how))
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        each!(self, s => s.set_nonblocking(nonblocking))
    }

    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        each!(self, s => s.set_read_timeout(timeout))
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        each!(self, s => s.set_write_timeout(timeout))
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

impl Read for &Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(s) => (&*s).read(buf),
            Stream::Unix(s) => (&*s).read(buf),
        }
    }
}

impl Write for &Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match *self {
            Stream::Tcp(s) => (&*s).write(buf),
            Stream::Unix(s) => (&*s).write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
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

impl AsFd for Stream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        each!(self, s => s.as_fd())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_pair_round_trips_through_both_forms() {
        let (a, b) = UnixStream::pair().unwrap();
        let (a, b) = (Stream::Unix(a), Stream::Unix(b));
        (&a).write_all(b"ping").unwrap();
        let mut conn = Conn::from_std(b);
        assert!(conn.is_unix());
        let mut buf = [0u8; 4];
        conn.read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"ping");
        let back = Stream::from(conn);
        let clone = back.try_clone().unwrap();
        (&clone).write_all(b"pong").unwrap();
        let mut buf = [0u8; 4];
        (&a).read_exact(&mut buf).unwrap();
        assert_eq!(&buf, b"pong");
        back.shutdown(Shutdown::Both).unwrap();
    }
}
