//! Netdata Cloud reachability probe: DNS + TCP + certificate-validating TLS
//! handshake + HTTP status, entirely in-process (rustls with the webpki root
//! store). No bundle data is sent — a single GET / with no payload.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

/// Returns a small human-readable report; each probe stage reports OK or the
/// failure, so support can separate network problems from agent problems.
pub fn cloud_connectivity_report(host: &str) -> String {
    let mut out = String::new();
    let addrs: Vec<_> = match (host, 443u16).to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(e) => {
            out.push_str(&format!("dns resolve {host}: FAILED ({e})\n"));
            return out;
        }
    };
    if addrs.is_empty() {
        out.push_str(&format!("dns resolve {host}: FAILED (no addresses)\n"));
        return out;
    }
    out.push_str(&format!(
        "dns resolve {host}: OK ({})\n",
        addrs
            .iter()
            .map(|a| a.ip().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let stream = match TcpStream::connect_timeout(&addrs[0], PROBE_TIMEOUT) {
        Ok(s) => s,
        Err(e) => {
            out.push_str(&format!(
                "tcp connect {}:443: FAILED ({e})\n",
                addrs[0].ip()
            ));
            return out;
        }
    };
    out.push_str("tcp connect: OK\n");
    let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let server_name = match rustls::pki_types::ServerName::try_from(host.to_string()) {
        Ok(n) => n,
        Err(e) => {
            out.push_str(&format!("tls: invalid server name ({e})\n"));
            return out;
        }
    };
    let mut conn = match rustls::ClientConnection::new(Arc::new(config), server_name) {
        Ok(c) => c,
        Err(e) => {
            out.push_str(&format!("tls setup: FAILED ({e})\n"));
            return out;
        }
    };
    let mut stream = stream;
    let mut tls = rustls::Stream::new(&mut conn, &mut stream);
    let request = format!(
        "GET / HTTP/1.0\r\nHost: {host}\r\nUser-Agent: netdata-support-bundle/{}\r\nConnection: close\r\n\r\n",
        crate::consts::TOOL_VERSION
    );
    // the handshake (incl. certificate validation) happens on first write
    if let Err(e) = tls.write_all(request.as_bytes()) {
        out.push_str(&format!(
            "tls handshake (certificate validation): FAILED ({e})\n"
        ));
        return out;
    }
    let proto = tls
        .conn
        .protocol_version()
        .map(|v| format!("{v:?}"))
        .unwrap_or_else(|| "unknown".to_string());
    out.push_str(&format!(
        "tls handshake (certificate validated): OK ({proto})\n"
    ));

    let mut buf = [0u8; 4096];
    match tls.read(&mut buf) {
        Ok(n) if n > 0 => {
            let head = String::from_utf8_lossy(&buf[..n]);
            let status = head.lines().next().unwrap_or("").trim().to_string();
            out.push_str(&format!("http response: {status}\n"));
        }
        Ok(_) => out.push_str("http response: (connection closed without data)\n"),
        Err(e) => out.push_str(&format!("http read: FAILED ({e})\n")),
    }
    out
}
