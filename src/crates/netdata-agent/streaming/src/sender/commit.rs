//! `sender_buffer_commit()` (`src/streaming/stream-sender-commit.c`): a rendered message into the sender's buffer,
//! compressed in C's pieces when the link is compressed, and the opcode that sends it, reports an overflow or
//! restarts without compression. Every commit takes the host's one lock (D100.7). Map:
//! `knowledge/map-m7-commit4-runtime.md` §2.

use netdata_agent_log::{ErrorLimit, Priority, Source, nd_log, nd_log_limit};

use super::buffer::{ADAPT_TO_TIMES_MAX_SIZE, Traffic};
use super::{Out, Sender, op};
use crate::caps;
use crate::compress::{Compressor, next_piece};
use crate::compression::{Algorithm, encode_signature};
use crate::reason::Reason;

/// How a commit ended, for the opcode posted after the lock is released.
enum Ended {
    Added { enable_sending: bool },
    Overflow,
    CompressionFailed,
}

/// `nd_log_limit_static_global_var(erl, 1, 0)`: one record a second, for all hosts.
static OVERFLOW_RECORD: ErrorLimit = ErrorLimit::new(1, 0);
static COMPRESSION_RECORD: ErrorLimit = ErrorLimit::new(1, 0);

impl Sender {
    /// `sender_buffer_commit()`: dropped without a dispatched connection (no session).
    pub fn commit(&self, src: &[u8], traffic: Traffic) {
        if src.is_empty() {
            return;
        }
        let hostname = self.hostname();
        let mut out = self.out();
        let Some(session) = out.session else {
            return;
        };
        if out.buffer.set_max_size(src.len() * ADAPT_TO_TIMES_MAX_SIZE, false) {
            nd_log!(
                Source::Daemon,
                Priority::Notice,
                "STREAM SND '{hostname}' [to {}]: Increased max buffer size to {} (message size {}).",
                out.remote_ip,
                out.buffer.stats().bytes_max_size,
                src.len() + 1
            );
        }
        let enable_sending = out.buffer.stats().bytes_outstanding == 0;
        let ended = if out.compressor.is_some() {
            compressed(&mut out, &hostname, src, traffic, enable_sending)
        } else if out.buffer.add(src, src.len(), traffic) {
            Ended::Added { enable_sending }
        } else {
            Ended::Overflow
        };
        match ended {
            Ended::Added { enable_sending } => {
                drop(out);
                if enable_sending {
                    self.post(session, op::POLLOUT, Reason::NEVER);
                }
            }
            Ended::Overflow => {
                let stats = *out.buffer.stats();
                let remote_ip = out.remote_ip.clone();
                drop(out);
                self.post(session, op::BUFFER_OVERFLOW, Reason::DISCONNECT_BUFFER_OVERFLOW);
                nd_log_limit!(
                    &OVERFLOW_RECORD,
                    Source::Daemon,
                    Priority::Err,
                    "STREAM SND '{hostname}' [to {remote_ip}]: buffer overflow (buffer size {}, max size {}, available \
                     {}). Restarting connection.",
                    stats.bytes_size,
                    stats.bytes_max_size,
                    stats.bytes_available
                );
            }
            Ended::CompressionFailed => {
                self.deactivate_compression(&out, &hostname);
                let remote_ip = out.remote_ip.clone();
                drop(out);
                self.post(session, op::RECONNECT_WITHOUT_COMPRESSION, Reason::SND_DISCONNECT_COMPRESSION_FAILED);
                nd_log_limit!(
                    &COMPRESSION_RECORD,
                    Source::Daemon,
                    Priority::Err,
                    "STREAM SND '{hostname}' [to {remote_ip}]: COMPRESSION failed (twice). Deactivating compression \
                     and restarting connection."
                );
            }
        }
    }

    /// `stream_compression_deactivate()`: the algorithm that failed is not offered at the next connection.
    fn deactivate_compression(&self, out: &Out, hostname: &str) {
        let (name, cap) = match out.algorithm {
            Some(Algorithm::Gzip) => ("GZIP", caps::GZIP),
            // C names ZSTD in the lz4 record
            Some(Algorithm::Lz4) => ("LZ4", caps::LZ4),
            Some(Algorithm::Zstd) => ("ZSTD", caps::ZSTD),
            Some(Algorithm::Brotli) => ("BROTLI", caps::BROTLI),
            None => {
                nd_log!(
                    Source::Daemon,
                    Priority::Err,
                    "STREAM_COMPRESSION: compression error on 'host:{hostname}' without any compression enabled. \
                     Ignoring error."
                );
                return;
            }
        };
        let disabling = if out.algorithm == Some(Algorithm::Lz4) { "ZSTD" } else { name };
        nd_log!(
            Source::Daemon,
            Priority::Err,
            "STREAM_COMPRESSION: {name} compression error on 'host:{hostname}'. Disabling {disabling} for this node."
        );
        self.disabled.fetch_or(cap, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The compressed branch: each piece compressed (after a failure the compressor is set up again from the
/// capabilities and the piece retried once), then its signature and its bytes added. Pieces added before a failure
/// stay in the buffer.
fn compressed(out: &mut Out, hostname: &str, src: &[u8], traffic: Traffic, enable_sending: bool) -> Ended {
    let binary = out.capabilities & caps::BINARY != 0;
    let mut rest = src;
    while !rest.is_empty() {
        let piece = &rest[..next_piece(rest, binary)];
        let compress = |out: &mut Out| out.compressor.as_mut().and_then(|c| c.compress(piece)).map(<[u8]>::len);
        let mut len = compress(out);
        if len.is_none() {
            nd_log!(
                Source::Daemon,
                Priority::Err,
                "STREAM SND '{hostname}' [to {}]: COMPRESSION failed. Resetting compressor and re-trying",
                out.remote_ip
            );
            // stream_compression_initialize()
            out.algorithm = Algorithm::for_capabilities(out.capabilities);
            out.compressor = out.algorithm.and_then(|a| Compressor::new(a, &out.levels));
            if out.compressor.is_none() {
                return Ended::CompressionFailed;
            }
            len = compress(out);
        }
        let (Some(len), Some(compressor)) = (len, out.compressor.as_ref()) else {
            return Ended::CompressionFailed;
        };
        let compressed = compressor.output(len);
        let Some(signature) = encode_signature(compressed.len()) else {
            return Ended::CompressionFailed;
        };
        if !out.buffer.add(&signature, signature.len(), traffic) || !out.buffer.add(compressed, piece.len(), traffic) {
            return Ended::Overflow;
        }
        rest = &rest[piece.len()..];
    }
    Ended::Added { enable_sending }
}
