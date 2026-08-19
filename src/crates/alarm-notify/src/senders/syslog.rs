//! Native syslog delivery.
//!
//! The script shelled out to `logger`, which does not exist on Windows and is
//! util-linux-specific for its `-n`/`-P` remote flags. Emitting the records directly
//! removes that dependency while keeping both the documented recipient grammar and
//! what `logger` actually put on the wire, which differs by destination:
//!
//! * locally, `<PRI>TAG: MESSAGE` - the receiving daemon supplies the timestamp and
//!   the host, and a timestamp in the datagram would be read as part of the message;
//! * remotely, RFC 5424, which util-linux has sent by default since 2.26.
//!
//! The recipient grammar is unchanged:
//!
//! ```text
//! [[facility.level][@host[:port]]/]prefix
//! ```

use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::time::Duration;

use crate::args::Status;
use crate::datefmt;
use crate::senders::Ctx;

const DEFAULT_FACILITY: &str = "local6";
const DEFAULT_SYSLOG_PORT: u16 = 514;

pub fn syslog(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("syslog") {
        return false;
    }

    let facility_default = {
        let f = ctx.cfg.str("SYSLOG_FACILITY");
        if f.is_empty() { DEFAULT_FACILITY } else { f }
    };
    // Severity follows the alert status, as before.
    let level_default = match ctx.args.status() {
        Status::Critical => "crit",
        Status::Warning => "warning",
        _ => "info",
    };

    let mut sent = false;
    for target in ctx.to("syslog") {
        let t = Target::parse(&target, facility_default, level_default);
        let message = format!(
            "{} {} on {} at {}: {} {}",
            t.prefix,
            ctx.args.status,
            ctx.msg.host,
            ctx.msg.date,
            ctx.args.chart,
            ctx.args.value_string
        );
        // The hostname is the sending node's, as `logger` reported it; the alert's own
        // host is already the subject of the message text.
        let record = match &t.server {
            Some(_) => format_rfc5424(
                t.priority_value(),
                &crate::hostname::full(),
                &syslog_tag(),
                &message,
            ),
            None => format_local(t.priority_value(), &syslog_tag(), &message),
        };

        match deliver(&t, record.as_bytes()) {
            Ok(()) => {
                tracing::info!("sent syslog notification to '{target}' for {}", ctx.what());
                sent = true;
            }
            Err(e) => tracing::error!(
                "failed to send syslog notification to '{target}' for {}: {e}",
                ctx.what()
            ),
        }
    }
    sent
}

#[derive(Debug, PartialEq, Eq)]
struct Target {
    facility: String,
    level: String,
    server: Option<String>,
    port: u16,
    prefix: String,
}

impl Target {
    /// Split the documented recipient grammar.
    fn parse(raw: &str, facility_default: &str, level_default: &str) -> Self {
        let mut t = Self {
            facility: facility_default.to_string(),
            level: level_default.to_string(),
            server: None,
            port: DEFAULT_SYSLOG_PORT,
            prefix: raw.to_string(),
        };

        let Some((head, prefix)) = raw.split_once('/') else {
            // No `/`: the whole value is the message prefix.
            return t;
        };
        t.prefix = prefix.to_string();

        let (priority_part, server_part) = match head.split_once('@') {
            Some((p, s)) => (p, Some(s)),
            None => (head, None),
        };

        if !priority_part.is_empty() {
            match priority_part.split_once('.') {
                Some((f, l)) => {
                    if !f.is_empty() {
                        t.facility = f.to_string();
                    }
                    if !l.is_empty() {
                        t.level = l.to_string();
                    }
                }
                // A bare word is a facility.
                None => t.facility = priority_part.to_string(),
            }
        }

        if let Some(server) = server_part.filter(|s| !s.is_empty()) {
            let (host, port) = split_host_port(server);
            t.server = Some(host);
            if let Some(p) = port {
                t.port = p;
            }
        }

        t
    }

    /// The RFC 3164 PRI value: facility * 8 + severity.
    fn priority_value(&self) -> u8 {
        facility_code(&self.facility) * 8 + severity_code(&self.level)
    }
}

/// Split `host[:port]`, honouring bracketed IPv6 literals.
fn split_host_port(server: &str) -> (String, Option<u16>) {
    if let Some(rest) = server.strip_prefix('[') {
        return match rest.split_once(']') {
            Some((host, tail)) => {
                let port = tail.strip_prefix(':').and_then(|p| p.parse().ok());
                (host.to_string(), port)
            }
            None => (rest.to_string(), None),
        };
    }
    // An unbracketed address with several colons is a bare IPv6 literal, not
    // host:port.
    if server.matches(':').count() > 1 {
        return (server.to_string(), None);
    }
    match server.split_once(':') {
        Some((host, port)) => (host.to_string(), port.parse().ok()),
        None => (server.to_string(), None),
    }
}

