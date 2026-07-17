//! Minimal HTTP/1.0 client for the local agent API.
//!
//! Local API reads target 127.0.0.1 directly over a raw TCP connection, so a
//! configured system proxy can never see or route diagnostic data — the
//! bypass is structural, not configuration-dependent. HTTP/1.0 with
//! Connection: close keeps the response un-chunked and EOF-delimited.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::Duration;

pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    /// true when the read stopped at the byte cap (body may be truncated)
    pub capped: bool,
    /// false when the read ended on timeout instead of EOF — the body may be
    /// cut mid-record and must not be treated as a complete document
    pub complete: bool,
}

/// GET `path` from the local agent. Returns Err on connect/IO/parse failure;
/// the caller treats that the same as an empty body (degrade, never fail the
/// run).
pub fn local_get(
    port: u16,
    path: &str,
    timeout: Duration,
    cap_bytes: usize,
) -> std::io::Result<HttpResponse> {
    // every caller passes literals today; the guard keeps a future dynamic
    // path from smuggling extra headers into the request
    if path.bytes().any(|b| b <= 0x20 || b == 0x7f) {
        return Err(std::io::Error::other("invalid characters in HTTP path"));
    }
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut stream = TcpStream::connect_timeout(&addr, timeout)?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = format!(
        "GET {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nUser-Agent: netdata-support-bundle/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        crate::consts::TOOL_VERSION
    );
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    let mut buf = [0u8; 65536];
    let started = std::time::Instant::now();
    let mut capped = false;
    let mut complete = false;
    loop {
        if started.elapsed() > timeout {
            break;
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                complete = true;
                break;
            }
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                // headers are small; cap_bytes bounds the body below
                if raw.len() > cap_bytes + 16384 {
                    capped = true;
                    break;
                }
            }
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break;
            }
            Err(e) => return Err(e),
        }
    }

    let header_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| p + 4)
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|p| p + 2))
        .ok_or_else(|| std::io::Error::other("no HTTP header terminator"))?;
    let head = String::from_utf8_lossy(&raw[..header_end]);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| std::io::Error::other("bad HTTP status line"))?;
    let mut body = raw.split_off(header_end);
    if body.len() > cap_bytes {
        body.truncate(cap_bytes);
        capped = true;
    }
    Ok(HttpResponse {
        status,
        body,
        capped,
        complete,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_cut_response_is_marked_incomplete() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = std::io::Read::read(&mut sock, &mut req);
            sock.write_all(b"HTTP/1.0 200 OK\r\n\r\n{\"password\":\"cut-mid-")
                .unwrap();
            sock.flush().unwrap();
            // hold the socket open past the client timeout: no EOF arrives
            std::thread::sleep(Duration::from_millis(1500));
        });
        let resp = local_get(port, "/x", Duration::from_millis(400), 65536).unwrap();
        assert_eq!(resp.status, 200);
        assert!(
            !resp.complete,
            "a timeout-cut body must not read as complete"
        );
        server.join().unwrap();
    }

    #[test]
    fn complete_response_reads_complete() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // drain the request first so the close sends FIN, not RST
            let mut req = [0u8; 4096];
            let _ = std::io::Read::read(&mut sock, &mut req);
            sock.write_all(b"HTTP/1.0 200 OK\r\n\r\n{\"ok\":true}")
                .unwrap();
            let _ = sock.shutdown(std::net::Shutdown::Write);
            let mut sink = [0u8; 1024];
            while matches!(std::io::Read::read(&mut sock, &mut sink), Ok(n) if n > 0) {}
        });
        let resp = local_get(port, "/x", Duration::from_secs(2), 65536).unwrap();
        assert_eq!(resp.status, 200);
        assert!(resp.complete);
        assert_eq!(resp.body, b"{\"ok\":true}");
        server.join().unwrap();
    }

    #[test]
    fn control_characters_in_path_are_refused() {
        assert!(local_get(1, "/a\r\nX: y", Duration::from_millis(10), 16).is_err());
    }
}
