// SPDX-License-Identifier: GPL-3.0-or-later

//! The socket options both directions of a stream set, as libnetdata's `socket.c`.

/// `LARGE_SOCK_SIZE`.
pub(crate) const LARGE_SOCK_SIZE: usize = 32 * 1024 * 1024;

/// `sock_enlarge_rcv_buf()` and `sock_enlarge_snd_buf()`: each buffer raised to 32 MiB when it is smaller, errors
/// ignored as their callers do.
pub(crate) fn enlarge_buffers(socket: &socket2::SockRef<'_>) {
    if socket.recv_buffer_size().is_ok_and(|size| size < LARGE_SOCK_SIZE) {
        let _ = socket.set_recv_buffer_size(LARGE_SOCK_SIZE);
    }
    if socket.send_buffer_size().is_ok_and(|size| size < LARGE_SOCK_SIZE) {
        let _ = socket.set_send_buffer_size(LARGE_SOCK_SIZE);
    }
}

#[cfg(test)]
mod tests {
    use std::net::{TcpListener, TcpStream};

    use socket2::{Domain, SockRef, Socket, Type};

    use super::*;

    /// Both buffers end as a socket asked for 32 MiB directly does, whatever the host's limits.
    #[test]
    fn the_buffers_grow_as_cs() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (accepted, _) = listener.accept().unwrap();
        let control = Socket::new(Domain::IPV4, Type::STREAM, None).unwrap();
        control.set_recv_buffer_size(LARGE_SOCK_SIZE).unwrap();
        control.set_send_buffer_size(LARGE_SOCK_SIZE).unwrap();
        let socket = SockRef::from(&accepted);
        enlarge_buffers(&socket);
        assert_eq!(socket.recv_buffer_size().unwrap(), control.recv_buffer_size().unwrap());
        assert_eq!(socket.send_buffer_size().unwrap(), control.send_buffer_size().unwrap());
    }
}
