//! Native IRC delivery.
//!
//! The script piped a hand-written IRC session into `nc`, which is not available on
//! Windows and not guaranteed anywhere. The protocol exchange is small enough to do
//! directly, and doing so keeps the same registration sequence, the same
//! newline-to-", " flattening of the message, and the same success rule: any 4xx or
//! 5xx numeric reply from the server means failure.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::senders::Ctx;
use crate::textutil::underscores_to_spaces;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const READ_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn irc(ctx: &Ctx<'_>) -> bool {
    if !ctx.enabled("irc") {
        return false;
    }
    let nickname = ctx.cfg.str("IRC_NICKNAME");
    let realname = ctx.cfg.str("IRC_REALNAME");
    let network = ctx.cfg.str("IRC_NETWORK");
    let port: u16 = ctx.cfg.str("IRC_PORT").trim().parse().unwrap_or(6667);
    let servername = &ctx.msg.host;

    if nickname.is_empty() || realname.is_empty() || network.is_empty() {
        return false;
    }

    let m = ctx.msg;
    let message = format!(
        "{} {} - {} - {} ----- {}\nSeverity: {}\nChart: {}\n{}",
        m.host,
        m.status_message,
        underscores_to_spaces(&ctx.args.name),
        ctx.args.chart,
        m.alarm,
        m.severity,
        ctx.args.chart,
        ctx.args.info
    );
    // IRC has no multi-line messages.
    let single_line = message.replace('\n', ", ");

    let mut sent = false;
    for channel in ctx.to("irc") {
        let session = format!(
            "USER {nickname} guest {realname} {servername}\r\nNICK {nickname}\r\nJOIN {channel}\r\nPRIVMSG {channel} :{single_line}\r\nQUIT\r\n"
        );
        match run_session(network, port, &session) {
            Ok(reply) => match error_code(&reply) {
                None => {
                    tracing::info!("sent irc notification to '{channel}' for {}", ctx.what());
                    sent = true;
                }
                Some(code) => tracing::error!(
                    "failed to send irc notification to '{channel}' for {}, with error code {code}.",
                    ctx.what()
                ),
            },
            Err(e) => tracing::error!(
                "failed to send irc notification to '{channel}' for {}: {e}",
                ctx.what()
            ),
        }
    }
    sent
}

fn run_session(network: &str, port: u16, session: &str) -> std::io::Result<String> {
    let addr = (network, port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| std::io::Error::other("could not resolve the IRC server"))?;
    let mut stream = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(READ_TIMEOUT))?;
    stream.write_all(session.as_bytes())?;
    stream.flush()?;

    // The server closes the connection after QUIT; a read timeout is normal and not
    // an error, so partial output is still inspected.
    let mut reply = String::new();
    let mut buf = [0u8; 4096];
    loop {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => reply.push_str(&String::from_utf8_lossy(&buf[..n])),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(e) => return Err(e),
        }
        if reply.len() > 64 * 1024 {
            break;
        }
    }
    Ok(reply)
}

/// First 4xx/5xx numeric reply in the server's response, if any.
///
/// IRC numerics sit in the second field of a `:server NNN nick ...` line.
fn error_code(reply: &str) -> Option<u16> {
    for line in reply.lines() {
        if let Some(field) = line.split_whitespace().nth(1) {
            if let Ok(code) = field.parse::<u16>() {
                if (400..=599).contains(&code) {
                    return Some(code);
                }
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "irc_tests.rs"]
mod tests;