fn facility_code(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "kern" => 0,
        "user" => 1,
        "mail" => 2,
        "daemon" => 3,
        "auth" | "security" => 4,
        "syslog" => 5,
        "lpr" => 6,
        "news" => 7,
        "uucp" => 8,
        "cron" => 9,
        "authpriv" => 10,
        "ftp" => 11,
        "local0" => 16,
        "local1" => 17,
        "local2" => 18,
        "local3" => 19,
        "local4" => 20,
        "local5" => 21,
        "local6" => 22,
        "local7" => 23,
        other => {
            tracing::warn!("unknown syslog facility '{other}', using local6");
            22
        }
    }
}

fn severity_code(name: &str) -> u8 {
    match name.to_ascii_lowercase().as_str() {
        "emerg" | "panic" => 0,
        "alert" => 1,
        "crit" => 2,
        "err" | "error" => 3,
        "warning" | "warn" => 4,
        "notice" => 5,
        "info" => 6,
        "debug" => 7,
        other => {
            tracing::warn!("unknown syslog level '{other}', using info");
            6
        }
    }
}

/// The local datagram: `<PRI>TAG: MESSAGE`.
///
/// No timestamp and no host - journald and rsyslog fill both in, and anything else
/// here ends up inside the message text and costs the record its identifier.
fn format_local(priority: u8, tag: &str, message: &str) -> String {
    format!("<{priority}>{}: {message}", sanitize_tag(tag))
}

/// The remote datagram, RFC 5424: `<PRI>1 TIMESTAMP HOST APP - - MESSAGE`.
fn format_rfc5424(priority: u8, host: &str, tag: &str, message: &str) -> String {
    let timestamp = datefmt::rfc5424_timestamp(datefmt::now_secs());
    let host = if host.is_empty() { "-" } else { host };
    format!(
        "<{priority}>1 {timestamp} {host} {} - - {message}",
        sanitize_tag(tag)
    )
}

/// The record's identity, which is what receiver-side rules match on.
///
/// `logger` used the invoking user's name, so under the Agent these records have
/// always been tagged with the account netdata runs as.
fn syslog_tag() -> String {
    for key in ["LOGNAME", "USER"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return value;
            }
        }
    }
    "netdata".to_string()
}

/// Syslog tags are alphanumeric; anything else would terminate the tag.
fn sanitize_tag(tag: &str) -> String {
    let cleaned: String = tag
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .collect();
    if cleaned.is_empty() {
        "netdata".to_string()
    } else {
        cleaned
    }
}

fn deliver(target: &Target, record: &[u8]) -> std::io::Result<()> {
    match &target.server {
        Some(server) => send_remote(server, target.port, record),
        None => send_local(record),
    }
}

fn send_remote(server: &str, port: u16, record: &[u8]) -> std::io::Result<()> {
    let addr = (server, port);
    let socket = UdpSocket::bind(("0.0.0.0", 0))?;
    if socket.send_to(record, addr).is_ok() {
        return Ok(());
    }
    // Some collectors only listen on TCP; try that before giving up.
    let sock_addr = addr
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other("could not resolve the syslog server"))?;
    let mut stream = TcpStream::connect_timeout(&sock_addr, Duration::from_secs(5))?;
    stream.write_all(record)?;
    stream.write_all(b"\n")?;
    Ok(())
}

#[cfg(unix)]
fn send_local(record: &[u8]) -> std::io::Result<()> {
    use std::os::unix::net::UnixDatagram;

    // The usual local endpoints: Linux/BSD, then macOS.
    let mut last_error = None;
    for path in ["/dev/log", "/var/run/syslog", "/var/run/log"] {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        match UnixDatagram::unbound().and_then(|s| s.send_to(record, path).map(|_| ())) {
            Ok(()) => return Ok(()),
            Err(e) => last_error = Some(e),
        }
    }
    // A local UDP listener is the last resort.
    match UdpSocket::bind(("0.0.0.0", 0)).and_then(|s| {
        s.send_to(record, ("127.0.0.1", DEFAULT_SYSLOG_PORT))
            .map(|_| ())
    }) {
        Ok(()) => Ok(()),
        Err(e) => Err(last_error.unwrap_or(e)),
    }
}

#[cfg(not(unix))]
fn send_local(_record: &[u8]) -> std::io::Result<()> {
    // Windows has no syslog daemon. Rather than silently dropping the record, say so:
    // the recipient must name a server, as in `local6.info@collector:514/netdata`.
    Err(std::io::Error::other(
        "this platform has no local syslog socket; configure a remote target such as \
         'local6.info@host:514/prefix'",
    ))
}

#[cfg(test)]
#[path = "syslog_tests.rs"]
mod tests;
